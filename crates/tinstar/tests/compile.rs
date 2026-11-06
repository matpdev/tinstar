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
    tinstar_with(dir, "compile-replay.json", args)
}

fn tinstar_with(dir: &Path, replay: &str, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_tinstar"))
        .args(args)
        .current_dir(dir)
        .env(
            "TINSTAR_LLM_REPLAY",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(replay),
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
        out.contains("✗ money-is-cents: rejected after 1 answer(s): gets its own examples wrong"),
        "{out}"
    );
    // The report says which examples failed, and the draft is kept for a person to finish.
    assert!(out.contains("\n      - "), "{out}");
    assert!(
        out.contains("draft kept in .tinstar/laws/pending/rejected/money-is-cents.yaml"),
        "{out}"
    );
    let kept = dir.join(".tinstar/laws/pending/rejected/money-is-cents.yaml");
    assert!(
        std::fs::read_to_string(kept)
            .unwrap()
            .contains("# Last error:")
    );
    assert!(out.contains("0 law(s) drafted"), "{out}");
    // `approve --all` takes laws only, not the rejected folder.
    let (code, _) = tinstar(&dir, &["approve", "--all"]);
    assert_ne!(code, 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A rules document with a section about process: the triage sets it aside, the noise check holds
/// a law that fires (limit 0 here) until the LLM confirms it, and a second run spends nothing.
#[test]
fn triage_noise_check_and_incremental_runs() {
    let dir = fresh_shop("flow");
    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    let section = |name: &str| {
        let start = agents.find(&format!("## {name}")).unwrap();
        let end = agents[start + 3..]
            .find("\n## ")
            .map_or(agents.len(), |e| start + 3 + e);
        agents[start..end].trim().to_string()
    };
    let doc = format!(
        "# Rules\n\n{}\n\n{}\n\n## Pull requests\n\nPR titles are imperative.\n",
        section("Money"),
        section("Containers")
    );
    std::fs::write(dir.join("RULES.md"), &doc).unwrap();

    let args = ["compile", "RULES.md", "--noise-limit", "0"];
    let (code, out) = tinstar_with(&dir, "compile-flow-replay.json", &args);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("compiling 3 of 3 section(s)"), "{out}");
    assert!(
        out.contains("stays in RULES.md: \"PR titles are imperative.\""),
        "{out}"
    );
    assert!(out.contains("1 set aside by the triage"), "{out}");
    assert!(
        out.contains("kept after the noise check: the api Deployment really has no limits"),
        "{out}"
    );
    let limits =
        std::fs::read_to_string(dir.join(".tinstar/laws/pending/containers-have-limits.yaml"))
            .unwrap();
    assert!(limits.contains("# Kept after the noise check:"), "{limits}");
    assert!(out.contains("✓ money-is-cents"), "{out}");

    // Nothing changed: no call at all (the replay has no answers left, so a call would fail).
    let (code, out) = tinstar_with(&dir, "compile-flow-replay.json", &["compile", "RULES.md"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("compiling 0 of 3 section(s)"), "{out}");
    assert!(
        out.contains("= unchanged since the last compile: 1 law(s)"),
        "{out}"
    );
    assert!(out.contains("0 input tokens"), "{out}");
    std::fs::remove_dir_all(&dir).unwrap();
}
