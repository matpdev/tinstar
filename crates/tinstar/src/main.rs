mod compile;
mod hook;
mod init;
mod lsp;
mod mcp;
mod output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use tinstar_core::{Lawbook, LoadError, Severity, find_root};

/// The law in your repo: rules written in plain language, checked on code, infra and layout.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Project root (default: the nearest folder with a `.tinstar/`, from the current one up).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create `.tinstar/laws/` with an example law.
    Init,
    /// Check the project (or the given files and folders) against its laws.
    Check {
        /// Files or folders to check, relative to the current folder (default: the whole project).
        paths: Vec<PathBuf>,
        /// Only files changed in git (modified, staged or untracked).
        #[arg(long, conflicts_with = "paths")]
        changed: bool,
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
    },
    /// Run every law against its own examples.
    Test,
    /// List the laws.
    Laws,
    /// Draft laws from a rules document with an LLM (once); they land in .tinstar/laws/pending/.
    Compile {
        /// The rules document, relative to the project root.
        #[arg(default_value = "AGENTS.md")]
        document: PathBuf,
        /// Only these sections (heading text or anchor); repeatable.
        #[arg(long = "section")]
        sections: Vec<String>,
        /// How many times a failing law goes back to the LLM with its error.
        #[arg(long, default_value_t = 3)]
        attempts: usize,
        /// LLM provider (default: the first of DEEPSEEK_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY set).
        #[arg(long, value_enum)]
        provider: Option<compile::Provider>,
        #[arg(long)]
        model: Option<String>,
        /// API base URL (for --provider compatible: e.g. http://localhost:11434/v1 for Ollama).
        #[arg(long)]
        base_url: Option<String>,
        /// Recompile every section, even the ones unchanged since the last compile.
        #[arg(long)]
        force: bool,
        /// Skip the first pass that sets aside sections with nothing checkable.
        #[arg(long)]
        no_triage: bool,
        /// A law that fires more often than this in the project goes back to the LLM with a sample
        /// of its hits, to be narrowed or confirmed.
        #[arg(long, default_value_t = 25)]
        noise_limit: usize,
    },
    /// Move reviewed laws from .tinstar/laws/pending/ into the lawbook.
    Approve {
        ids: Vec<String>,
        #[arg(long, conflicts_with = "ids")]
        all: bool,
    },
    /// Run the language server (stdio), for editors and agents.
    Lsp,
    /// Run an MCP server (stdio): agents draft laws with their own model, tinstar checks them.
    Mcp,
    /// Run as an agent hook: check the file the agent just edited and tell it what it broke.
    Hook {
        #[arg(value_enum)]
        agent: hook::Agent,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    /// For people, with the offending line.
    Human,
    /// One JSON document, for tools.
    Json,
    /// One compact line per problem, for AI agents.
    Agent,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tinstar: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    match &cli.command {
        Command::Init => {
            init::run(&cli.root.unwrap_or(cwd))?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Lsp => {
            lsp::run()?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Mcp => {
            mcp::run(cli.root.clone())?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Hook { agent } => return hook::run(*agent),
        _ => {}
    }

    let root = match cli.root {
        Some(r) => r
            .canonicalize()
            .with_context(|| format!("no such folder: {}", r.display()))?,
        None => find_root(&cwd)
            .context("no `.tinstar/` here or in any parent folder (run `tinstar init`)")?
            .canonicalize()?,
    };
    let (book, errors) = Lawbook::load(&root);
    report_load_errors(&errors);

    match cli.command {
        Command::Init | Command::Lsp | Command::Mcp | Command::Hook { .. } => {
            unreachable!("handled above")
        }
        Command::Compile {
            document,
            sections,
            attempts,
            provider,
            model,
            base_url,
            force,
            no_triage,
            noise_limit,
        } => compile::run(
            &root,
            compile::Options {
                document,
                sections,
                attempts,
                provider,
                model,
                base_url,
                force,
                triage: !no_triage,
                noise_limit,
            },
        ),
        Command::Approve { ids, all } => compile::approve(&root, &ids, all),
        Command::Laws => {
            output::laws(&book);
            Ok(exit_for_load(&errors))
        }
        Command::Test => {
            let ok = output::tests(&book);
            Ok(if ok && errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Command::Check {
            paths,
            changed,
            format,
        } => {
            let targets = if changed {
                Some(changed_files(&root)?)
            } else if paths.is_empty() {
                None
            } else {
                Some(
                    paths
                        .iter()
                        .map(|p| relative_to_root(&root, &cwd, p))
                        .collect::<Result<_>>()?,
                )
            };
            let diagnostics = match &targets {
                Some(t) if t.is_empty() => Vec::new(),
                t => book.check_project(t.as_deref()),
            };
            output::print(&book, &diagnostics, format);
            if diagnostics.iter().any(|d| d.severity == Severity::Error) {
                Ok(ExitCode::FAILURE)
            } else {
                Ok(exit_for_load(&errors))
            }
        }
    }
}

fn report_load_errors(errors: &[LoadError]) {
    for e in errors {
        eprintln!("tinstar: law not loaded: {e}");
    }
}

/// A broken law file is a configuration error (exit 2), not a broken law (exit 1).
fn exit_for_load(errors: &[LoadError]) -> ExitCode {
    if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

fn relative_to_root(root: &Path, cwd: &Path, p: &Path) -> Result<PathBuf> {
    let abs = cwd.join(p);
    let abs = abs
        .canonicalize()
        .with_context(|| format!("no such file: {}", p.display()))?;
    match abs.strip_prefix(root) {
        Ok(rel) => Ok(rel.to_path_buf()),
        Err(_) => bail!(
            "{} is outside the project ({})",
            p.display(),
            root.display()
        ),
    }
}

/// Modified, staged and untracked files, relative to the root.
fn changed_files(root: &Path) -> Result<Vec<PathBuf>> {
    let git = |args: &[&str]| -> Result<String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .context("could not run git")?;
        if !out.status.success() {
            bail!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    // git prints paths relative to the repository's top level, which may be above the root.
    let top = PathBuf::from(git(&["rev-parse", "--show-toplevel"])?.trim());
    let tracked = git(&["diff", "--name-only", "HEAD"])
        .or_else(|_| git(&["diff", "--name-only", "--cached"]))?;
    let untracked = git(&[
        "ls-files",
        "--others",
        "--exclude-standard",
        "--full-name",
        ":/",
    ])?;
    let mut files: Vec<PathBuf> = tracked
        .lines()
        .chain(untracked.lines())
        .map(|line| top.join(line))
        .filter(|abs| abs.exists())
        .filter_map(|abs| abs.strip_prefix(root).ok().map(Path::to_path_buf))
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}
