//! Scheduling checks, running them on workers, and publishing what they find.
//!
//! The main loop never checks anything itself: it sends [`Job`]s to a pool of worker threads and
//! gets [`Done`] messages back. Every job carries the document version and the lawbook generation
//! it was made for; a result that no longer matches is dropped ("latest wins").

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Connection, Message, Notification};
use lsp_types::notification::{Notification as _, PublishDiagnostics};
use lsp_types::{Diagnostic as LspDiagnostic, PublishDiagnosticsParams};
use tinstar_core::{Diagnostic, Lawbook, walk_files};

use super::convert;
use super::state::State;
use super::uri;

/// How long to wait after a keystroke before checking.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

pub enum Job {
    /// Per-file laws on a document's text.
    File {
        uri: String,
        version: i32,
        rel: PathBuf,
        text: Arc<str>,
        book: Arc<Lawbook>,
        generation: u64,
    },
    /// Layout laws on the whole project (walks the files).
    Layout { book: Arc<Lawbook>, generation: u64 },
}

pub enum Done {
    File {
        uri: String,
        version: i32,
        generation: u64,
        text_hash: u64,
        diagnostics: Vec<Diagnostic>,
    },
    Layout {
        generation: u64,
        files: Vec<PathBuf>,
        diagnostics: Vec<Diagnostic>,
    },
}

/// Starts the worker pool. Workers stop when the job channel closes.
pub fn spawn_workers(jobs: Receiver<Job>, done: Sender<Done>) {
    let n = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(2));
    for i in 0..n {
        let (jobs, done) = (jobs.clone(), done.clone());
        std::thread::Builder::new()
            .name(format!("tinstar-check-{i}"))
            .spawn(move || {
                for job in jobs {
                    let result = match job {
                        Job::File {
                            uri,
                            version,
                            rel,
                            text,
                            book,
                            generation,
                        } => Done::File {
                            text_hash: hash(&text),
                            diagnostics: book.check_source(&rel, &text),
                            uri,
                            version,
                            generation,
                        },
                        Job::Layout { book, generation } => {
                            let files = walk_files(&book.root);
                            let diagnostics = book.check_layout(&files);
                            Done::Layout {
                                generation,
                                files,
                                diagnostics,
                            }
                        }
                    };
                    if done.send(result).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn a worker thread");
    }
}

pub fn hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Marks a document to be checked after `delay` (zero: on the next turn of the loop).
pub fn schedule(state: &mut State, uri: &str, delay: Duration) {
    state.due.insert(uri.to_string(), Instant::now() + delay);
}

/// How long the loop may sleep before a scheduled check is due.
pub fn next_deadline(state: &State) -> Option<Duration> {
    state
        .due
        .values()
        .min()
        .map(|t| t.saturating_duration_since(Instant::now()))
}

/// Sends every due document to the workers (or answers from the memo).
pub fn dispatch_due(state: &mut State, conn: &Connection) {
    let now = Instant::now();
    let due: Vec<String> = state
        .due
        .iter()
        .filter(|(_, t)| **t <= now)
        .map(|(u, _)| u.clone())
        .collect();
    for uri_s in due {
        state.due.remove(&uri_s);
        let Some(doc) = state.docs.get(&uri_s) else {
            continue;
        };
        let Some(path) = uri_s.parse().ok().as_ref().and_then(uri::to_path) else {
            continue;
        };
        let Some(rel) = state.relative(&path) else {
            continue;
        };
        let text_hash = hash(&doc.text);
        if let Some((h, g, cached)) = state.memo.get(&uri_s)
            && *h == text_hash
            && *g == state.generation
        {
            let cached = cached.clone();
            show_file(state, conn, &uri_s, &cached);
            continue;
        }
        let job = Job::File {
            uri: uri_s.clone(),
            version: doc.version,
            rel,
            text: doc.text.clone(),
            book: state.book.clone(),
            generation: state.generation,
        };
        let _ = state.jobs.send(job);
    }
}

pub fn request_layout(state: &State) {
    if state.root.is_some() {
        let _ = state.jobs.send(Job::Layout {
            book: state.book.clone(),
            generation: state.generation,
        });
    }
}

/// A worker finished.
pub fn on_done(state: &mut State, conn: &Connection, done: Done) {
    match done {
        Done::File {
            uri,
            version,
            generation,
            text_hash,
            diagnostics,
        } => {
            let current = state.docs.get(&uri).is_some_and(|d| d.version == version);
            if !current || generation != state.generation {
                return; // stale: a newer check is coming
            }
            state
                .memo
                .insert(uri.clone(), (text_hash, generation, diagnostics.clone()));
            show_file(state, conn, &uri, &diagnostics);
        }
        Done::Layout {
            generation,
            files,
            diagnostics,
        } => {
            if generation != state.generation {
                return;
            }
            show_layout(state, conn, &files, &diagnostics);
        }
    }
}

fn show_file(state: &mut State, conn: &Connection, uri: &str, diagnostics: &[Diagnostic]) {
    let text = state.docs.get(uri).map(|d| d.text.clone());
    let lsp: Vec<LspDiagnostic> = diagnostics
        .iter()
        .map(|d| convert::diagnostic(d, text.as_deref(), &state.book, &state.sources))
        .collect();
    state.file_diags.insert(uri.to_string(), lsp);
    publish(state, conn, uri);
}

/// Layout diagnostics go on the offending file, or, for a folder, on its first file (usually
/// its index), so they show up where people work.
fn show_layout(
    state: &mut State,
    conn: &Connection,
    files: &[PathBuf],
    diagnostics: &[Diagnostic],
) {
    let Some(root) = state.root.clone() else {
        return;
    };
    let mut by_uri: HashMap<String, Vec<LspDiagnostic>> = HashMap::new();
    for d in diagnostics {
        let target = if root.join(&d.path).is_dir() {
            files
                .iter()
                .find(|f| f.starts_with(&d.path))
                .cloned()
                .unwrap_or_else(|| d.path.clone())
        } else {
            d.path.clone()
        };
        let u = convert::uri_of(&root, &target).as_str().to_string();
        by_uri.entry(u).or_default().push(convert::diagnostic(
            d,
            None,
            &state.book,
            &state.sources,
        ));
    }
    let touched: HashSet<String> = state
        .layout_diags
        .keys()
        .chain(by_uri.keys())
        .cloned()
        .collect();
    state.layout_diags = by_uri;
    for u in touched {
        publish(state, conn, &u);
    }
}

/// Sends what is known for a URI: per-file, layout and law-file diagnostics together.
pub fn publish(state: &mut State, conn: &Connection, uri_s: &str) {
    let mut all: Vec<LspDiagnostic> = Vec::new();
    for map in [&state.file_diags, &state.layout_diags, &state.law_diags] {
        if let Some(d) = map.get(uri_s) {
            all.extend(d.iter().cloned());
        }
    }
    if all.is_empty() && !state.published.contains(uri_s) {
        return;
    }
    if all.is_empty() {
        state.published.remove(uri_s);
    } else {
        state.published.insert(uri_s.to_string());
    }
    let Ok(uri) = uri_s.parse() else { return };
    let version = state.docs.get(uri_s).map(|d| d.version);
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics: all,
        version,
    };
    let _ = conn.sender.send(Message::Notification(Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        params,
    )));
}
