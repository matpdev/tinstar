//! A law's own examples: snippets (or file lists) it must accept and must reject. A law whose
//! examples fail is not trusted — this is what keeps generated laws honest.

use std::path::{Path, PathBuf};

use ast_grep_core::tree_sitter::LanguageExt;

use crate::check::Check;
use crate::law::{Example, Law};

pub struct ExampleReport {
    pub law: String,
    pub passed: usize,
    pub failures: Vec<ExampleFailure>,
}

pub struct ExampleFailure {
    /// `valid` or `invalid`, and its index in that list.
    pub which: &'static str,
    pub index: usize,
    pub example: String,
    pub reason: String,
}

impl Law {
    pub fn has_examples(&self) -> bool {
        !self.examples.valid.is_empty() || !self.examples.invalid.is_empty()
    }

    /// Runs the law on each of its examples: valid ones must give no hit, invalid ones at least one.
    pub fn test_examples(&self) -> ExampleReport {
        let mut report = ExampleReport {
            law: self.id.clone(),
            passed: 0,
            failures: Vec::new(),
        };
        let cases = self
            .examples
            .valid
            .iter()
            .enumerate()
            .map(|(i, e)| ("valid", i, e, false))
            .chain(
                self.examples
                    .invalid
                    .iter()
                    .enumerate()
                    .map(|(i, e)| ("invalid", i, e, true)),
            );
        for (which, index, example, should_hit) in cases {
            let fail = |reason: String| ExampleFailure {
                which,
                index,
                example: show(example),
                reason,
            };
            match self.hits_in(example) {
                Err(reason) => report.failures.push(fail(reason)),
                Ok(n) if (n > 0) == should_hit => report.passed += 1,
                Ok(0) => report.failures.push(fail("the law did not fire".into())),
                Ok(n) => report
                    .failures
                    .push(fail(format!("the law fired {n} time(s)"))),
            }
        }
        report
    }

    fn hits_in(&self, example: &Example) -> Result<usize, String> {
        match (&self.check, example) {
            (Check::Layout(layout), Example::Files(files)) => {
                let all: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
                let selected: Vec<&Path> = all
                    .iter()
                    .map(PathBuf::as_path)
                    .filter(|p| self.selects(p))
                    .collect();
                Ok(layout.check(&selected, &all).len())
            }
            (Check::Layout(_), Example::Source(_)) => {
                Err("layout examples are lists of paths, not code".into())
            }
            (check, Example::Source(src)) => {
                let lang = check.language().expect("per-file checks have a language");
                Ok(check.check_tree(&lang.ast_grep(src)).len())
            }
            (_, Example::Files(_)) => {
                Err("this law's examples are code, not lists of paths".into())
            }
        }
    }
}

fn show(e: &Example) -> String {
    match e {
        Example::Source(s) => s.clone(),
        Example::Files(f) => f.join(", "),
    }
}
