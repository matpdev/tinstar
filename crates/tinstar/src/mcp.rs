//! `tinstar mcp`: a Model Context Protocol server (stdio), for editors and agents that bring their
//! own model (Zed, VS Code with Copilot, Cursor, Claude Code…).
//!
//! `tinstar compile` calls an LLM itself, with a key. Here it is the other way around: the agent's
//! model does the thinking, and tinstar does what it does without an LLM. It gives the rules
//! document and the law format, and checks every law the agent writes against its examples and on
//! the project (the noise check). Nothing reaches `.tinstar/laws/pending/` without passing those
//! checks, whichever model wrote it. `tinstar_check` runs the approved laws, so any agent can check
//! its own work.
//!
//! Messages are JSON-RPC 2.0, one per line on stdin/stdout. Logs go to stderr.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};
use tinstar_core::{LAWS_DIR, Lawbook, find_root, walk_files};

use crate::compile::{self, noise, sections};
use crate::output;

/// Protocol versions this server speaks; the client's choice is echoed when it is one of them.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Above this many hits in the project, a law needs narrowing or a reason (as in `compile`).
const NOISE_LIMIT: usize = 25;

/// The most problems `tinstar_check` lists, so a big project doesn't flood the agent's context.
const MAX_PROBLEMS: usize = 100;

const INSTRUCTIONS: &str = "tinstar turns a project's rules (AGENTS.md) into laws: small checks that \
run without an LLM, in editors, CI and agents' loops. To draft laws, call tinstar_sections and follow \
its instructions; check each law with tinstar_check_law and save it with tinstar_save_law. To check \
code against the project's approved laws, call tinstar_check.";

/// What the agent does with the sections, in place of the "Answer" part of the law format, which
/// is written for `tinstar compile`.
const WORKFLOW: &str = "## How to work (read this first)\n\n\
1. Read each section below and find its rules. A rule a checker can't see in code, config or the \
file tree (process, intent, runtime values) stays in the document: just mention it in your summary.\n\
2. Write each checkable rule as one law, a JSON object with the fields described in the law format \
below. Use the section's `source` exactly as given.\n\
3. Call `tinstar_check_law` with it. It compiles the law, runs its examples, then runs it on the \
project. Fix what it reports and check again. After three failed tries on one law, give up on it and \
say why.\n\
4. If it says the law is noisy (it fires many times in the project), look at the sample: narrow it \
(`files`/`ignores`, a tighter rule), or, if the hits really break the rule, save it with \
`confirmed` explaining why, or give up on it.\n\
5. Save each passing law with `tinstar_save_law`. It goes to .tinstar/laws/pending/: nothing is \
enforced until a person reviews it and runs `tinstar approve`.\n\
6. End with a short summary: laws saved, rules left to the document, laws given up.\n\n\
In the law format below, ignore the parts about the answer's JSON shape, `not_checkable` and \
`confirmed` lists, and feedback rounds: with these tools you check and save laws one at a time.\n\n";

struct Server {
    /// Where the server started (a `.tinstar/` above it, or the folder itself).
    root: Option<PathBuf>,
    /// Who is calling, from `initialize`: written in the header of saved laws.
    client: String,
}

pub fn run(root: Option<PathBuf>) -> Result<()> {
    let root = root.or_else(|| {
        let cwd = std::env::current_dir().ok()?;
        Some(find_root(&cwd).unwrap_or(cwd))
    });
    let mut server = Server {
        root,
        client: "an MCP client".into(),
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                let reply = error(Value::Null, -32700, &format!("parse error: {e}"));
                writeln!(stdout, "{reply}")?;
                stdout.flush()?;
                continue;
            }
        };
        // Notifications (no id) need no answer.
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let method = msg["method"].as_str().unwrap_or_default();
        let reply = match server.handle(method, &msg["params"]) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error(id, code, &message),
        };
        writeln!(stdout, "{reply}")?;
        stdout.flush()?;
    }
    Ok(())
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

impl Server {
    fn handle(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                if let Some(name) = params["clientInfo"]["name"].as_str() {
                    self.client = name.to_string();
                }
                let asked = params["protocolVersion"].as_str().unwrap_or_default();
                let version = VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .unwrap_or(&VERSIONS[1]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "tinstar", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default();
                let args = &params["arguments"];
                let result = match name {
                    "tinstar_sections" => self.sections(args),
                    "tinstar_check_law" => self.check_law(args),
                    "tinstar_save_law" => self.save_law(args),
                    "tinstar_check" => self.check(args),
                    _ => return Err((-32602, format!("unknown tool `{name}`"))),
                };
                // A tool that can't do its job says so in its result, for the model to read.
                Ok(match result {
                    Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                    Err(text) => {
                        json!({ "content": [{ "type": "text", "text": text }], "isError": true })
                    }
                })
            }
            _ => Err((-32601, format!("method not found: {method}"))),
        }
    }

    /// The project a call is about: its `project` argument, or where the server started.
    fn root(&self, args: &Value) -> Result<PathBuf, String> {
        match args["project"].as_str() {
            Some(p) => {
                let p = PathBuf::from(p);
                if !p.is_dir() {
                    return Err(format!("no such folder: {}", p.display()));
                }
                Ok(find_root(&p).unwrap_or(p))
            }
            None => self
                .root
                .clone()
                .ok_or_else(|| "no project folder: pass `project` (an absolute path)".into()),
        }
    }

    fn sections(&self, args: &Value) -> Result<String, String> {
        let root = self.root(args)?;
        let document = args["document"].as_str().unwrap_or("AGENTS.md");
        let markdown = std::fs::read_to_string(root.join(document))
            .map_err(|e| format!("can't read {document} in {}: {e}", root.display()))?;
        let files = walk_files(&root);
        let mut out = String::from(WORKFLOW);
        out.push_str("## The law format\n\n");
        out.push_str(&compile::system_prompt());
        out.push_str("\n\n## The project\n\n");
        out.push_str(&sections::overview(&files));
        out.push_str(&format!("\n## The sections of {document}\n"));
        for s in sections::split(&markdown) {
            out.push_str(&format!(
                "\n### {} (source: `{document}#{}`)\n\n{}",
                s.heading, s.anchor, s.text
            ));
            let mentioned = sections::mentioned(&root, &files, &s.text);
            if !mentioned.is_empty() {
                out.push_str(&format!("\nFiles it mentions:\n{mentioned}"));
            }
        }
        Ok(out)
    }

    /// Compiles a law, runs its examples and runs it on the project.
    fn check_law(&self, args: &Value) -> Result<String, String> {
        let root = self.root(args)?;
        let (yaml, source) = law_args(args)?;
        let files = walk_files(&root);
        let (law, hits) = match compile::validate(&root, &files, &yaml, &source) {
            Ok(v) => v,
            Err(e) => return Ok(format!("FAILED: the law {e}\nFix it and check again.")),
        };
        let mut out = format!(
            "The law `{}` passes its {} example(s) and fires {} time(s) in the project.\n",
            law.id, law.examples, law.hits
        );
        for w in &law.warnings {
            out.push_str(&format!("Warning: {w}\n"));
        }
        if let Some(tip) = fix_tip(&yaml) {
            out.push_str(tip);
        }
        if law.hits > NOISE_LIMIT {
            out.push_str(&format!(
                "\nNOISY: it fires more than {NOISE_LIMIT} times.\n{}\n\n{}",
                noise::ASK,
                noise::report(&law.id, &hits, &root)
            ));
            out.push_str(
                "\nCheck a narrower version, or save it with `confirmed` if the hits are real.",
            );
        } else {
            out.push_str("\nOK: save it with tinstar_save_law.");
        }
        Ok(out)
    }

    /// Checks a law again and writes it to `.tinstar/laws/pending/` if it passes.
    fn save_law(&self, args: &Value) -> Result<String, String> {
        let root = self.root(args)?;
        let (yaml, source) = law_args(args)?;
        let files = walk_files(&root);
        let (law, _) = compile::validate(&root, &files, &yaml, &source)
            .map_err(|e| format!("not saved: the law {e}"))?;
        let confirmed = args["confirmed"]
            .as_str()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        if law.hits > NOISE_LIMIT && confirmed.is_none() {
            return Err(format!(
                "not saved: it fires {} times in the project. Narrow it (check it again with \
                 tinstar_check_law), or pass `confirmed` with why these hits really break the rule.",
                law.hits
            ));
        }
        let (book, _) = Lawbook::load(&root);
        if book.laws.iter().any(|l| l.id == law.id) {
            return Err(format!(
                "not saved: the project already has an approved law `{}`; choose another id",
                law.id
            ));
        }
        let pending = root.join(LAWS_DIR).join("pending");
        std::fs::create_dir_all(&pending).map_err(|e| e.to_string())?;
        let mut header = format!(
            "# Drafted through `tinstar mcp` by {} from {source}.\n\
             # It passed its {} example(s) and fires {} time(s) in the project today.\n",
            self.client, law.examples, law.hits
        );
        if let Some(why) = confirmed {
            header.push_str(&format!("# Kept after the noise check: {why}\n"));
        }
        for w in &law.warnings {
            header.push_str(&format!("# Warning: {w}\n"));
        }
        header.push_str(&format!("# Review it, then: tinstar approve {}\n", law.id));
        let path = pending.join(format!("{}.yaml", law.id));
        let replaced = path.exists();
        std::fs::write(&path, format!("{header}{}\n", law.yaml.trim_end()))
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Saved {}{}. Nothing is enforced until a person reviews it and runs `tinstar approve {}`.",
            path.strip_prefix(&root).unwrap_or(&path).display(),
            if replaced {
                " (replacing the earlier draft)"
            } else {
                ""
            },
            law.id
        ))
    }

    /// Runs the approved laws on the project, or on some paths.
    fn check(&self, args: &Value) -> Result<String, String> {
        let root = self.root(args)?;
        let (book, errors) = Lawbook::load(&root);
        let targets: Option<Vec<PathBuf>> = args["paths"].as_array().map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|p| relative(&root, Path::new(p)))
                .collect()
        });
        let found = book.check_project(targets.as_deref());
        let mut out = String::new();
        for e in &errors {
            out.push_str(&format!("law file error: {e}\n"));
        }
        if book.laws.is_empty() {
            out.push_str("This project has no approved laws yet (.tinstar/laws/).");
            return Ok(out);
        }
        if found.is_empty() {
            out.push_str(&format!(
                "No law broken ({} law(s) checked).",
                book.laws.len()
            ));
            return Ok(out);
        }
        out.push_str(&format!("{} problem(s):\n", found.len()));
        for d in found.iter().take(MAX_PROBLEMS) {
            out.push_str(&output::agent_line(d));
            out.push('\n');
        }
        if found.len() > MAX_PROBLEMS {
            out.push_str(&format!(
                "…and {} more; pass `paths` to look at fewer files.\n",
                found.len() - MAX_PROBLEMS
            ));
        }
        Ok(out)
    }
}

/// A path relative to the root (agents often pass absolute paths).
fn relative(root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(root).unwrap_or(p).to_path_buf()
}

/// A pattern law without `fix`: editors can't offer a quick fix, so remind the agent once.
fn fix_tip(yaml: &str) -> Option<&'static str> {
    let law: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let pattern = law.get("kind").and_then(|k| k.as_str()) == Some("pattern");
    (pattern && law.get("fix").is_none()).then_some(
        "Tip: it has no `fix`. If the correct code is a mechanical rewrite of what it catches \
         (one call for another, e.g. `fix: logger.info($$$ARGS)`), add one: editors offer it as \
         a quick fix.\n",
    )
}

/// The law (a JSON object, or YAML text) and its source, from a tool call.
fn law_args(args: &Value) -> Result<(String, String), String> {
    let source = args["source"]
        .as_str()
        .ok_or("`source` is required, as given by tinstar_sections (`AGENTS.md#anchor`)")?
        .to_string();
    let yaml = match &args["law"] {
        Value::String(text) => text.clone(),
        law @ Value::Object(_) => compile::draft_yaml(law)?,
        _ => return Err("`law` is required: one law as a JSON object".into()),
    };
    Ok((yaml, source))
}

fn tools() -> Value {
    let project = json!({
        "type": "string",
        "description": "Absolute path of the project you are working in. Pass it unless the server was started inside the project."
    });
    let law = json!({
        "type": "object",
        "description": "One law as a JSON object, with the fields of the law format from tinstar_sections."
    });
    let source = json!({
        "type": "string",
        "description": "Where the rule lives, as given by tinstar_sections: `AGENTS.md#anchor`."
    });
    json!([
        {
            "name": "tinstar_sections",
            "description": "Start here to turn a rules document into laws: returns how to work, the law format, a picture of the project, and the document's sections with their `source`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "document": { "type": "string", "description": "The rules document, relative to the project (default AGENTS.md)." },
                    "project": project
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "tinstar_check_law",
            "description": "Compiles a drafted law, runs it against its own examples, then on the whole project. Says what to fix, or, if it fires too often, shows a sample of its hits. Saves nothing.",
            "inputSchema": {
                "type": "object",
                "properties": { "law": law, "source": source, "project": project },
                "required": ["law", "source"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "tinstar_save_law",
            "description": "Checks a law again and, if it passes, writes it to .tinstar/laws/pending/ for a person to review and approve. A law that fires often needs `confirmed`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "law": law,
                    "source": source,
                    "confirmed": { "type": "string", "description": "Only for a noisy law: why its hits in the project really break the rule." },
                    "project": project
                },
                "required": ["law", "source"]
            }
        },
        {
            "name": "tinstar_check",
            "description": "Checks code against the project's approved laws, without an LLM. Use it after editing files: it lists each broken law with the place, how to fix it and the rule it comes from.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Files or folders to check (default: the whole project)." },
                    "project": project
                }
            },
            "annotations": { "readOnlyHint": true }
        }
    ])
}
