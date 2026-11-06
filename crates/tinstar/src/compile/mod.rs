//! `tinstar compile`: drafts laws from a rules document (AGENTS.md) with an LLM, once.
//!
//! 1. Sections unchanged since the last compile are skipped (`ledger`).
//! 2. One cheap call sets aside the sections with nothing checkable (`triage`).
//! 3. Each remaining section goes to the LLM with a picture of the project. Every law that comes
//!    back is compiled and run against its own examples, then on the project: a law that fails
//!    goes back with the error, and one that fires too often goes back with a sample of its hits
//!    (`noise`), a few times at most.
//!
//! Only laws that pass are written, to `.tinstar/laws/pending/`, for a person to review and
//! approve. Rules that can't be checked from the code are listed, not forced.

mod ledger;
mod llm;
mod noise;
mod sections;
mod strict_json;
mod triage;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tinstar_core::{Diagnostic, LAWS_DIR, Law, Lawbook, walk_files};

use ledger::{Entry, Ledger};
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
    pub force: bool,
    pub triage: bool,
    pub noise_limit: usize,
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
        "required": ["laws", "not_checkable", "confirmed"],
        "properties": {
            "laws": {
                "type": "array",
                "items": {
                    "type": "object",
                    "description": "One law, as a JSON object with the fields of a law file."
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
            },
            "confirmed": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "reason"],
                    "properties": { "id": { "type": "string" }, "reason": { "type": "string" } }
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    laws: Vec<Value>,
    #[serde(default)]
    not_checkable: Vec<NotCheckable>,
    /// Laws sent back by the noise check whose hits the LLM says are real violations.
    #[serde(default)]
    confirmed: Vec<Confirmed>,
}

/// A law as the LLM wrote it, as YAML text for the law file. Laws come as JSON objects: no YAML
/// syntax to get wrong (unquoted colons, indentation), and tinstar writes the YAML. An object
/// holding only `yaml` text (the older answer shape) is taken as it is.
fn draft_yaml(law: &Value) -> Result<String, String> {
    match law {
        Value::Object(m) if m.len() == 1 && m.get("yaml").is_some_and(Value::is_string) => {
            Ok(m["yaml"].as_str().unwrap_or_default().to_string())
        }
        Value::Object(_) => serde_yaml::to_string(law).map_err(|e| e.to_string()),
        _ => Err("each item of `laws` must be a JSON object (one law)".into()),
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct NotCheckable {
    statement: String,
    reason: String,
}

#[derive(Deserialize)]
struct Confirmed {
    id: String,
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
    /// Why it was kept although it fires often (the LLM's answer to the noise check).
    confirmed: Option<String>,
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
    /// Laws the LLM dropped after the noise check. They passed their examples, so they are kept
    /// as drafts for a person who may still want them (as warnings, say).
    dropped: Vec<Accepted>,
    not_checkable: Vec<NotCheckable>,
    attempts: usize,
    usage: Usage,
    error: Option<String>,
}

/// What happens to a section in this run.
enum Step {
    /// Same text as last time: what it gave then still holds.
    Unchanged(Entry),
    /// The triage found nothing checkable in it.
    SetAside(Vec<NotCheckable>),
    Compile,
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
    let named = !opts.sections.is_empty();
    if named {
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
    let mut ledger = Ledger::load(root);
    let mut total = Usage::default();

    // 1. Sections named on the command line, or all with --force, are compiled again; the others
    //    only if their text changed.
    let fingerprints: Vec<String> = sections
        .iter()
        .map(|s| sections::fingerprint(&s.text))
        .collect();
    let mut steps: Vec<Step> = sections
        .iter()
        .zip(&fingerprints)
        .map(|(s, f)| match ledger.unchanged(&doc_name, &s.anchor, f) {
            Some(e) if !named && !opts.force => Step::Unchanged(e.clone()),
            _ => Step::Compile,
        })
        .collect();
    let to_compile = |steps: &[Step]| -> Vec<usize> {
        (0..steps.len())
            .filter(|&i| matches!(steps[i], Step::Compile))
            .collect()
    };
    println!(
        "compiling {} of {} section(s) of {doc_name} with {}",
        to_compile(&steps).len(),
        sections.len(),
        llm.describe()
    );

    // 2. Triage: one call over the whole document, so sections about process or intent don't
    //    pay for the full prompt. Skipped when sections are named: the person already chose.
    let todo = to_compile(&steps);
    if opts.triage && !named && todo.len() > 2 {
        let refs: Vec<&Section> = todo.iter().map(|&i| &sections[i]).collect();
        match triage::run(llm.as_ref(), &doc_name, &refs) {
            Ok((mut skipped, usage)) => {
                total += usage;
                for &i in &todo {
                    if let Some(nc) = skipped.remove(&sections[i].anchor) {
                        steps[i] = Step::SetAside(nc);
                    }
                }
            }
            Err(e) => println!("  ! triage failed ({e:#}); compiling every section"),
        }
    }

    // 3. Compile. The first section goes alone, so the prompt's stable start (law format and
    //    project picture) is cached before the others ask for it; then a few at a time.
    let todo = to_compile(&steps);
    let system = system_prompt();
    let schema = schema();
    let overview = sections::overview(&files);
    let ctx = Ctx {
        root,
        files: &files,
        doc_name: &doc_name,
        llm: llm.as_ref(),
        system: &system,
        schema: &schema,
        overview: &overview,
        attempts: opts.attempts.max(1),
        noise_limit: opts.noise_limit,
        taken: &taken,
    };
    let results: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::new());
    if let Some(&first) = todo.first() {
        let outcome = compile_section(&ctx, &sections[first]);
        results.lock().expect("results lock").push((first, outcome));
    }
    let next = AtomicUsize::new(1);
    std::thread::scope(|scope| {
        for _ in 0..todo.len().saturating_sub(1).min(4) {
            scope.spawn(|| {
                loop {
                    let n = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&i) = todo.get(n) else {
                        break;
                    };
                    let outcome = compile_section(&ctx, &sections[i]);
                    results.lock().expect("results lock").push((i, outcome));
                }
            });
        }
    });
    let mut results = results.into_inner().expect("results lock");

    // 4. Write what passed, and report, in the document's order.
    std::fs::create_dir_all(&pending_dir)?;
    let mut written = Vec::new();
    let (mut rejected, mut unchanged, mut set_aside) = (0, 0, 0);
    let mut written_ids = taken.clone();
    let mut failed = false;
    for (i, section) in sections.iter().enumerate() {
        println!("\n## {} ({}#{})", section.heading, doc_name, section.anchor);
        let outcome = match &steps[i] {
            Step::Unchanged(e) => {
                unchanged += 1;
                println!(
                    "  = unchanged since the last compile: {} law(s), {} rule(s) left to the document",
                    e.laws.len(),
                    e.not_checkable.len()
                );
                continue;
            }
            Step::SetAside(nc) => {
                set_aside += 1;
                print_not_checkable(&doc_name, nc);
                if nc.is_empty() {
                    println!("  · no rules here");
                }
                ledger.record(
                    &doc_name,
                    &section.anchor,
                    Entry {
                        fingerprint: fingerprints[i].clone(),
                        laws: Vec::new(),
                        not_checkable: nc.clone(),
                    },
                );
                continue;
            }
            Step::Compile => {
                let at = results
                    .iter()
                    .position(|(j, _)| *j == i)
                    .expect("every compiled section has an outcome");
                results.swap_remove(at).1
            }
        };
        total += outcome.usage;
        if let Some(e) = &outcome.error {
            failed = true;
            println!("  ✗ {e}");
        }
        let mut section_laws = Vec::new();
        for a in &outcome.accepted {
            let mut id = a.id.clone();
            let mut n = 2;
            // Two sections running at once may pick the same id.
            while written_ids.contains(&id) {
                id = format!("{}-{n}", a.id);
                n += 1;
            }
            written_ids.insert(id.clone());
            let mut header = format!(
                "# Drafted by `tinstar compile` from {doc_name}#{} with {}.\n\
                 # It passed its {} example(s) and fires {} time(s) in the project today.\n",
                section.anchor,
                llm.describe(),
                a.examples,
                a.hits,
            );
            if let Some(why) = &a.confirmed {
                header.push_str(&format!("# Kept after the noise check: {why}\n"));
            }
            for w in &a.warnings {
                header.push_str(&format!("# Warning: {w}\n"));
            }
            header.push_str(&format!("# Review it, then: tinstar approve {id}\n"));
            let path = pending_dir.join(format!("{id}.yaml"));
            std::fs::write(&path, format!("{header}{}\n", a.yaml.trim_end()))?;
            let fixed = if a.attempt > 1 {
                format!(", fixed from feedback (answer {})", a.attempt)
            } else {
                String::new()
            };
            println!(
                "  ✓ {id}: {} example(s) pass, {} hit(s) in the project{fixed}",
                a.examples, a.hits
            );
            if let Some(why) = &a.confirmed {
                println!("    kept after the noise check: {why}");
            }
            for w in &a.warnings {
                println!("    ! {w}");
            }
            section_laws.push(id.clone());
            written.push(id);
        }
        for a in &outcome.dropped {
            let dir = pending_dir.join("rejected");
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{}.yaml", a.id));
            std::fs::write(
                &path,
                format!(
                    "# Drafted by `tinstar compile` from {doc_name}#{} with {}. It passes its\n\
                     # {} example(s) but fires {} time(s) in the project, and the LLM dropped it after\n\
                     # the noise check. Keep it (maybe as `severity: warning`) by moving it to\n\
                     # .tinstar/laws/, or delete it.\n{}\n",
                    section.anchor,
                    llm.describe(),
                    a.examples,
                    a.hits,
                    a.yaml.trim_end()
                ),
            )?;
            println!(
                "  ~ {}: dropped after the noise check (it fired {} time(s))\n    draft kept in {}",
                a.id,
                a.hits,
                path.strip_prefix(root).unwrap_or(&path).display()
            );
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
        print_not_checkable(&doc_name, &outcome.not_checkable);
        if outcome.accepted.is_empty()
            && outcome.rejected.is_empty()
            && outcome.dropped.is_empty()
            && outcome.not_checkable.is_empty()
            && outcome.error.is_none()
        {
            println!("  · no rules here");
        }
        // A section is remembered unless the call itself failed (network, provider): running again
        // shouldn't pay for the same rejected drafts. --force or --section tries again.
        if outcome.error.is_none() {
            ledger.record(
                &doc_name,
                &section.anchor,
                Entry {
                    fingerprint: fingerprints[i].clone(),
                    laws: section_laws,
                    not_checkable: outcome.not_checkable.clone(),
                },
            );
        } else {
            ledger.forget(&doc_name, &section.anchor);
        }
    }
    ledger.save(root)?;

    println!(
        "\n{} law(s) drafted in {}, {} rejected; {} section(s) unchanged, {} set aside by the triage.",
        written.len(),
        pending_dir
            .strip_prefix(root)
            .unwrap_or(&pending_dir)
            .display(),
        rejected,
        unchanged,
        set_aside,
    );
    println!(
        "{} input tokens ({} from the cache) + {} output, spent once.",
        total.input, total.cached, total.output
    );
    if !written.is_empty() {
        println!("review them, then: tinstar approve --all (or one id at a time)");
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn print_not_checkable(doc_name: &str, items: &[NotCheckable]) {
    for nc in items {
        println!(
            "  · stays in {doc_name}: \"{}\" ({})",
            nc.statement, nc.reason
        );
    }
}

struct Ctx<'a> {
    root: &'a Path,
    files: &'a [PathBuf],
    doc_name: &'a str,
    llm: &'a dyn Llm,
    system: &'a str,
    schema: &'a Value,
    /// The project's picture: the same for every section, sent first so it can be cached.
    overview: &'a str,
    attempts: usize,
    noise_limit: usize,
    taken: &'a HashSet<String>,
}

fn compile_section(ctx: &Ctx, section: &Section) -> Outcome {
    let source = format!("{}#{}", ctx.doc_name, section.anchor);
    let mentioned = sections::mentioned(ctx.root, ctx.files, &section.text);
    let files = if mentioned.is_empty() {
        String::new()
    } else {
        format!("\n\n<files>\n{mentioned}</files>")
    };
    // Stable part first (cached by providers that cache prompts), the section last.
    let first = json!({ "role": "user", "content": [
        { "type": "text", "text": format!("<project>\n{}</project>", ctx.overview), "cache": true },
        { "type": "text", "text": format!(
            "Document: {doc}. Section \"{heading}\", source: `{source}`.\n\n<section>\n{text}</section>{files}",
            doc = ctx.doc_name,
            heading = section.heading,
            text = section.text,
        ) },
    ]});
    let mut messages = vec![first];
    let mut out = Outcome::default();
    let mut failing: Vec<Rejected> = Vec::new();
    // Laws held back by the noise check, waiting for the LLM's verdict, with their hits.
    let mut held: Vec<(Accepted, Vec<Diagnostic>)> = Vec::new();
    // Each law goes through the noise check once: a narrowed version is taken as it is.
    let mut reviewed: HashSet<String> = HashSet::new();
    // Laws still failing after the last regular answer.
    let mut rejected: Vec<Rejected> = Vec::new();

    // `attempts` answers to get laws right, plus one more, only for the noise check, when laws
    // that passed on the last answer still wait for it.
    for attempt in 1..=ctx.attempts + 1 {
        let noise_round = attempt > ctx.attempts;
        if noise_round && held.is_empty() {
            break;
        }
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
        out.usage += reply.usage;
        messages.push(reply.message.clone());

        let parsed =
            strict_json::parse(json_body(&reply.text)).and_then(serde_json::from_value::<Answer>);
        let answer: Answer = match parsed {
            Ok(a) => a,
            Err(e) => {
                messages.push(json!({ "role": "user", "content": format!("Your answer was not valid JSON ({e}). Answer again with the JSON object only.") }));
                continue;
            }
        };
        for nc in answer.not_checkable {
            if !out
                .not_checkable
                .iter()
                .any(|o| o.statement == nc.statement)
            {
                out.not_checkable.push(nc);
            }
        }
        let drafts: Vec<Result<String, String>> = answer.laws.iter().map(draft_yaml).collect();
        let answered: Vec<Option<String>> = drafts
            .iter()
            .map(|d| d.as_ref().ok().and_then(|y| id_of(y)))
            .collect();
        if noise_round {
            noise_verdicts(
                ctx,
                &source,
                &answer.confirmed,
                &drafts,
                &answered,
                &mut held,
                &mut out,
                attempt,
            );
            break;
        }
        // The verdict on laws held by the noise check: confirmed ones are kept; the others are
        // dropped, unless a narrower version came back (it is checked below).
        for (a, _) in std::mem::take(&mut held) {
            let narrowed = answered.iter().any(|id| id.as_deref() == Some(&a.id));
            match answer.confirmed.iter().find(|c| c.id == a.id) {
                Some(c) => out.accepted.push(Accepted {
                    confirmed: Some(c.reason.clone()),
                    ..a
                }),
                None if !narrowed => out.dropped.push(a),
                None => {}
            }
        }
        // A failing law the answer leaves out stays failing: it is asked for again, and in the
        // end kept as a rejected draft rather than lost.
        let previous = std::mem::take(&mut failing);
        failing.extend(previous.into_iter().filter(|r| {
            !answered
                .iter()
                .any(|a| a.as_deref() == Some(r.label.as_str()))
        }));
        for (n, draft) in drafts.into_iter().enumerate() {
            let label = answered[n]
                .clone()
                .unwrap_or_else(|| format!("law #{}", n + 1));
            let draft = match draft {
                Ok(yaml) => yaml,
                Err(error) => {
                    failing.push(Rejected {
                        label,
                        error,
                        yaml: answer.laws[n].to_string(),
                    });
                    continue;
                }
            };
            let taken =
                |id: &str| ctx.taken.contains(id) || out.accepted.iter().any(|a| a.id == id);
            match validate(ctx, &draft, &source) {
                Ok((a, _)) if taken(&a.id) => failing.push(Rejected {
                    error: format!("the id `{}` is already used; choose another", a.id),
                    label,
                    yaml: draft.clone(),
                }),
                Ok((a, hits)) if a.hits > ctx.noise_limit && reviewed.insert(a.id.clone()) => {
                    held.push((Accepted { attempt, ..a }, hits));
                }
                Ok((mut a, _)) => {
                    if a.hits > ctx.noise_limit {
                        a.warnings.push(format!(
                            "still fires {} times in the project after the noise check",
                            a.hits
                        ));
                    }
                    out.accepted.push(Accepted { attempt, ..a });
                }
                Err(error) => failing.push(Rejected {
                    label,
                    error,
                    yaml: draft.clone(),
                }),
            }
        }
        if failing.is_empty() && held.is_empty() {
            break;
        }
        if attempt == ctx.attempts {
            // Out of answers for getting laws right: what still fails is final. Laws waiting for
            // the noise check get their own, last round.
            rejected.append(&mut failing);
        }
        if attempt <= ctx.attempts && !(failing.is_empty() && held.is_empty()) {
            let mut feedback = String::new();
            if !failing.is_empty() {
                feedback.push_str("tinstar checked your laws. These failed:\n\n");
                for r in &failing {
                    feedback.push_str(&format!("### {}\n{}\n\n", r.label, r.error));
                }
            }
            if !held.is_empty() {
                feedback.push_str(noise::ASK);
                feedback.push_str("\n\n");
                for (a, hits) in &held {
                    feedback.push_str(&noise::report(&a.id, hits, ctx.root));
                    feedback.push('\n');
                }
            }
            if !out.accepted.is_empty() {
                let ids: Vec<&str> = out.accepted.iter().map(|a| a.id.as_str()).collect();
                feedback.push_str(&format!(
                    "Already accepted (don't repeat them): {}.\n",
                    ids.join(", ")
                ));
            }
            feedback.push_str("Answer with corrected versions of these laws only, in the same JSON shape (not_checkable may be empty).");
            messages.push(json!({ "role": "user", "content": feedback }));
        }
    }
    // The noise round's answer was not valid JSON: keep held laws for the person reviewing.
    for (mut a, _) in held {
        a.warnings.push(format!(
            "fires {} times in the project; the noise check got no usable answer",
            a.hits
        ));
        out.accepted.push(a);
    }
    rejected.append(&mut failing);
    out.rejected = rejected;
    out
}

/// The last round's answer, about laws held by the noise check only. Each is confirmed, replaced by
/// a narrower version, or dropped; a narrower version that breaks keeps the original, with a
/// warning, since the original passed its examples.
#[allow(clippy::too_many_arguments)]
fn noise_verdicts(
    ctx: &Ctx,
    source: &str,
    confirmed: &[Confirmed],
    drafts: &[Result<String, String>],
    answered: &[Option<String>],
    held: &mut Vec<(Accepted, Vec<Diagnostic>)>,
    out: &mut Outcome,
    attempt: usize,
) {
    for (mut a, _) in std::mem::take(held) {
        if let Some(c) = confirmed.iter().find(|c| c.id == a.id) {
            out.accepted.push(Accepted {
                confirmed: Some(c.reason.clone()),
                ..a
            });
            continue;
        }
        let Some(n) = answered.iter().position(|id| id.as_deref() == Some(&a.id)) else {
            out.dropped.push(a);
            continue;
        };
        let narrowed = drafts[n]
            .clone()
            .and_then(|yaml| validate(ctx, &yaml, source).map(|(v, _)| v));
        match narrowed {
            Ok(mut v) => {
                if v.hits > ctx.noise_limit {
                    v.warnings.push(format!(
                        "still fires {} times in the project after the noise check",
                        v.hits
                    ));
                }
                out.accepted.push(Accepted { attempt, ..v });
            }
            Err(e) => {
                a.warnings.push(format!(
                    "fires {} times in the project; its narrower version failed: {}",
                    a.hits,
                    e.lines().next().unwrap_or_default()
                ));
                out.accepted.push(a);
            }
        }
    }
}

/// Compiles a drafted law and runs it against its examples, then on the project (its hits).
fn validate(ctx: &Ctx, yaml: &str, source: &str) -> Result<(Accepted, Vec<Diagnostic>), String> {
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
    if !ctx.files.iter().any(|f| law.selects(f)) {
        warnings.push("its `files` globs select no file in the project today".into());
    }
    let id = law.id.clone();
    let book = Lawbook::from_laws(ctx.root, vec![law]);
    let refs: Vec<&PathBuf> = ctx.files.iter().collect();
    let mut hits = book.check_files(&refs);
    hits.extend(book.check_layout(ctx.files));
    Ok((
        Accepted {
            id,
            attempt: 0,
            yaml: yaml.to_string(),
            examples: valid + invalid,
            hits: hits.len(),
            warnings,
            confirmed: None,
        },
        hits,
    ))
}

/// The JSON object in an answer, without a Markdown fence or words around it.
fn json_body(text: &str) -> &str {
    match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => &text[a..=b],
        _ => text.trim(),
    }
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
