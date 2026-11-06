//! `tinstar hook <agent>`: runs after an AI agent edits a file and tells the agent which laws the
//! edit broke, so it fixes them before going on.
//!
//! Claude Code: a `PostToolUse` hook on `Edit|Write|MultiEdit`. It gets the tool call as JSON on
//! stdin; exiting with status 2 sends stderr back to the agent as feedback.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::ValueEnum;
use tinstar_core::{Lawbook, Severity, find_root};

use crate::output::agent_line;

#[derive(Clone, Copy, ValueEnum)]
pub enum Agent {
    ClaudeCode,
}

pub fn run(agent: Agent) -> Result<ExitCode> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let files = match agent {
        Agent::ClaudeCode => claude_code_files(&input),
    };
    // Never get in the agent's way because of tinstar itself: anything unexpected is a pass.
    let mut report = Vec::new();
    for file in files {
        report.extend(check(&file));
    }
    if report.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!(
        "tinstar: this edit breaks {} project law(s). Fix them before going on:",
        report.len()
    );
    for line in report {
        eprintln!("- {line}");
    }
    Ok(ExitCode::from(2))
}

/// The file the tool call wrote, from Claude Code's hook input.
fn claude_code_files(input: &str) -> Vec<PathBuf> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(input) else {
        return Vec::new();
    };
    let cwd = v["cwd"].as_str().map(PathBuf::from);
    let Some(path) = v["tool_input"]["file_path"].as_str() else {
        return Vec::new();
    };
    let path = PathBuf::from(path);
    vec![match (&cwd, path.is_absolute()) {
        (Some(cwd), false) => cwd.join(path),
        _ => path,
    }]
}

/// Errors and warnings of the laws that apply to `file` (hints and info are left out).
fn check(file: &Path) -> Vec<String> {
    let Ok(file) = file.canonicalize() else {
        return Vec::new();
    };
    let Some(root) = file.parent().and_then(find_root) else {
        return Vec::new();
    };
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let Ok(rel) = file.strip_prefix(&root) else {
        return Vec::new();
    };
    let (book, _errors) = Lawbook::load(&root);
    book.check_project(Some(&[rel.to_path_buf()]))
        .iter()
        .filter(|d| d.severity >= Severity::Warning)
        .map(agent_line)
        .collect()
}
