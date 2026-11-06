//! `tinstar mcp` over stdio, as an editor or agent would use it: the agent drafts a law, tinstar
//! checks it and saves it only once it passes.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

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

fn shop() -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop");
    let dir = std::env::temp_dir().join(format!("tinstar-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&src, &dir);
    dir
}

/// Sends the requests, closes stdin, and returns the answers by id.
fn session(dir: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tinstar"))
        .arg("mcp")
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn call(id: i64, tool: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } })
}

fn text(answer: &Value) -> &str {
    answer["result"]["content"][0]["text"].as_str().unwrap()
}

/// The shop's console.log law, as the JSON object an agent would write, under a new id.
fn console_law(id: &str) -> Value {
    let yaml = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/shop/.tinstar/laws/no-console-log.yaml"),
    )
    .unwrap();
    let mut law: Value = serde_yaml::from_str(&yaml).unwrap();
    law["id"] = json!(id);
    sorted(law)
}

/// Keys sorted alphabetically, as some MCP clients send them.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = m.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[test]
fn drafts_checks_saves_and_checks_code() {
    let dir = shop();
    let mut broken = console_law("logs-through-logger");
    broken["examples"]["invalid"] = json!(["logger.info(\"fine\");"]);

    let answers = session(
        &dir,
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1" } } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            call(3, "tinstar_sections", json!({})),
            call(
                4,
                "tinstar_check_law",
                json!({ "law": broken, "source": "AGENTS.md#logging" }),
            ),
            call(
                5,
                "tinstar_check_law",
                json!({ "law": console_law("logs-through-logger"), "source": "AGENTS.md#logging" }),
            ),
            call(
                6,
                "tinstar_save_law",
                json!({ "law": console_law("logs-through-logger"), "source": "AGENTS.md#logging" }),
            ),
            call(
                7,
                "tinstar_save_law",
                json!({ "law": console_law("no-console-log"), "source": "AGENTS.md#logging" }),
            ),
            call(8, "tinstar_check", json!({ "paths": ["src/cart.ts"] })),
            json!({ "jsonrpc": "2.0", "id": 9, "method": "no/such" }),
        ],
    );
    let by_id = |id: i64| answers.iter().find(|a| a["id"] == json!(id)).unwrap();

    assert_eq!(by_id(1)["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(by_id(1)["result"]["serverInfo"]["name"], "tinstar");
    let tools: Vec<&str> = by_id(2)["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        [
            "tinstar_sections",
            "tinstar_check_law",
            "tinstar_save_law",
            "tinstar_check"
        ]
    );

    let sections = text(by_id(3));
    assert!(sections.starts_with("## How to work"), "{sections}");
    assert!(
        sections.contains("### Money (source: `AGENTS.md#money`)"),
        "{sections}"
    );
    assert!(sections.contains("## The project"), "{sections}");

    let failed = text(by_id(4));
    assert!(
        failed.starts_with("FAILED: the law gets its own examples wrong"),
        "{failed}"
    );

    let ok = text(by_id(5));
    assert!(
        ok.contains("passes its 2 example(s) and fires 1 time(s)"),
        "{ok}"
    );
    assert!(ok.contains("OK: save it"), "{ok}");

    assert!(text(by_id(6)).starts_with("Saved .tinstar/laws/pending/logs-through-logger.yaml"));
    let saved = std::fs::read_to_string(dir.join(".tinstar/laws/pending/logs-through-logger.yaml"))
        .unwrap();
    assert!(
        saved.starts_with("# Drafted through `tinstar mcp` by test-client from AGENTS.md#logging.")
    );
    assert!(saved.contains("\nid: logs-through-logger\n"), "{saved}");
    // Saved in reading order, whatever order the client sent (here: sorted, as some clients do).
    let id_at = saved.find("\nid:").unwrap();
    let examples_at = saved.find("\nexamples:").unwrap();
    assert!(id_at < examples_at, "{saved}");

    // An id the project already uses for an approved law is refused.
    assert_eq!(by_id(7)["result"]["isError"], true);
    assert!(text(by_id(7)).contains("already has an approved law `no-console-log`"));

    let check = text(by_id(8));
    assert!(check.starts_with("2 problem(s):"), "{check}");
    assert!(
        check.contains("src/cart.ts:11:15: error [no-float-money]"),
        "{check}"
    );

    assert_eq!(by_id(9)["error"]["code"], -32601);
    std::fs::remove_dir_all(&dir).unwrap();
}
