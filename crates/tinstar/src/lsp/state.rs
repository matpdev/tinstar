//! Everything the server knows. Only the main loop owns it, so there are no locks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::Sender;
use lsp_types::{Diagnostic as LspDiagnostic, Location};
use tinstar_core::{Lawbook, LoadError};

use super::diagnostics::Job;

pub struct Document {
    pub version: i32,
    pub text: Arc<str>,
}

pub struct State {
    /// The project root (the folder holding `.tinstar/`), once one is found.
    pub root: Option<PathBuf>,
    /// An immutable snapshot: jobs keep the one they started with, a reload swaps in a new one.
    pub book: Arc<Lawbook>,
    /// Bumped on every reload, so results computed with an older book are dropped.
    pub generation: u64,
    pub load_errors: Vec<LoadError>,
    /// Where each law's `source` (`AGENTS.md#money`) points, resolved once per reload.
    pub sources: HashMap<String, Location>,

    /// Open documents, by URI.
    pub docs: HashMap<String, Document>,
    /// Debounce: documents waiting to be checked, and when.
    pub due: HashMap<String, Instant>,
    /// Last results for open documents: (text hash, generation) → diagnostics, to skip rechecks.
    pub memo: HashMap<String, (u64, u64, Vec<tinstar_core::Diagnostic>)>,

    /// What is shown, by source: per-file laws on open documents, per-file laws on files as saved
    /// (shown while they are closed), layout laws, and problems in law files.
    pub file_diags: HashMap<String, Vec<LspDiagnostic>>,
    pub disk_diags: HashMap<String, Vec<LspDiagnostic>>,
    pub layout_diags: HashMap<String, Vec<LspDiagnostic>>,
    pub law_diags: HashMap<String, Vec<LspDiagnostic>>,
    /// URIs with diagnostics on screen, so they can be cleared.
    pub published: HashSet<String>,

    pub jobs: Sender<Job>,
}

impl State {
    pub fn new(jobs: Sender<Job>) -> Self {
        State {
            root: None,
            book: Arc::new(Lawbook::from_laws(Path::new("."), Vec::new())),
            generation: 0,
            load_errors: Vec::new(),
            sources: HashMap::new(),
            docs: HashMap::new(),
            due: HashMap::new(),
            memo: HashMap::new(),
            file_diags: HashMap::new(),
            disk_diags: HashMap::new(),
            layout_diags: HashMap::new(),
            law_diags: HashMap::new(),
            published: HashSet::new(),
            jobs,
        }
    }

    /// A path relative to the root, if the path is inside it.
    pub fn relative(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(self.root.as_ref()?)
            .ok()
            .map(Path::to_path_buf)
    }
}
