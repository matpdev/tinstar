//! Tinstar's engine: laws (rules written for a repository) compiled into checks that run without an
//! LLM. Front-ends (the CLI, the language server, agent hooks) only load a [`Lawbook`] and turn its
//! [`Diagnostic`]s into their own output, so every one of them reports the same thing.

mod check;
mod diag;
mod examples;
mod law;
mod lawbook;
mod lines;

pub use diag::{Diagnostic, Fix, Severity, Span};
pub use examples::{ExampleFailure, ExampleReport};
pub use law::{Kind, Law, LawError};
pub use lawbook::{LAWS_DIR, Lawbook, LoadError, find_root, walk_files};
pub use lines::LineIndex;
