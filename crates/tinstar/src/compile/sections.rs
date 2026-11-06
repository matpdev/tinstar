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

/// What the LLM sees of the whole project: languages, folders and a sample of paths. It is the
/// same for every section of a run, so it goes first and providers can cache it.
pub fn overview(files: &[PathBuf]) -> String {
    let ext_of = |f: &Path| {
        f.extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                f.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
    };
    let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
    for f in files {
        *by_ext.entry(ext_of(f)).or_default() += 1;
    }
    let mut out = format!("{} files. By extension: {}\n", files.len(), top(by_ext, 15));

    // Folders, as deep as fits: three levels, fewer on big repositories.
    const MAX_FOLDERS: usize = 120;
    let mut folders: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for depth in (1..=3).rev() {
        folders.clear();
        for f in files {
            let parts: Vec<_> = f.parent().into_iter().flat_map(Path::components).collect();
            if parts.is_empty() {
                continue;
            }
            let dir: PathBuf = parts.iter().take(depth).collect();
            *folders
                .entry(format!("{}/", dir.display()))
                .or_default()
                .entry(ext_of(f))
                .or_default() += 1;
        }
        if folders.len() <= MAX_FOLDERS {
            break;
        }
    }
    let shown = folders.len().min(MAX_FOLDERS);
    out.push_str(&format!(
        "\nFolders ({shown} of {}), with their files by extension:\n",
        folders.len()
    ));
    for (dir, exts) in folders.into_iter().take(MAX_FOLDERS) {
        out.push_str(&format!("{dir} {}\n", top(exts, 4)));
    }

    // Real paths, spread over the tree, so globs and examples use the project's names.
    const SAMPLE: usize = 60;
    let step = (files.len() / SAMPLE).max(1);
    out.push_str("\nSome paths:\n");
    let root_files = files.iter().filter(|f| f.components().count() == 1);
    let spread = files.iter().step_by(step).take(SAMPLE);
    let mut seen = std::collections::BTreeSet::new();
    for f in root_files.chain(spread) {
        if seen.insert(f) {
            out.push_str(&format!("{}\n", f.display()));
        }
    }
    out
}

/// `rs (40), toml (2)`, biggest first.
fn top(counts: BTreeMap<String, usize>, n: usize) -> String {
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    v.iter()
        .take(n)
        .map(|(e, c)| format!("{e} ({c})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Files and folders a section mentions (`src/payments/`, `deploy/api.yaml`): the start of a few
/// of them, so examples look like this project's code.
pub fn mentioned(root: &Path, files: &[PathBuf], section: &str) -> String {
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
    let mut out = String::new();
    for f in mentioned {
        let Ok(text) = std::fs::read_to_string(root.join(f)) else {
            continue;
        };
        let head: String = text.lines().take(50).collect::<Vec<_>>().join("\n");
        out.push_str(&format!("--- {} ---\n{head}\n", f.display()));
    }
    out
}

/// A stable 64-bit hash (FNV-1a) of a section's text, to know whether it changed between runs.
pub fn fingerprint(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.trim().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
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
