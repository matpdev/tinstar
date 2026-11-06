//! The checkers. Each law holds one (a strategy picked by `kind`), and all of them report
//! [`Hit`]s, which the lawbook turns into diagnostics with the law's metadata.

pub(crate) mod config;
pub(crate) mod layout;
pub(crate) mod pattern;

use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_language::SupportLang;

use crate::diag::Fix;

pub(crate) type Root = ast_grep_core::AstGrep<StrDoc<SupportLang>>;

pub(crate) enum Check {
    Pattern(Box<pattern::PatternCheck>),
    Config(config::ConfigCheck),
    Layout(layout::LayoutCheck),
}

impl Check {
    /// The language whose syntax tree this check reads (none for layout laws).
    pub fn language(&self) -> Option<SupportLang> {
        match self {
            Check::Pattern(p) => Some(p.lang),
            Check::Config(c) => Some(c.lang),
            Check::Layout(_) => None,
        }
    }

    /// Runs a per-file check on an already parsed file. The tree is parsed once per file and
    /// shared by every law of that language.
    pub fn check_tree(&self, root: &Root) -> Vec<Hit> {
        match self {
            Check::Pattern(p) => p.check(root),
            Check::Config(c) => c.check(root),
            Check::Layout(_) => Vec::new(),
        }
    }
}

/// Where a law was broken in a file, before the law's metadata is attached.
pub(crate) struct Hit {
    pub start: usize,
    pub end: usize,
    /// Set when the message depends on the match (ast-grep `$VARS`, a config assertion).
    pub message: Option<String>,
    pub fix: Option<Fix>,
}
