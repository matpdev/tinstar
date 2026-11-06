//! `tinstar compile`: drafts laws from a rules document (AGENTS.md) with an LLM, once.
//!
//! Each section goes to the LLM with a picture of the project. Every law that comes back is
//! compiled and run against its own examples; the ones that fail go back to the LLM with the error,
//! a few times at most. Only laws that pass are written, to `.tinstar/laws/pending/`, for a person
//! to review and approve. Rules that can't be checked from the code are listed, not forced.

mod llm;
mod sections;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tinstar_core::{LAWS_DIR, Law, Lawbook, walk_files};

pub use llm::Provider;
use llm::{Llm, Usage};
use sections::Section;

pub struct Options {
    pub document: PathBuf,
    pub sections: Vec<String>,
    pub attempts: usize,
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub base_url: Option<String>,
}

const PROMPT: &str = include_str!("prompt.md");

fn system_prompt() -> String {
    PROMPT
        .replace(
            "{{EXAMPLE_PATTERN}}",
            include_str!("../../../../examples/shop/.tinstar/laws/no-float-money.yaml").trim(),
        )
        .replace(
            "{{EXAMPLE_CONFIG}}",
            include_str!("../../../../examples/shop/.tinstar/laws/k8s-pinned-images.yaml").trim(),
        )
        .replace(
            "{{EXAMPLE_LAYOUT}}",
            include_str!("../../../../examples/shop/.tinstar/laws/feature-has-test.yaml").trim(),
        )
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["laws", "not_checkable"],
        "properties": {
            "laws": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["yaml"],
                    "properties": { "yaml": { "type": "string" } }
                }
            },
            "not_checkable": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["statement", "reason"],
                    "properties": { "statement": { "type": "string" }, "reason": { "type": "string" } }
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    laws: Vec<DraftLaw>,
    #[serde(default)]
    not_checkable: Vec<NotCheckable>,
}

#[derive(Deserialize)]
struct DraftLaw {
    yaml: String,
}

#[derive(Deserialize, Clone)]
struct NotCheckable {
    statement: String,
    reason: String,
}

struct Accepted {
    id: String,
    /// Which answer it came in: 1 is right the first time, more means it was fixed from feedback.
    attempt: usize,
    yaml: String,
    examples: usize,
    hits: usize,
    warnings: Vec<String>,
}

/// A draft that still failed after the last answer: kept for people to finish by hand.
struct Rejected {
    /// The law's id, or `law #N` when the YAML has none.
    label: String,
    error: String,
    yaml: String,
}

#[derive(Default)]
struct Outcome {
    accepted: Vec<Accepted>,
    rejected: Vec<Rejected>,
    not_checkable: Vec<NotCheckable>,
    attempts: usize,
    usage: Usage,
    error: Option<String>,
}

pub fn run(root: &Path, opts: Options) -> Result<ExitCode> {
    let doc_path = if opts.document.is_absolute() {
        opts.document.clone()
    } else {
        root.join(&opts.document)
    };
    let markdown = std::fs::read_to_string(&doc_path)
        .with_context(|| format!("reading {}", doc_path.display()))?;
    let doc_name = doc_path
        .strip_prefix(root)
        .unwrap_or(&doc_path)
        .display()
        .to_string();
    let mut sections = sections::split(&markdown);
    if !opts.sections.is_empty() {
        sections.retain(|s| {
            opts.sections
                .iter()
                .any(|a| sections::anchor(a) == s.anchor)
        });
        if sections.is_empty() {
            bail!(
                "no section of {doc_name} matches {}",
                opts.sections.join(", ")
            );
        }
    }
    let llm = llm::from_choice(llm::Choice {
        provider: opts.provider,
        model: opts.model,
        base_url: opts.base_url,
    })?;
    let files = walk_files(root);
    let pending_dir = root.join(LAWS_DIR).join("pending");
    let (book, _) = Lawbook::load(root);
    let mut taken: HashSet<String> = book.laws.iter().map(|l| l.id.clone()).collect();
    taken.extend(existing_pending(&pending_dir));

    println!(
        "compiling {} section(s) of {doc_name} with {}",
        sections.len(),
        llm.describe()
    );
    let system = system_prompt();
    let schema = schema();
    let ctx = Ctx {
        root,
        files: &files,
        doc_name: &doc_name,
        llm: llm.as_ref(),
        system: &system,
        schema: &schema,
        attempts: opts.attempts.max(1),
        taken: &taken,
    };

    // Sections are independent: a few at a time.
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..sections.len().min(4) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(section) = sections.get(i) else {
                        break;
                    };
                    let outcome = compile_section(&ctx, section);
                    results.lock().expect("results lock").push((i, outcome));
                }
            });
        }
    });
    let mut results = results.into_inner().expect("results lock");
    results.sort_by_key(|(i, _)| *i);

    // Write what passed, and report.
    std::fs::create_dir_all(&pending_dir)?;
    let mut written = Vec::new();
    let mut total = Usage::default();
    let mut rejected = 0;
    let mut written_ids = taken.clone();
    for (i, outcome) in &results {
        let section = &sections[*i];
        total.input += outcome.usage.input;
        total.output += outcome.usage.output;
        println!("\n## {} ({}#{})", section.heading, doc_name, section.anchor);
        if let Some(e) = &outcome.error {
            println!("  ✗ {e}");
        }
        for a in &outcome.accepted {
            let mut id = a.id.clone();
            let mut n = 2;
            // Two sections running at once may pick the same id.
            while written_ids.contains(&id) {
                id = format!("{}-{n}", a.id);
                n += 1;
            }
            written_ids.insert(id.clone());
            let header = format!(
                "# Drafted by `tinstar compile` from {doc_name}#{} with {}.\n\
                 # It passed its {} example(s) and fires {} time(s) in the project today.\n\
                 # Review it, then: tinstar approve {id}\n",
                section.anchor,
                llm.describe(),
                a.examples,
                a.hits,
            );
            let path = pending_dir.join(format!("{id}.yaml"));
            std::fs::write(
                &path,
                format!("{header}{}", a.yaml.trim_end().to_string() + "\n"),
            )?;
            let fixed = if a.attempt > 1 {
                format!(", fixed from feedback (answer {})", a.attempt)
            } else {
                String::new()
            };
            println!(
                "  ✓ {id}: {} example(s) pass, {} hit(s) in the project{fixed}",
                a.examples, a.hits
            );
            for w in &a.warnings {
                println!("    ! {w}");
            }
            written.push(id);
        }
        for r in &outcome.rejected {
            rejected += 1;
            let saved = match id_of(&r.yaml) {
                Some(id) => {
                    let dir = pending_dir.join("rejected");
                    std::fs::create_dir_all(&dir)?;
                    let path = dir.join(format!("{id}.yaml"));
                    let error: String = r.error.lines().map(|l| format!("#   {l}\n")).collect();
                    std::fs::write(
                        &path,
                        format!(
                            "# Drafted by `tinstar compile` from {doc_name}#{} with {}, rejected after\n\
                             # {} answer(s). Fix it by hand and move it to .tinstar/laws/, or delete it.\n\
                             # Last error:\n{error}\n{}\n",
                            section.anchor,
                            llm.describe(),
                            outcome.attempts,
                            r.yaml.trim_end()
                        ),
                    )?;
                    format!(
                        "\n    draft kept in {}",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    )
                }
                None => String::new(),
            };
            println!(
                "  ✗ {}: rejected after {} answer(s): {}{saved}",
                r.label,
                outcome.attempts,
                summary(&r.error)
            );
        }
        for nc in &outcome.not_checkable {
            println!(
                "  · stays in {doc_name}: \"{}\" ({})",
                nc.statement, nc.reason
            );
        }
        if outcome.accepted.is_empty()
            && outcome.rejected.is_empty()
            && outcome.not_checkable.is_empty()
            && outcome.error.is_none()
        {
            println!("  · no rules here");
        }
    }
    println!(
        "\n{} law(s) drafted in {}, {} rejected; {} input + {} output tokens, spent once.",
        written.len(),
        pending_dir
            .strip_prefix(root)
            .unwrap_or(&pending_dir)
            .display(),
        rejected,
        total.input,
        total.output
    );
    if !written.is_empty() {
        println!("review them, then: tinstar approve --all (or one id at a time)");
    }
    let failed = results.iter().any(|(_, o)| o.error.is_some());
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

struct Ctx<'a> {
    root: &'a Path,
    files: &'a [PathBuf],
    doc_name: &'a str,
    llm: &'a dyn Llm,
    system: &'a str,
    schema: &'a Value,
    attempts: usize,
    taken: &'a HashSet<String>,
}

fn compile_section(ctx: &Ctx, section: &Section) -> Outcome {
    let source = format!("{}#{}", ctx.doc_name, section.anchor);
    let prompt = format!(
        "Document: {doc}. Section \"{heading}\", source: `{source}`.\n\n<section>\n{text}</section>\n\n<project>\n{project}</project>",
        doc = ctx.doc_name,
        heading = section.heading,
        text = section.text,
        project = sections::project_context(ctx.root, ctx.files, &section.text),
    );
    let mut messages = vec![json!({ "role": "user", "content": prompt })];
    let mut out = Outcome::default();
    let mut failing: Vec<Rejected> = Vec::new();

    for attempt in 1..=ctx.attempts {
        out.attempts = attempt;
        let reply = match ctx
            .llm
            .complete(ctx.system, &messages, ctx.schema, &section.anchor)
        {
            Ok(r) => r,
            Err(e) => {
                out.error = Some(format!("{e:#}"));
                return out;
            }
        };
        out.usage.input += reply.usage.input;
        out.usage.output += reply.usage.output;
        messages.push(reply.message.clone());

        let answer: Answer = match serde_json::from_str(reply.text.trim()) {
            Ok(a) => a,
            Err(e) => {
                messages.push(json!({ "role": "user", "content": format!("Your answer was not valid JSON ({e}). Answer again with the JSON object only.") }));
                continue;
            }
        };
        if attempt == 1 {
            out.not_checkable = answer.not_checkable.clone();
        }
        failing.clear();
        for (n, draft) in answer.laws.iter().enumerate() {
            let label = id_of(&draft.yaml).unwrap_or_else(|| format!("law #{}", n + 1));
            let taken =
                |id: &str| ctx.taken.contains(id) || out.accepted.iter().any(|a| a.id == id);
            match validate(ctx, &draft.yaml, &source) {
                Ok(a) if taken(&a.id) => failing.push(Rejected {
                    error: format!("the id `{}` is already used; choose another", a.id),
                    label,
                    yaml: draft.yaml.clone(),
                }),
                Ok(a) => out.accepted.push(Accepted { attempt, ..a }),
                Err(error) => failing.push(Rejected {
                    label,
                    error,
                    yaml: draft.yaml.clone(),
                }),
            }
        }
        if failing.is_empty() {
            break;
        }
        if attempt < ctx.attempts {
            let mut feedback = String::from("tinstar checked your laws. These failed:\n\n");
            for r in &failing {
                feedback.push_str(&format!("### {}\n{}\n\n", r.label, r.error));
            }
            if !out.accepted.is_empty() {
                let ids: Vec<&str> = out.accepted.iter().map(|a| a.id.as_str()).collect();
                feedback.push_str(&format!(
                    "Already accepted (don't repeat them): {}.\n",
                    ids.join(", ")
                ));
            }
            feedback.push_str("Answer with corrected versions of the failing laws only, in the same JSON shape (not_checkable may be empty).");
            messages.push(json!({ "role": "user", "content": feedback }));
        }
    }
    out.rejected = failing;
    out
}

/// Compiles a drafted law and runs it against its examples and on the project.
fn validate(ctx: &Ctx, yaml: &str, source: &str) -> Result<Accepted, String> {
    let law = Law::from_yaml(yaml).map_err(|e| format!("does not load: {e}"))?;
    if law.source.as_deref() != Some(source) {
        return Err(format!("`source` must be `{source}`"));
    }
    let (valid, invalid) = (law.examples.valid.len(), law.examples.invalid.len());
    if valid == 0 || invalid == 0 {
        return Err(format!(
            "needs valid and invalid examples (has {valid} valid, {invalid} invalid)"
        ));
    }
    let report = law.test_examples();
    if !report.failures.is_empty() {
        let mut e = String::from("gets its own examples wrong:\n");
        for f in &report.failures {
            e.push_str(&format!(
                "- {} example #{} ({}):\n{}\n",
                f.which,
                f.index + 1,
                f.reason,
                indent(&f.example)
            ));
        }
        return Err(e);
    }
    let mut warnings = Vec::new();
    let selected = ctx.files.iter().filter(|f| law.selects(f)).count();
    if selected == 0 {
        warnings.push("its `files` globs select no file in the project today".into());
    }
    let id = law.id.clone();
    let book = Lawbook::from_laws(ctx.root, vec![law]);
    let hits = book.check_project(None).len();
    Ok(Accepted {
        id,
        attempt: 0,
        yaml: yaml.to_string(),
        examples: valid + invalid,
        hits,
        warnings,
    })
}

fn id_of(yaml: &str) -> Option<String> {
    yaml.lines()
        .find_map(|l| l.strip_prefix("id:"))
        .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// An error for the report: its first line, plus which examples failed (without their code).
fn summary(error: &str) -> String {
    let mut lines = error.lines();
    let mut out = lines.next().unwrap_or("").to_string();
    for l in lines.filter(|l| l.starts_with("- ")) {
        out.push_str(&format!("\n      {l}"));
    }
    out
}

fn existing_pending(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .collect()
}

/// `tinstar approve`: moves reviewed laws from `pending/` into the lawbook.
pub fn approve(root: &Path, ids: &[String], all: bool) -> Result<ExitCode> {
    let pending = root.join(LAWS_DIR).join("pending");
    let wanted: Vec<String> = if all {
        existing_pending(&pending)
    } else {
        ids.to_vec()
    };
    if wanted.is_empty() {
        bail!("nothing to approve: name law ids or use --all");
    }
    for id in wanted {
        let from = pending.join(format!("{id}.yaml"));
        let to = root.join(LAWS_DIR).join(format!("{id}.yaml"));
        if !from.exists() {
            bail!("no pending law `{id}`");
        }
        if to.exists() {
            bail!(
                "{} already exists",
                to.strip_prefix(root).unwrap_or(&to).display()
            );
        }
        std::fs::rename(&from, &to)?;
        println!("approved {id}");
    }
    Ok(ExitCode::SUCCESS)
}
