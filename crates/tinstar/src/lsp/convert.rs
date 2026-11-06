//! tinstar diagnostics → LSP diagnostics. Every protocol detail (UTF-16 columns, severities, codes,
//! links) lives here.

use std::collections::HashMap;
use std::path::Path;

use lsp_types::{
    CodeDescription, Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    Location, NumberOrString, Position, Range, TextEdit, Uri,
};
use tinstar_core::{Diagnostic, Lawbook, LineIndex, Severity};

use super::uri;

pub const SOURCE: &str = "tinstar";

pub fn severity(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

pub fn range(lines: &LineIndex, start: usize, end: usize) -> Range {
    let (l1, c1) = lines.line_col_utf16(start);
    let (l2, c2) = lines.line_col_utf16(end);
    Range::new(
        Position::new(l1 as u32, c1 as u32),
        Position::new(l2 as u32, c2 as u32),
    )
}

fn top() -> Range {
    Range::new(Position::new(0, 0), Position::new(0, 0))
}

/// `text` is the file's contents when the diagnostic has a span (needed for UTF-16 columns and
/// for the fix's range).
pub fn diagnostic(
    d: &Diagnostic,
    text: Option<&str>,
    book: &Lawbook,
    sources: &HashMap<String, Location>,
) -> LspDiagnostic {
    let lines = text.map(LineIndex::new);
    let at = match (d.span, &lines) {
        (Some(s), Some(lines)) => range(lines, s.start, s.end),
        _ => top(),
    };
    let mut message = d.message.clone();
    if d.span.is_none() && book.root.join(&d.path).is_dir() {
        message.push_str(&format!(" in {}/", d.path.display()));
    }
    if let Some(h) = &d.hint {
        message.push_str(&format!("\nhelp: {h}"));
    }
    // Editors (Zed among them) don't show relatedInformation on hover, so the law itself goes in
    // the message too: it is what tells people and agents *why*.
    if let Some(s) = &d.statement {
        let src = d.source.as_deref().map(|s| format!(" ({s})")).unwrap_or_default();
        message.push_str(&format!("\nlaw: \"{s}\"{src}"));
    }
    let law_file = book
        .law(&d.law)
        .and_then(|l| l.path.as_ref())
        .map(|p| uri::from_path(&book.root.join(p)));
    let related = d.statement.as_ref().map(|statement| {
        let location = d
            .source
            .as_ref()
            .and_then(|s| sources.get(s).cloned())
            .or_else(|| law_file.clone().map(|u| Location::new(u, top())));
        location.map(|location| {
            vec![DiagnosticRelatedInformation {
                location,
                message: format!("law: {statement}"),
            }]
        })
    });
    let data = match (&d.fix, &lines) {
        (Some(fix), Some(lines)) => Some(
            serde_json::to_value(TextEdit::new(
                range(lines, fix.start, fix.end),
                fix.replacement.clone(),
            ))
            .expect("a text edit serializes"),
        ),
        _ => None,
    };
    LspDiagnostic {
        range: at,
        severity: Some(severity(d.severity)),
        code: Some(NumberOrString::String(d.law.clone())),
        code_description: law_file.map(|href| CodeDescription { href }),
        source: Some(SOURCE.into()),
        message,
        related_information: related.flatten(),
        tags: None,
        data,
    }
}

/// Resolves every law's `source` (`AGENTS.md#money`) to the heading it names.
pub fn resolve_sources(book: &Lawbook) -> HashMap<String, Location> {
    let mut out = HashMap::new();
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    for law in &book.laws {
        let Some(source) = &law.source else { continue };
        if out.contains_key(source) {
            continue;
        }
        let (file, anchor) = source.split_once('#').unwrap_or((source.as_str(), ""));
        let path = book.root.join(file);
        if !path.is_file() {
            continue;
        }
        let text = files
            .entry(file.to_string())
            .or_insert_with(|| std::fs::read_to_string(&path).ok())
            .as_deref()
            .unwrap_or("");
        let line = if anchor.is_empty() {
            Some(0)
        } else {
            heading_line(text, anchor)
        };
        let pos = Position::new(line.unwrap_or(0) as u32, 0);
        out.insert(
            source.clone(),
            Location::new(uri::from_path(&path), Range::new(pos, pos)),
        );
    }
    out
}

/// The line of the Markdown heading whose GitHub-style anchor is `anchor`.
fn heading_line(text: &str, anchor: &str) -> Option<usize> {
    text.lines().position(|line| {
        let Some(title) = line.trim_start().strip_prefix('#') else {
            return false;
        };
        let slug: String = title
            .trim_start_matches('#')
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|c| match c {
                ' ' => Some('-'),
                c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
                _ => None,
            })
            .collect();
        slug == anchor.to_lowercase()
    })
}

/// A diagnostic on a law file (it doesn't load, or fails its own examples).
pub fn law_problem(
    text: Option<&str>,
    line_col: Option<(usize, usize)>,
    message: String,
) -> LspDiagnostic {
    let range = match (text, line_col) {
        (Some(_), Some((line, col))) => {
            let p = Position::new(line.saturating_sub(1) as u32, col.saturating_sub(1) as u32);
            Range::new(p, Position::new(p.line, p.character + 1))
        }
        _ => top(),
    };
    LspDiagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some(SOURCE.into()),
        message,
        ..Default::default()
    }
}

/// The line of `key:` at the start of a line in a YAML text (0-based), for pointing at sections.
pub fn yaml_key_line(text: &str, key: &str) -> Option<usize> {
    text.lines().position(|l| l.starts_with(&format!("{key}:")))
}

pub fn uri_of(root: &Path, rel: &Path) -> Uri {
    uri::from_path(&root.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_anchors() {
        let md = "# Shop\n\n## Money\ntext\n## Things a checker can't see\n";
        assert_eq!(heading_line(md, "money"), Some(2));
        assert_eq!(heading_line(md, "things-a-checker-cant-see"), Some(4));
        assert_eq!(heading_line(md, "nope"), None);
    }
}
