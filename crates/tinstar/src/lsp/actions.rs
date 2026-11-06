//! Quick fixes. A diagnostic whose law has a `fix` carries the edit in `data`, so offering it
//! needs no new check.

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, TextEdit, WorkspaceEdit,
};

use super::convert::SOURCE;

pub fn code_action(p: CodeActionParams) -> Vec<CodeActionOrCommand> {
    p.context
        .diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some(SOURCE))
        .filter_map(|d| {
            let edit: TextEdit = serde_json::from_value(d.data.clone()?).ok()?;
            let law = match &d.code {
                Some(lsp_types::NumberOrString::String(s)) => s.clone(),
                _ => SOURCE.into(),
            };
            // lsp-types keys WorkspaceEdit by Uri, which clippy flags as a mutable key; it isn't mutated.
            #[allow(clippy::mutable_key_type)]
            let changes = HashMap::from([(p.text_document.uri.clone(), vec![edit])]);
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Fix: {law}"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![d.clone()]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                is_preferred: Some(true),
                ..Default::default()
            }))
        })
        .collect()
}
