//! All of a project's laws, and the entry points every front-end uses.

use std::path::{Path, PathBuf};

use ast_grep_core::Language;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::SupportLang;
use rayon::prelude::*;

use crate::check::{Check, Hit};
use crate::diag::Diagnostic;
use crate::law::{Law, LawError};
use crate::lines::LineIndex;

/// Where laws live, relative to the project root. `pending/` below it holds laws not yet
/// approved (what `tinstar compile` writes) and is not loaded.
pub const LAWS_DIR: &str = ".tinstar/laws";

pub struct Lawbook {
    pub root: PathBuf,
    pub laws: Vec<Law>,
}

#[derive(Debug, thiserror::Error)]
#[error("{}: {error}", path.display())]
pub struct LoadError {
    pub path: PathBuf,
    pub error: LawError,
}

/// The nearest folder, from `start` up, that has a `.tinstar/` folder.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(".tinstar").is_dir())
        .map(Path::to_path_buf)
}

/// The project's files, relative to `root`: .gitignore is honoured, hidden files are included
/// (`.github/`, `.env`), and `.git/` and `.tinstar/` are skipped.
pub fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | ".tinstar")))
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| e.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .collect();
    files.sort();
    files
}

impl Lawbook {
    /// Loads every `*.yaml` / `*.yml` under `.tinstar/laws/` (except `pending/`). Laws that fail to
    /// load are returned as errors next to the ones that loaded, so one typo doesn't silence the
    /// whole book.
    pub fn load(root: &Path) -> (Lawbook, Vec<LoadError>) {
        let dir = root.join(LAWS_DIR);
        let mut paths: Vec<PathBuf> = ignore::WalkBuilder::new(&dir)
            .hidden(false)
            .standard_filters(false)
            .filter_entry(|e| e.file_name() != "pending")
            .build()
            .filter_map(Result::ok)
            .map(|e| e.into_path())
            .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
            .collect();
        paths.sort();

        let mut laws: Vec<Law> = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            let result = std::fs::read_to_string(&path)
                .map_err(|e| LawError::Invalid(e.to_string()))
                .and_then(|text| Law::from_yaml(&text));
            match result {
                Ok(law) if laws.iter().any(|l| l.id == law.id) => errors.push(LoadError {
                    path: rel,
                    error: LawError::Invalid(format!(
                        "another law already has the id `{}`",
                        law.id
                    )),
                }),
                Ok(law) => laws.push(law),
                Err(error) => errors.push(LoadError { path: rel, error }),
            }
        }
        (
            Lawbook {
                root: root.to_path_buf(),
                laws,
            },
            errors,
        )
    }

    /// A lawbook made of laws already in hand (tests, `tinstar test`).
    pub fn from_laws(root: &Path, laws: Vec<Law>) -> Lawbook {
        Lawbook {
            root: root.to_path_buf(),
            laws,
        }
    }

    /// Checks one file's contents (as on disk, or as an editor holds it). `rel` is relative to
    /// the root and decides which laws apply. The file is parsed once per language, and that tree
    /// is shared by every law of the language.
    pub fn check_source(&self, rel: &Path, text: &str) -> Vec<Diagnostic> {
        let Some(file_lang) = SupportLang::from_path(rel) else {
            return Vec::new();
        };
        let laws: Vec<&Law> = self
            .laws
            .iter()
            .filter(|l| l.check.language() == Some(file_lang) && l.selects(rel))
            .collect();
        if laws.is_empty() {
            return Vec::new();
        }
        let root = file_lang.ast_grep(text);
        let lines = LineIndex::new(text);
        let mut out = Vec::new();
        for law in laws {
            for hit in law.check.check_tree(&root) {
                out.push(self.diagnostic(
                    law,
                    rel,
                    Some((&lines, &hit)),
                    hit.message.clone(),
                    hit.fix.clone(),
                ));
            }
        }
        sort(&mut out);
        out
    }

    /// Runs the layout laws over a list of project files (relative paths).
    pub fn check_layout(&self, all: &[PathBuf]) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for law in &self.laws {
            let Check::Layout(layout) = &law.check else {
                continue;
            };
            let selected: Vec<&Path> = all
                .iter()
                .map(PathBuf::as_path)
                .filter(|p| law.selects(p))
                .collect();
            for hit in layout.check(&selected, all) {
                let message = format!("{} ({})", law.message, hit.detail);
                out.push(self.diagnostic(law, &hit.path, None, Some(message), None));
            }
        }
        sort(&mut out);
        out
    }

    /// Checks files on disk. With `targets`, per-file laws run on those files only and layout
    /// results are kept when they concern them (the file itself, or a folder that holds it).
    pub fn check_project(&self, targets: Option<&[PathBuf]>) -> Vec<Diagnostic> {
        let all = walk_files(&self.root);
        let files: Vec<&PathBuf> = match targets {
            Some(t) => all
                .iter()
                .filter(|f| t.iter().any(|t| f.starts_with(t)))
                .collect(),
            None => all.iter().collect(),
        };
        // Files are independent: check them on every core. Each worker reads, parses and checks one
        // file at a time, so memory stays at a few trees, not the whole repository.
        let mut out: Vec<Diagnostic> = files
            .par_iter()
            .filter(|rel| SupportLang::from_path(rel).is_some())
            .flat_map_iter(|rel| match std::fs::read_to_string(self.root.join(rel)) {
                Ok(text) => self.check_source(rel, &text),
                Err(_) => Vec::new(),
            })
            .collect();
        let layout = self.check_layout(&all);
        out.extend(layout.into_iter().filter(|d| {
            match targets {
                None => true,
                Some(t) => t
                    .iter()
                    .any(|t| t.starts_with(&d.path) || d.path.starts_with(t)),
            }
        }));
        sort(&mut out);
        out
    }

    fn diagnostic(
        &self,
        law: &Law,
        path: &Path,
        at: Option<(&LineIndex, &Hit)>,
        message: Option<String>,
        fix: Option<crate::diag::Fix>,
    ) -> Diagnostic {
        Diagnostic {
            law: law.id.clone(),
            severity: law.severity,
            message: message
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| law.message.clone()),
            path: path.to_path_buf(),
            span: at.map(|(lines, hit)| lines.span(hit.start, hit.end)),
            statement: law.statement.clone(),
            source: law.source.clone(),
            hint: law.hint.clone(),
            fix,
        }
    }
}

fn sort(d: &mut [Diagnostic]) {
    d.sort_by(|a, b| {
        (&a.path, a.span.map(|s| s.start), &a.law).cmp(&(&b.path, b.span.map(|s| s.start), &b.law))
    });
}
