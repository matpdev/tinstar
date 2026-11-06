//! `tinstar compile` end to end, with recorded LLM answers (no network): a law that gets its own
//! example wrong goes back with the error and comes back fixed; only passing laws are written.

use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fresh_shop(name: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop");
    let dir = std::env::temp_dir().join(format!("tinstar-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&src, &dir);
    for law in std::fs::read_dir(dir.join(".tinstar/laws")).unwrap() {
        std::fs::remove_file(law.unwrap().path()).unwrap();
    }
    dir
}

fn tinstar(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_tinstar"))
        .args(args)
        .current_dir(dir)
        .env(
            "TINSTAR_LLM_REPLAY",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compile-replay.json"),
        )
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn drafts_validates_retries_and_approves() {
    let dir = fresh_shop("compile");
    let (code, out) = tinstar(
        &dir,
        &[
            "compile",
            "--section",
            "money",
            "--section",
            "containers",
            "--section",
            "Things a checker can't see",
        ],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("✓ money-is-cents: 4 example(s) pass, 1 hit(s) in the project, fixed from feedback (answer 2)"), "{out}");
    assert!(
        out.contains("✓ containers-have-limits: 4 example(s) pass, 1 hit(s) in the project\n"),
        "{out}"
    );
    assert!(out.contains("✓ no-latest-images"), "{out}");
    assert!(
        out.contains("stays in AGENTS.md: \"Discounts never exceed 30% of the order total.\""),
        "{out}"
    );
    assert!(out.contains("3 law(s) drafted"), "{out}");

    let pending = dir.join(".tinstar/laws/pending");
    let money = std::fs::read_to_string(pending.join("money-is-cents.yaml")).unwrap();
    assert!(money.starts_with("# Drafted by `tinstar compile` from AGENTS.md#money"));
    assert!(
        money.contains("field: pattern"),
        "the corrected version is the one written"
    );

    // Pending laws are not enforced until approved.
    let (code, _) = tinstar(&dir, &["check"]);
    assert_eq!(code, 0);
    let (code, out) = tinstar(&dir, &["approve", "--all"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = tinstar(&dir, &["check", "--format", "agent"]);
    assert_eq!(code, 1);
    assert_eq!(out.lines().count(), 3, "{out}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rejects_laws_that_never_pass() {
    let dir = fresh_shop("reject");
    // One attempt only: the flawed first answer for `money` is rejected, nothing is written.
    let (code, out) = tinstar(&dir, &["compile", "--section", "money", "--attempts", "1"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("✗ money-is-cents: rejected after 1 attempt(s): gets its own examples wrong"),
        "{out}"
    );
    assert!(out.contains("0 law(s) drafted"), "{out}");
    std::fs::remove_dir_all(&dir).unwrap();
}
