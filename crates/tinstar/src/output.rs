//! How diagnostics look in a terminal, in JSON and for agents.

use std::io::IsTerminal;

use tinstar_core::{Diagnostic, Kind, Lawbook, LineIndex, Severity};

use crate::Format;

struct Style {
    on: bool,
}

impl Style {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn severity(&self, s: Severity) -> String {
        let code = match s {
            Severity::Error => "1;31",
            Severity::Warning => "1;33",
            Severity::Info => "1;36",
            Severity::Hint => "1;32",
        };
        self.paint(code, s.as_str())
    }
}

fn style() -> Style {
    Style {
        on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    }
}

pub fn print(book: &Lawbook, diagnostics: &[Diagnostic], format: Format) {
    match format {
        Format::Json => {
            #[derive(serde::Serialize)]
            struct Report<'a> {
                diagnostics: &'a [Diagnostic],
            }
            let json = serde_json::to_string_pretty(&Report { diagnostics })
                .expect("diagnostics serialize");
            println!("{json}");
        }
        Format::Agent => {
            for d in diagnostics {
                println!("{}", agent_line(d));
            }
        }
        Format::Human => human(book, diagnostics),
    }
}

/// One line an agent can act on without reading anything else.
pub fn agent_line(d: &Diagnostic) -> String {
    let at = match d.span {
        Some(s) => format!("{}:{}:{}", d.path.display(), s.line, s.column),
        None => d.path.display().to_string(),
    };
    let mut line = format!("{at}: {} [{}] {}", d.severity.as_str(), d.law, d.message);
    if let Some(h) = &d.hint {
        line.push_str(&format!(" Fix: {h}"));
    }
    if let Some(s) = &d.statement {
        line.push_str(&format!(" Rule: \"{s}\""));
    }
    if let Some(src) = &d.source {
        line.push_str(&format!(" ({src})"));
    }
    line
}

fn human(book: &Lawbook, diagnostics: &[Diagnostic]) {
    let st = style();
    let mut cache: Option<(std::path::PathBuf, String)> = None;
    for d in diagnostics {
        println!(
            "{}{} {}",
            st.severity(d.severity),
            st.paint("1", &format!("[{}]", d.law)),
            st.paint("1", &d.message)
        );
        match d.span {
            Some(span) => {
                println!(
                    "  {} {}:{}:{}",
                    st.paint("34", "-->"),
                    d.path.display(),
                    span.line,
                    span.column
                );
                if cache.as_ref().is_none_or(|(p, _)| p != &d.path) {
                    let text = std::fs::read_to_string(book.root.join(&d.path)).unwrap_or_default();
                    cache = Some((d.path.clone(), text));
                }
                let text = &cache.as_ref().expect("just filled").1;
                if !text.is_empty() {
                    let lines = LineIndex::new(text);
                    let line = lines.line_text(span.line - 1);
                    let gutter = span.line.to_string();
                    let pad = " ".repeat(gutter.len());
                    let width = if span.end_line == span.line {
                        span.end_column.saturating_sub(span.column).max(1)
                    } else {
                        line.chars().count().saturating_sub(span.column - 1).max(1)
                    };
                    println!("  {pad} {}", st.paint("34", "|"));
                    println!(
                        "  {} {} {line}",
                        st.paint("34", &gutter),
                        st.paint("34", "|")
                    );
                    println!(
                        "  {pad} {} {}{}",
                        st.paint("34", "|"),
                        " ".repeat(span.column - 1),
                        st.paint(sev_code(d.severity), &"^".repeat(width))
                    );
                }
            }
            None => println!("  {} {}", st.paint("34", "-->"), d.path.display()),
        }
        if let Some(s) = &d.statement {
            let src = d
                .source
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default();
            println!("  {} law: \"{s}\"{src}", st.paint("34", "="));
        }
        if let Some(h) = &d.hint {
            println!("  {} help: {h}", st.paint("34", "="));
        }
        println!();
    }
    if diagnostics.is_empty() {
        println!(
            "{} {}, nothing broken",
            st.paint("1;32", "✓"),
            count(book.laws.len(), "law")
        );
    } else {
        // "3 errors, 1 warning, from 9 laws": each severity that occurs, most serious first.
        let mut parts = Vec::new();
        for (sev, word) in [
            (Severity::Error, "error"),
            (Severity::Warning, "warning"),
            (Severity::Info, "info"),
            (Severity::Hint, "hint"),
        ] {
            let n = diagnostics.iter().filter(|d| d.severity == sev).count();
            if n > 0 {
                parts.push(count(n, word));
            }
        }
        println!(
            "{} {}, from {}",
            st.paint("1;31", "✗"),
            parts.join(", "),
            count(book.laws.len(), "law")
        );
    }
}

fn sev_code(s: Severity) -> &'static str {
    match s {
        Severity::Error => "1;31",
        Severity::Warning => "1;33",
        Severity::Info => "1;36",
        Severity::Hint => "1;32",
    }
}

pub fn laws(book: &Lawbook) {
    let st = style();
    for law in &book.laws {
        let kind = match law.kind {
            Kind::Pattern => "pattern",
            Kind::Config => "config",
            Kind::Layout => "layout",
        };
        println!(
            "{} {} {}",
            st.paint("1", &law.id),
            st.paint("2", &format!("({kind}, {})", law.severity.as_str())),
            law.message
        );
        if let Some(s) = &law.statement {
            println!("    \"{s}\"");
        }
    }
    println!("{} law(s)", book.laws.len());
    let pending = book.root.join(tinstar_core::LAWS_DIR).join("pending");
    let n = std::fs::read_dir(&pending).map(|d| d.count()).unwrap_or(0);
    if n > 0 {
        println!(
            "{n} pending in {} (review, then `tinstar approve`)",
            pending
                .strip_prefix(&book.root)
                .unwrap_or(&pending)
                .display()
        );
    }
}

/// Returns whether every law passed its examples.
pub fn tests(book: &Lawbook) -> bool {
    let st = style();
    let mut all_ok = true;
    let mut untested = Vec::new();
    for law in &book.laws {
        if !law.has_examples() {
            untested.push(law.id.as_str());
            continue;
        }
        let r = law.test_examples();
        if r.failures.is_empty() {
            println!(
                "{} {} ({} example(s))",
                st.paint("1;32", "✓"),
                r.law,
                r.passed
            );
            continue;
        }
        all_ok = false;
        println!("{} {}", st.paint("1;31", "✗"), r.law);
        for f in &r.failures {
            println!("    {} example #{}: {}", f.which, f.index + 1, f.reason);
            for line in f.example.lines() {
                println!("      {}", st.paint("2", line));
            }
        }
    }
    if !untested.is_empty() {
        println!(
            "{} no examples: {}",
            st.paint("1;33", "!"),
            untested.join(", ")
        );
    }
    all_ok
}

/// `1 law`, `9 laws`.
fn count(n: usize, word: &str) -> String {
    match (n, word) {
        (1, _) => format!("1 {word}"),
        (_, "info") => format!("{n} info"),
        _ => format!("{n} {word}s"),
    }
}
