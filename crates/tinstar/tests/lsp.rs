//! Talks to `tinstar lsp` the way an editor does, over stdio, on examples/shop.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
}

impl Client {
    fn start(root: &Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tinstar"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start tinstar lsp");
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, incoming) = channel();
        std::thread::spawn(move || {
            loop {
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length: ") {
                        len = n.parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                stdout.read_exact(&mut body).unwrap();
                if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut c = Client {
            child,
            stdin,
            incoming,
            next_id: 1,
        };
        let id = c.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": uri(root),
                "capabilities": { "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }
            }),
        );
        c.response(id);
        c.notify("initialized", json!({}));
        c
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Waits for a message matching `pred`, answering the server's own requests on the way.
    fn wait(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = self
                .incoming
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            if msg.get("method").is_some() && msg.get("id").is_some() {
                // e.g. client/registerCapability
                let id = msg["id"].clone();
                self.send(json!({ "jsonrpc": "2.0", "id": id, "result": null }));
            }
            if pred(&msg) {
                return msg;
            }
        }
    }

    fn response(&mut self, id: i64) -> Value {
        self.wait("a response", |m| {
            m["id"] == json!(id) && m.get("method").is_none()
        })
    }

    fn diagnostics_for(&mut self, file: &Path, pred: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let u = uri(file);
        let msg = self.wait(&format!("diagnostics for {}", file.display()), |m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == json!(u)
                && pred(m["params"]["diagnostics"].as_array().unwrap())
        });
        msg["params"]["diagnostics"].as_array().unwrap().clone()
    }

    fn open(&mut self, file: &Path, version: i64, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri(file), "languageId": "typescript", "version": version, "text": text } }),
        );
    }

    fn change(&mut self, file: &Path, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri(file), "version": version }, "contentChanges": [ { "text": text } ] }),
        );
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let id = self.request("shutdown", Value::Null);
        // Diagnostics may still be on their way: skip them until the answer to shutdown.
        while let Ok(m) = self.incoming.recv_timeout(Duration::from_secs(2)) {
            if m["id"] == json!(id) && m.get("method").is_none() {
                break;
            }
        }
        self.notify("exit", Value::Null);
        let _ = self.child.wait();
    }
}

fn shop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/shop")
        .canonicalize()
        .unwrap()
}

fn uri(p: &Path) -> String {
    format!("file://{}", p.display())
}

fn codes(d: &[Value]) -> Vec<String> {
    let mut c: Vec<String> = d
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect();
    c.sort();
    c
}

#[test]
fn diagnostics_fixes_and_edits() {
    let root = shop();
    let mut c = Client::start(&root);
    let cart = root.join("src/cart.ts");
    let text = std::fs::read_to_string(&cart).unwrap();

    c.open(&cart, 1, &text);
    let d = c.diagnostics_for(&cart, |d| d.len() == 2);
    assert_eq!(codes(&d), ["no-console-log", "no-float-money"]);
    let money = d.iter().find(|d| d["code"] == "no-float-money").unwrap();
    assert_eq!(
        money["range"]["start"],
        json!({ "line": 10, "character": 14 })
    );
    assert_eq!(money["severity"], 1);
    assert!(
        money["message"]
            .as_str()
            .unwrap()
            .contains("help: Type it as Money")
    );
    // The plain-language law, pointing at its heading in AGENTS.md.
    let related = &money["relatedInformation"][0];
    assert!(
        related["message"]
            .as_str()
            .unwrap()
            .starts_with("law: Money is always")
    );
    assert!(
        related["location"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/AGENTS.md")
    );
    assert_eq!(related["location"]["range"]["start"]["line"], 2);
    assert!(
        money["codeDescription"]["href"]
            .as_str()
            .unwrap()
            .ends_with("no-float-money.yaml")
    );

    // The console.log law has a fix: offered as a quick fix.
    let log = d
        .iter()
        .find(|d| d["code"] == "no-console-log")
        .unwrap()
        .clone();
    let id = c.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri(&cart) },
            "range": log["range"],
            "context": { "diagnostics": [log] }
        }),
    );
    let actions = c.response(id);
    let action = &actions["result"][0];
    assert_eq!(action["title"], "Fix: no-console-log");
    let edit = &action["edit"]["changes"][uri(&cart)][0];
    assert_eq!(edit["newText"], r#"logger.info("coupon", code, discount)"#);
    assert_eq!(
        edit["range"]["start"],
        json!({ "line": 11, "character": 2 })
    );

    // Typing: fix one problem, the other stays.
    let fixed = text.replace(
        r#"console.log("coupon", code, discount);"#,
        r#"logger.info("coupon", code);"#,
    );
    c.change(&cart, 2, &fixed);
    let d = c.diagnostics_for(&cart, |d| d.len() == 1);
    assert_eq!(codes(&d), ["no-float-money"]);
}

#[test]
fn layout_laws_without_opening_anything() {
    let root = shop();
    let mut c = Client::start(&root);
    // A folder missing its test: shown on the folder's first file.
    let d = c.diagnostics_for(&root.join("src/features/catalog/index.ts"), |d| {
        !d.is_empty()
    });
    assert_eq!(codes(&d), ["feature-has-test"]);
    assert!(
        d[0]["message"]
            .as_str()
            .unwrap()
            .contains("in src/features/catalog/")
    );
    let d = c.diagnostics_for(&root.join(".env"), |d| !d.is_empty());
    assert_eq!(codes(&d), ["no-committed-env"]);
}

#[test]
fn law_files_are_checked_as_they_are_typed() {
    let root = shop();
    let mut c = Client::start(&root);
    let law = root.join(".tinstar/laws/no-console-log.yaml");
    let text = std::fs::read_to_string(&law).unwrap();
    c.open(&law, 1, &text);

    // A typo in a field name: the law doesn't load, and the error points at the line.
    c.change(&law, 2, &text.replace("severity:", "severty:"));
    let d = c.diagnostics_for(&law, |d| !d.is_empty());
    assert!(
        d[0]["message"]
            .as_str()
            .unwrap()
            .contains("unknown field `severty`")
    );
    assert_eq!(d[0]["range"]["start"]["line"], 5); // `severity:` is line 6

    // A rule that no longer matches its own invalid example.
    c.change(
        &law,
        3,
        &text
            .replace("console.log($$$ARGS)", "console.warn($$$ARGS)")
            .replace("fix: logger.info($$$ARGS)\n", ""),
    );
    let d = c.diagnostics_for(&law, |d| {
        !d.is_empty() && d[0]["message"].as_str().unwrap().contains("example")
    });
    assert!(
        d[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("invalid example #1: the law did not fire")
    );

    // Back to the original: clean.
    c.change(&law, 4, &text);
    c.diagnostics_for(&law, |d| d.is_empty());
}
