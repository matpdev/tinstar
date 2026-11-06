//! `.tinstar/compiled.json`: what each section of a rules document gave the last time it was
//! compiled, and a fingerprint of its text. A section that hasn't changed isn't sent again, so
//! editing one paragraph of AGENTS.md costs one section, not the whole document.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::NotCheckable;

#[derive(Serialize, Deserialize, Default)]
pub struct Ledger {
    /// Document → section anchor → entry.
    #[serde(default)]
    pub documents: BTreeMap<String, BTreeMap<String, Entry>>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Entry {
    pub fingerprint: String,
    /// The laws it gave (ids, as written to pending/).
    pub laws: Vec<String>,
    pub not_checkable: Vec<NotCheckable>,
}

pub fn path(root: &Path) -> PathBuf {
    root.join(".tinstar/compiled.json")
}

impl Ledger {
    pub fn load(root: &Path) -> Ledger {
        std::fs::read_to_string(path(root))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        std::fs::write(path(root), serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }

    /// The entry for a section, if its text is the same as when it was compiled.
    pub fn unchanged(&self, doc: &str, anchor: &str, fingerprint: &str) -> Option<&Entry> {
        self.documents
            .get(doc)?
            .get(anchor)
            .filter(|e| e.fingerprint == fingerprint)
    }

    pub fn forget(&mut self, doc: &str, anchor: &str) {
        if let Some(d) = self.documents.get_mut(doc) {
            d.remove(anchor);
        }
    }

    pub fn record(&mut self, doc: &str, anchor: &str, entry: Entry) {
        self.documents
            .entry(doc.to_string())
            .or_default()
            .insert(anchor.to_string(), entry);
    }
}
