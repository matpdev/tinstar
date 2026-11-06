//! Method name → handler. Every LSP method tinstar answers is listed here, once.

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as _,
};
use lsp_types::request::{CodeActionRequest, Request as _};
use serde::de::DeserializeOwned;

use super::state::State;
use super::{actions, documents, watch};

pub fn request(_state: &mut State, conn: &Connection, req: Request) {
    let response = match req.method.as_str() {
        CodeActionRequest::METHOD => answer(&req, actions::code_action),
        _ => Response::new_err(
            req.id.clone(),
            ErrorCode::MethodNotFound as i32,
            format!("unknown method {}", req.method),
        ),
    };
    let _ = conn.sender.send(Message::Response(response));
}

pub fn notification(state: &mut State, conn: &Connection, n: Notification) {
    match n.method.as_str() {
        DidOpenTextDocument::METHOD => with(n, |p| documents::did_open(state, conn, p)),
        DidChangeTextDocument::METHOD => with(n, |p| documents::did_change(state, conn, p)),
        DidSaveTextDocument::METHOD => with(n, |p| documents::did_save(state, conn, p)),
        DidCloseTextDocument::METHOD => with(n, |p| documents::did_close(state, conn, p)),
        DidChangeWatchedFiles::METHOD => {
            with(n, |p| watch::did_change_watched_files(state, conn, p))
        }
        _ => {}
    }
}

fn answer<P: DeserializeOwned, R: serde::Serialize>(
    req: &Request,
    f: impl FnOnce(P) -> R,
) -> Response {
    match serde_json::from_value::<P>(req.params.clone()) {
        Ok(p) => Response::new_ok(req.id.clone(), f(p)),
        Err(e) => Response::new_err(
            req.id.clone(),
            ErrorCode::InvalidParams as i32,
            e.to_string(),
        ),
    }
}

fn with<P: DeserializeOwned>(n: Notification, f: impl FnOnce(P)) {
    if let Ok(p) = serde_json::from_value::<P>(n.params) {
        f(p);
    }
}
