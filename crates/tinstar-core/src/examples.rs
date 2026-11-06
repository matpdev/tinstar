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
            (Check::Layout(layout), example) => {
                let all: Vec<PathBuf> = match example {
                    Example::Files(files) => files.iter().map(PathBuf::from).collect(),
                    // A single path, or one path per line, is a small project too.
                    Example::Source(text) if is_path_list(text) => text
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(PathBuf::from)
                        .collect(),
                    Example::Source(_) => {
                        return Err("layout examples are file paths (a list of paths per \
                                    example, e.g. [\"src/a.rs\", \"src/b.rs\"]), not code"
                            .into());
                    }
                };
                let selected: Vec<&Path> = all
                    .iter()
                    .map(PathBuf::as_path)
                    .filter(|p| self.selects(p))
                    .collect();
                Ok(layout.check(&selected, &all).len())
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

/// Text that is only paths, one per line (no spaces, so not code).
fn is_path_list(text: &str) -> bool {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .peekable();
    lines.peek().is_some()
        && lines.all(|l| !l.contains(char::is_whitespace) && l.contains(['/', '.']))
}

fn show(e: &Example) -> String {
    match e {
        Example::Source(s) => s.clone(),
        Example::Files(f) => f.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use crate::Law;

    #[test]
    fn a_layout_example_can_be_one_path() {
        let law = Law::from_yaml(
            r#"
id: no-mod-rs
kind: layout
message: no mod.rs
files: ["crates/**/mod.rs"]
forbid: true
examples:
  valid: ["crates/editor/src/editor.rs", ["crates/vim/src/vim.rs", "crates/vim/src/normal.rs"]]
  invalid: ["crates/vim/src/normal/mod.rs", "crates/a/src/a.rs\ncrates/a/src/b/mod.rs"]
"#,
        )
        .unwrap();
        let report = law.test_examples();
        assert!(
            report.failures.is_empty(),
            "{:?}",
            report
                .failures
                .iter()
                .map(|f| &f.reason)
                .collect::<Vec<_>>()
        );
        assert_eq!(report.passed, 4);
    }
}
