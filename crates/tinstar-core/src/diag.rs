use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How serious a broken law is. Only `error` makes `tinstar check` fail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Hint,
    Info,
    Warning,
    #[default]
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Hint => "hint",
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// A region of a file. Byte offsets for edits; 1-based line and character column for people.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// A mechanical fix: replace `start..end` (bytes) with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// One broken law, at one place.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// The law's id, stable across runs: what tools and agents key on.
    pub law: String,
    pub severity: Severity,
    pub message: String,
    /// Relative to the project root. A directory for layout laws about folders.
    pub path: PathBuf,
    /// `None` when the law is about the file (or folder) as a whole.
    pub span: Option<Span>,
    /// The law as people wrote it, in plain language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    /// Where that sentence lives, e.g. `AGENTS.md#money`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// How to fix it, for people and agents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}
