//! A rules document split into its sections, and the picture of the project the LLM gets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Section {
    /// GitHub-style anchor of the heading: `money`, `things-a-checker-cant-see`.
    pub anchor: String,
    pub heading: String,
    /// The section's text, heading included.
    pub text: String,
}

/// Splits Markdown at its `#`, `##` and `###` headings. Text before the first heading is skipped
/// (it is usually an introduction); empty sections too.
pub fn split(markdown: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut in_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        let heading = (!in_code)
            .then(|| line.strip_prefix('#'))
            .flatten()
            .map(|rest| rest.trim_start_matches('#'))
            .filter(|rest| rest.starts_with(' '))
            .map(str::trim);
        match heading {
            Some(h) => sections.push(Section {
                anchor: anchor(h),
                heading: h.to_string(),
                text: format!("{line}\n"),
            }),
            None => {
                if let Some(s) = sections.last_mut() {
                    s.text.push_str(line);
                    s.text.push('\n');
                }
            }
        }
    }
    sections.retain(|s| s.text.lines().skip(1).any(|l| !l.trim().is_empty()));
    sections
}

/// The anchor GitHub gives a heading.
pub fn anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// What the LLM sees of the project: languages, paths, and the files a section names.
pub fn project_context(root: &Path, files: &[PathBuf], section: &str) -> String {
    let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
    for f in files {
        let ext = f
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                f.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        *by_ext.entry(ext).or_default() += 1;
    }
    let mut exts: Vec<(String, usize)> = by_ext.into_iter().collect();
    exts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut out = String::from("Files by extension: ");
    out.push_str(
        &exts
            .iter()
            .take(15)
            .map(|(e, n)| format!("{e} ({n})"))
            .collect::<Vec<_>>()
            .join(", "),
    );

    const MAX_PATHS: usize = 300;
    out.push_str(&format!(
        "\n\nPaths ({} files{}):\n",
        files.len(),
        if files.len() > MAX_PATHS {
            ", first 300"
        } else {
            ""
        }
    ));
    for f in files.iter().take(MAX_PATHS) {
        out.push_str(&format!("{}\n", f.display()));
    }

    // Files and folders the section mentions (`src/payments/`, `deploy/api.yaml`): a few of their
    // files, so examples look like this project's code.
    let mentioned: Vec<&PathBuf> = files
        .iter()
        .filter(|f| {
            f.ancestors().any(|a| {
                let s = a.to_string_lossy();
                !s.is_empty()
                    && s.contains('/')
                    && (section.contains(&format!("{s}/")) || section.contains(&format!("`{s}`")))
            }) || section.contains(&format!("`{}`", f.display()))
        })
        .take(4)
        .collect();
    for f in mentioned {
        let Ok(text) = std::fs::read_to_string(root.join(f)) else {
            continue;
        };
        let head: String = text.lines().take(80).collect::<Vec<_>>().join("\n");
        out.push_str(&format!("\n--- {} ---\n{head}\n", f.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_headings() {
        let md = "# Shop\nintro\n\n## Money\nCents only.\n\n## Empty\n\n```sh\n# not a heading\n```\n## Things a checker can't see\nx\n";
        let s = split(md);
        let anchors: Vec<&str> = s.iter().map(|s| s.anchor.as_str()).collect();
        assert_eq!(
            anchors,
            ["shop", "money", "empty", "things-a-checker-cant-see"]
        );
        assert!(s[2].text.contains("# not a heading"));
    }
}
