//! `kind: layout`: rules about files and folders rather than their contents. They run on the
//! list of the project's files (relative paths), so they need no parsing at all.
//!
//! - `forbid: true` — no file selected by `files` may exist (`**/.env`).
//! - `name_matches: <regex>` — every file selected by `files` has a name that matches.
//! - `each_dir: <glob>` + `require: [globs]` — every folder matching `each_dir` contains a file
//!   matching each of the `require` globs, relative to that folder (`**/*.test.ts` for any depth).
//!
//! Globs follow .gitignore: `*` stays inside one folder, `**` crosses folders.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use globset::GlobMatcher;
use regex::Regex;

use crate::law::{LawError, LawFile, glob, invalid};

pub(crate) enum LayoutCheck {
    Forbid,
    NameMatches(Regex),
    Require {
        each_dir: GlobMatcher,
        require: Vec<(String, GlobMatcher)>,
    },
}

/// A file or folder that breaks a layout law, and why.
pub(crate) struct LayoutHit {
    pub path: PathBuf,
    pub detail: String,
}

impl LayoutCheck {
    pub fn compile(file: &LawFile) -> Result<Self, LawError> {
        let modes = [
            file.forbid,
            file.name_matches.is_some(),
            file.each_dir.is_some(),
        ];
        if modes.iter().filter(|m| **m).count() != 1 {
            return invalid(
                "a layout law needs exactly one of `forbid: true`, `name_matches` or `each_dir`",
            );
        }
        if file.forbid {
            if file.files.is_empty() {
                return invalid("`forbid: true` needs `files`: the paths that must not exist");
            }
            return Ok(LayoutCheck::Forbid);
        }
        if let Some(re) = &file.name_matches {
            if file.files.is_empty() {
                return invalid("`name_matches` needs `files`: which files it names");
            }
            let re = Regex::new(re).or_else(|e| invalid(format!("bad regex `{re}`: {e}")))?;
            return Ok(LayoutCheck::NameMatches(re));
        }
        let each_dir = file.each_dir.as_deref().unwrap_or_default();
        if file.require.is_empty() {
            return invalid("`each_dir` needs `require`: the files every such folder must have");
        }
        Ok(LayoutCheck::Require {
            each_dir: glob(each_dir.trim_end_matches('/'))?.compile_matcher(),
            require: file
                .require
                .iter()
                .map(|r| Ok((r.clone(), glob(r)?.compile_matcher())))
                .collect::<Result<_, LawError>>()?,
        })
    }

    /// `selected` are the files the law's `files`/`ignores` pick; `all` is every project file.
    pub fn check(&self, selected: &[&Path], all: &[PathBuf]) -> Vec<LayoutHit> {
        match self {
            LayoutCheck::Forbid => selected
                .iter()
                .map(|p| LayoutHit {
                    path: p.to_path_buf(),
                    detail: "this file should not exist".into(),
                })
                .collect(),
            LayoutCheck::NameMatches(re) => selected
                .iter()
                .filter(|p| {
                    !p.file_name()
                        .is_some_and(|n| re.is_match(&n.to_string_lossy()))
                })
                .map(|p| LayoutHit {
                    path: p.to_path_buf(),
                    detail: format!("name does not match `{}`", re.as_str()),
                })
                .collect(),
            LayoutCheck::Require { each_dir, require } => {
                let dirs: BTreeSet<&Path> = all
                    .iter()
                    .flat_map(|f| f.ancestors().skip(1))
                    .filter(|d| !d.as_os_str().is_empty() && each_dir.is_match(d))
                    .collect();
                let mut hits = Vec::new();
                for dir in dirs {
                    let inside: Vec<&Path> = all
                        .iter()
                        .filter_map(|f| f.strip_prefix(dir).ok())
                        .collect();
                    for (text, glob) in require {
                        if !inside.iter().any(|f| glob.is_match(f)) {
                            hits.push(LayoutHit {
                                path: dir.to_path_buf(),
                                detail: format!("missing `{text}`"),
                            });
                        }
                    }
                }
                hits
            }
        }
    }
}
