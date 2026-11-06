//! The project root, the lawbook and its reloads, and law files being written.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lsp_server::{Connection, Message, Request, RequestId};
use lsp_types::request::{RegisterCapability, Request as _};
use lsp_types::{
    DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher,
    GlobPattern, Registration, RegistrationParams, Uri,
};
use tinstar_core::{LAWS_DIR, Law, Lawbook};

use super::convert;
use super::diagnostics;
use super::state::State;
use super::uri;

pub fn set_root(state: &mut State, conn: &Connection, root: PathBuf) {
    log(conn, &format!("tinstar: project root {}", root.display()));
    state.root = Some(root);
    reload(state, conn);
}

pub fn is_law_file(state: &State, u: &Uri) -> bool {
    uri::to_path(u)
        .and_then(|p| state.relative(&p))
        .is_some_and(|rel| is_law_path(&rel))
}

fn is_law_path(rel: &Path) -> bool {
    rel.starts_with(LAWS_DIR) && rel.extension().is_some_and(|e| e == "yaml" || e == "yml")
}

/// Loads the laws again and rechecks everything. The new book replaces the old one as a whole;
/// jobs already running finish with the old one and their results are dropped.
pub fn reload(state: &mut State, conn: &Connection) {
    let Some(root) = state.root.clone() else {
        return;
    };
    let (book, errors) = Lawbook::load(&root);
    state.generation += 1;
    state.sources = convert::resolve_sources(&book);
    state.book = Arc::new(book);
    state.load_errors = errors;
    state.memo.clear();

    // Problems in law files: ones that don't load, and ones that fail their own examples.
    let old: HashSet<String> = state.law_diags.keys().cloned().collect();
    state.law_diags.clear();
    for e in &state.load_errors {
        let key = convert::uri_of(&root, &e.path).as_str().to_string();
        let text = std::fs::read_to_string(root.join(&e.path)).ok();
        let d = convert::law_problem(
            text.as_deref(),
            e.error.location(),
            format!("law not loaded: {}", e.error),
        );
        state.law_diags.entry(key).or_default().push(d);
    }
    let book = state.book.clone();
    for law in &book.laws {
        let Some(rel) = &law.path else { continue };
        let key = convert::uri_of(&root, rel).as_str().to_string();
        let text = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
        let found = example_problems(law, &text);
        if !found.is_empty() {
            state.law_diags.entry(key).or_default().extend(found);
        }
    }
    let touched: HashSet<String> = old
        .into_iter()
        .chain(state.law_diags.keys().cloned())
        .collect();
    for key in touched {
        diagnostics::publish(state, conn, &key);
    }

    let open: Vec<String> = state.docs.keys().cloned().collect();
    for key in open {
        let Ok(u) = key.parse() else { continue };
        if !is_law_file(state, &u) {
            diagnostics::schedule(state, &key, Duration::ZERO);
        }
    }
    diagnostics::request_layout(state);
    log(
        conn,
        &format!(
            "tinstar: {} law(s) loaded, {} with errors",
            state.book.laws.len(),
            state.load_errors.len()
        ),
    );
}

/// A law file open in the editor, as it is being typed: does it load, and pass its examples?
pub fn check_law_text(state: &mut State, conn: &Connection, key: &str) {
    let Some(doc) = state.docs.get(key) else {
        return;
    };
    let text = doc.text.clone();
    let found = match Law::from_yaml(&text) {
        Err(e) => vec![convert::law_problem(
            Some(&text),
            e.location(),
            format!("law not loaded: {e}"),
        )],
        Ok(law) => example_problems(&law, &text),
    };
    state.law_diags.insert(key.to_string(), found);
    diagnostics::publish(state, conn, key);
}

fn example_problems(law: &Law, text: &str) -> Vec<lsp_types::Diagnostic> {
    let report = law.test_examples();
    let line = convert::yaml_key_line(text, "examples").map(|l| (l + 1, 1));
    report
        .failures
        .iter()
        .map(|f| {
            convert::law_problem(
                Some(text),
                line,
                format!(
                    "{} example #{}: {}\n{}",
                    f.which,
                    f.index + 1,
                    f.reason,
                    f.example.trim_end()
                ),
            )
        })
        .collect()
}

/// Asks the editor to tell us about file changes: law files (reload) and created or deleted files
/// (layout laws).
pub fn register_watchers(conn: &Connection) {
    let options = DidChangeWatchedFilesRegistrationOptions {
        watchers: vec![FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*".into()),
            kind: None,
        }],
    };
    let params = RegistrationParams {
        registrations: vec![Registration {
            id: "tinstar-watch".into(),
            method: "workspace/didChangeWatchedFiles".into(),
            register_options: Some(serde_json::to_value(options).expect("options serialize")),
        }],
    };
    let req = Request::new(
        RequestId::from("tinstar-watch".to_string()),
        RegisterCapability::METHOD.into(),
        params,
    );
    let _ = conn.sender.send(Message::Request(req));
}

pub fn did_change_watched_files(
    state: &mut State,
    conn: &Connection,
    p: DidChangeWatchedFilesParams,
) {
    let laws_changed = p.changes.iter().any(|c| is_law_file(state, &c.uri));
    let inside = p.changes.iter().any(|c| {
        uri::to_path(&c.uri)
            .and_then(|path| state.relative(&path))
            .is_some()
    });
    if laws_changed {
        reload(state, conn);
    } else if inside {
        diagnostics::request_layout(state);
    }
}

fn log(conn: &Connection, message: &str) {
    use lsp_types::notification::{LogMessage, Notification as _};
    let params = lsp_types::LogMessageParams {
        typ: lsp_types::MessageType::LOG,
        message: message.into(),
    };
    let _ = conn
        .sender
        .send(Message::Notification(lsp_server::Notification::new(
            LogMessage::METHOD.into(),
            params,
        )));
}
