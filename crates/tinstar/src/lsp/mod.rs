//! `tinstar lsp`: a language server that shows broken laws as diagnostics, with quick fixes.
//!
//! One thread owns all the state and turns messages into actions (see `docs/architecture.md`):
//! LSP messages from the editor, results from the worker pool, and debounce deadlines.

mod actions;
mod convert;
mod diagnostics;
mod dispatch;
mod documents;
mod state;
mod uri;
mod watch;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{select, unbounded};
use lsp_server::{Connection, Message};
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionProviderCapability, InitializeParams,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions,
};
use tinstar_core::find_root;

use state::State;

pub fn run() -> Result<()> {
    let (conn, io) = Connection::stdio();
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
            ..Default::default()
        })),
        ..Default::default()
    };
    let params = conn.initialize(serde_json::to_value(capabilities)?)?;
    let params: InitializeParams = serde_json::from_value(params)?;

    let (job_tx, job_rx) = unbounded();
    let (done_tx, done_rx) = unbounded();
    diagnostics::spawn_workers(job_rx, done_tx);
    let mut state = State::new(job_tx);

    if let Some(root) = workspace_root(&params).as_deref().and_then(find_root) {
        watch::set_root(&mut state, &conn, root);
    }
    let can_watch = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.did_change_watched_files.as_ref())
        .and_then(|d| d.dynamic_registration)
        .unwrap_or(false);
    if can_watch {
        watch::register_watchers(&conn);
    }

    main_loop(&mut state, &conn, &done_rx)?;
    drop(conn);
    io.join()?;
    Ok(())
}

fn main_loop(
    state: &mut State,
    conn: &Connection,
    done: &crossbeam_channel::Receiver<diagnostics::Done>,
) -> Result<()> {
    loop {
        let wait = diagnostics::next_deadline(state).unwrap_or(Duration::from_secs(3600));
        select! {
            recv(conn.receiver) -> msg => match msg {
                Ok(Message::Request(req)) => {
                    if conn.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    dispatch::request(state, conn, req);
                }
                Ok(Message::Notification(n)) => dispatch::notification(state, conn, n),
                Ok(Message::Response(_)) => {}
                Err(_) => return Ok(()),
            },
            recv(done) -> d => if let Ok(d) = d {
                diagnostics::on_done(state, conn, d);
            },
            default(wait) => {}
        }
        diagnostics::dispatch_due(state, conn);
    }
}

#[allow(deprecated)] // root_uri: older clients only send that
fn workspace_root(p: &InitializeParams) -> Option<PathBuf> {
    p.workspace_folders
        .as_ref()
        .and_then(|f| f.first())
        .and_then(|f| uri::to_path(&f.uri))
        .or_else(|| p.root_uri.as_ref().and_then(uri::to_path))
}
