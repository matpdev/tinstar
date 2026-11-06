//! The noise check: the project itself is a test. A law that passes its own examples but fires
//! hundreds of times in a project that already follows (most of) its rules is probably broader
//! than the rule: it goes back to the LLM with numbers and a sample of what it caught, to be
//! narrowed, confirmed or set aside.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tinstar_core::Diagnostic;

/// How many hits the LLM gets to see.
const SAMPLE: usize = 10;

/// Test code: the most common reason a law fires everywhere (tests unwrap, print, sleep…).
pub fn is_test(path: &Path) -> bool {
    path.components().any(|c| {
        let c = c.as_os_str().to_string_lossy().to_lowercase();
        matches!(
            c.as_str(),
            "test" | "tests" | "spec" | "specs" | "__tests__" | "testdata" | "fixtures"
        ) || c.contains("_test.")
            || c.contains(".test.")
            || c.contains("_tests.")
            || c.contains(".spec.")
            || c.starts_with("test_")
    })
}

/// The feedback for a law that fires too often: how much, where, and a sample with the code.
pub fn report(id: &str, hits: &[Diagnostic], root: &Path) -> String {
    let files: BTreeSet<&PathBuf> = hits.iter().map(|d| &d.path).collect();
    let in_test_files = hits.iter().filter(|d| is_test(&d.path)).count();
    let mut first_test_line: BTreeMap<&PathBuf, Option<usize>> = BTreeMap::new();
    let inline = hits
        .iter()
        .filter(|d| !is_test(&d.path))
        .filter(|d| {
            let start = *first_test_line
                .entry(&d.path)
                .or_insert_with(|| inline_tests_start(&root.join(&d.path)));
            matches!((start, d.span), (Some(t), Some(s)) if s.line > t)
        })
        .count();

    let mut by_folder: BTreeMap<String, usize> = BTreeMap::new();
    for d in hits {
        let folder: PathBuf = d
            .path
            .parent()
            .into_iter()
            .flat_map(Path::components)
            .take(2)
            .collect();
        *by_folder
            .entry(format!("{}/", folder.display()))
            .or_default() += 1;
    }
    let mut folders: Vec<(String, usize)> = by_folder.into_iter().collect();
    folders.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let folders: Vec<String> = folders
        .iter()
        .take(6)
        .map(|(f, n)| format!("{f} ({n})"))
        .collect();

    let mut out = format!(
        "### {id}\nIt fires {} time(s) in {} file(s) of this project today. In test code: \
         {in_test_files} in test files, {inline} in tests inside source files (after `#[cfg(test)]`). \
         Most hits: {}.\nA sample:\n",
        hits.len(),
        files.len(),
        folders.join(", ")
    );
    for d in sample(hits) {
        let line = d
            .span
            .and_then(|s| {
                let text = std::fs::read_to_string(root.join(&d.path)).ok()?;
                Some(format!(
                    ":{}: `{}`",
                    s.line,
                    text.lines().nth(s.line - 1)?.trim()
                ))
            })
            .unwrap_or_default();
        out.push_str(&format!("- {}{line}\n", d.path.display()));
    }
    out
}

/// Where a Rust file's inline tests start (`#[cfg(test)] mod tests` usually ends the file):
/// a hit below that line is test code, although the file isn't a test file.
fn inline_tests_start(path: &Path) -> Option<usize> {
    if path.extension().is_none_or(|e| e != "rs") {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .position(|l| l.trim_start().starts_with("#[cfg(test)]"))
        .map(|i| i + 1)
}

/// Hits spread over different files, so the sample isn't ten lines of the same file.
fn sample(hits: &[Diagnostic]) -> Vec<&Diagnostic> {
    let mut seen = BTreeSet::new();
    let distinct: Vec<&Diagnostic> = hits.iter().filter(|d| seen.insert(&d.path)).collect();
    let pool = if distinct.len() >= SAMPLE {
        distinct
    } else {
        hits.iter().collect()
    };
    let step = (pool.len() / SAMPLE).max(1);
    pool.into_iter().step_by(step).take(SAMPLE).collect()
}

/// What the LLM must do with the laws in the report.
pub const ASK: &str = "These laws pass their examples but fire often in a project whose rules \
document says the opposite should hold, so they are probably broader than the rule. For each \
one, look at the sample and answer one of:\n\
- a narrower version in `laws` (same id): scope it with `files`/`ignores` (e.g. leave test code \
out when the rule is about production code), or tighten the rule so it only catches what the \
document forbids;\n\
- its id in `confirmed`, with a reason, if the hits really break the rule as written (a project \
can have many old violations);\n\
- the rule in `not_checkable`, if correct and incorrect code can't be told apart by syntax (it \
needs types, runtime values or intent).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code() {
        for p in [
            "crates/editor/src/editor_tests.rs",
            "crates/gpui/tests/app.rs",
            "src/features/checkout/checkout.test.ts",
            "pkg/server/handler_test.go",
            "tests/test_api.py",
        ] {
            assert!(is_test(Path::new(p)), "{p}");
        }
        for p in [
            "crates/editor/src/editor.rs",
            "src/latest.ts",
            "contest/main.rs",
        ] {
            assert!(!is_test(Path::new(p)), "{p}");
        }
    }
}
