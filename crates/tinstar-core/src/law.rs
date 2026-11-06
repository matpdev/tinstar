//! A law file (`.tinstar/laws/*.yaml`) and its compiled form.

use std::collections::BTreeMap;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_yaml::Value;

use crate::check::{Check, config::ConfigCheck, layout::LayoutCheck, pattern::PatternCheck};
use crate::diag::Severity;

/// Which checker a law uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A code pattern, matched on the syntax tree (ast-grep): code, YAML, JSON, HCL, Bash…
    #[default]
    Pattern,
    /// Assertions on structured data (YAML / JSON documents): Kubernetes, compose, CI workflows…
    Config,
    /// Files and folders: what must exist, what must not, how things are named.
    Layout,
}

/// The YAML as written. Every field is listed here so a typo is an error, not a silent no-op.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LawFile {
    pub id: String,
    #[serde(default)]
    pub kind: Kind,
    pub message: String,
    #[serde(default)]
    pub severity: Severity,
    pub statement: Option<String>,
    pub source: Option<String>,
    pub hint: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub ignores: Vec<String>,
    pub language: Option<String>,

    // kind: pattern (ast-grep's rule fields, passed through)
    pub rule: Option<Value>,
    pub constraints: Option<Value>,
    pub utils: Option<Value>,
    pub transform: Option<Value>,
    pub fix: Option<Value>,

    // kind: config
    #[serde(rename = "match", default)]
    pub match_: BTreeMap<String, Value>,
    #[serde(rename = "assert", default)]
    pub asserts: Vec<Assertion>,

    // kind: layout
    #[serde(default)]
    pub forbid: bool,
    pub name_matches: Option<String>,
    pub each_dir: Option<String>,
    #[serde(default)]
    pub require: Vec<String>,

    #[serde(default)]
    pub examples: Examples,
}

/// One assertion of a config law, on every value a path reaches.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Assertion {
    /// Dotted path; `*` is any key of a mapping, `[*]` any item of a sequence.
    pub path: String,
    /// `true`: the path must exist. `false`: it must not.
    pub exists: Option<bool>,
    pub matches: Option<String>,
    pub not_matches: Option<String>,
    pub equals: Option<Value>,
    pub one_of: Option<Vec<Value>>,
    /// Overrides the law's message for this assertion.
    pub message: Option<String>,
}

/// Code (or file lists, for layout laws) the law must accept and must reject.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Examples {
    #[serde(default)]
    pub valid: Vec<Example>,
    #[serde(default)]
    pub invalid: Vec<Example>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Example {
    /// A snippet, for pattern and config laws.
    Source(String),
    /// A list of paths, for layout laws.
    Files(Vec<String>),
}

#[derive(Debug, thiserror::Error)]
pub enum LawError {
    #[error("invalid YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("{0}")]
    Invalid(String),
}

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T, LawError> {
    Err(LawError::Invalid(msg.into()))
}

/// A law, compiled and ready to check files.
pub struct Law {
    pub id: String,
    pub kind: Kind,
    pub message: String,
    pub severity: Severity,
    pub statement: Option<String>,
    pub source: Option<String>,
    pub hint: Option<String>,
    files: Option<GlobSet>,
    ignores: Option<GlobSet>,
    pub(crate) check: Check,
    pub examples: Examples,
}

impl Law {
    pub fn from_yaml(text: &str) -> Result<Law, LawError> {
        let file: LawFile = serde_yaml::from_str(text)?;
        if file.id.trim().is_empty() {
            return invalid("`id` is empty");
        }
        let check = match file.kind {
            Kind::Pattern => Check::Pattern(Box::new(PatternCheck::compile(&file)?)),
            Kind::Config => Check::Config(ConfigCheck::compile(&file)?),
            Kind::Layout => Check::Layout(LayoutCheck::compile(&file)?),
        };
        Ok(Law {
            files: globs(&file.files)?,
            ignores: globs(&file.ignores)?,
            id: file.id,
            kind: file.kind,
            message: file.message,
            severity: file.severity,
            statement: file.statement,
            source: file.source,
            hint: file.hint,
            check,
            examples: file.examples,
        })
    }

    /// Whether the law's `files` / `ignores` globs select this path (relative to the root).
    pub fn selects(&self, rel: &Path) -> bool {
        self.files.as_ref().is_none_or(|g| g.is_match(rel))
            && !self.ignores.as_ref().is_some_and(|g| g.is_match(rel))
    }
}

/// Globs where `*` stays inside one path segment and `**` crosses them, as in .gitignore.
pub(crate) fn glob(pattern: &str) -> Result<Glob, LawError> {
    globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .or_else(|e| invalid(format!("bad glob `{pattern}`: {e}")))
}

fn globs(patterns: &[String]) -> Result<Option<GlobSet>, LawError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut set = GlobSetBuilder::new();
    for p in patterns {
        set.add(glob(p)?);
    }
    set.build().map(Some).or_else(|e| invalid(e.to_string()))
}
