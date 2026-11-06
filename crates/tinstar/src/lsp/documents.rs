//! Open, edited, saved and closed documents.

use std::sync::Arc;
use std::time::Duration;

use lsp_server::Connection;
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams,
};
use tinstar_core::find_root;

use super::diagnostics::{self, DEBOUNCE};
use super::state::{Document, State};
use super::{uri, watch};

pub fn did_open(state: &mut State, conn: &Connection, p: DidOpenTextDocumentParams) {
    let key = p.text_document.uri.as_str().to_string();
    if state.root.is_none()
        && let Some(path) = uri::to_path(&p.text_document.uri)
        && let Some(root) = path.parent().and_then(find_root)
    {
        watch::set_root(state, conn, root);
    }
    state.docs.insert(
        key.clone(),
        Document {
            version: p.text_document.version,
            text: Arc::from(p.text_document.text),
        },
    );
    after_edit(state, conn, &key, Duration::ZERO);
}

pub fn did_change(state: &mut State, conn: &Connection, p: DidChangeTextDocumentParams) {
    let key = p.text_document.uri.as_str().to_string();
    // Full sync: the last change holds the whole text.
    let Some(change) = p.content_changes.into_iter().last() else {
        return;
    };
    state.docs.insert(
        key.clone(),
        Document {
            version: p.text_document.version,
            text: Arc::from(change.text),
        },
    );
    after_edit(state, conn, &key, DEBOUNCE);
}

pub fn did_save(state: &mut State, conn: &Connection, p: DidSaveTextDocumentParams) {
    if watch::is_law_file(state, &p.text_document.uri) {
        watch::reload(state, conn);
        return;
    }
    let key = p.text_document.uri.as_str().to_string();
    diagnostics::schedule(state, &key, Duration::ZERO);
    // A saved file may be new, renamed or the last one a folder was missing.
    diagnostics::request_project(state);
}

pub fn did_close(state: &mut State, conn: &Connection, p: DidCloseTextDocumentParams) {
    let key = p.text_document.uri.as_str().to_string();
    state.docs.remove(&key);
    state.due.remove(&key);
    state.memo.remove(&key);
    state.file_diags.remove(&key);
    diagnostics::publish(state, conn, &key);
}

fn after_edit(state: &mut State, conn: &Connection, key: &str, delay: Duration) {
    let Ok(u) = key.parse() else { return };
    if watch::is_law_file(state, &u) {
        // A law being written: show at once whether it loads and passes its examples.
        watch::check_law_text(state, conn, key);
    } else {
        diagnostics::schedule(state, key, delay);
    }
}
