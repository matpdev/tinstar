//! The first, cheap pass: the whole document in one call, to set aside the sections with nothing
//! a checker could see (process, intent, descriptions). Only the rest get the full compile, with
//! the project's picture and the law format, which is where the tokens go.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use super::NotCheckable;
use super::llm::{Llm, Usage};
use super::sections::Section;

const SYSTEM: &str = "You sort the sections of a software project's rules document (like \
AGENTS.md). A later step turns rules into automatic checks that look at source code, config files \
(YAML/JSON: Kubernetes, CI workflows, package.json…) and the file tree, without running anything. \
For each section, decide whether it states at least one rule such a check could see: a construct to \
use or avoid, a naming or placement convention, a required or forbidden file, a config value. \
Sections about process (pull requests, reviews, commits), intent, runtime behavior, or that only \
describe how something works, are not checkable: list their rules in `not_checkable`, each with a \
one-line reason. When in doubt, say checkable: a later step looks closer.";

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["sections"],
        "properties": {
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["anchor", "checkable", "not_checkable"],
                    "properties": {
                        "anchor": { "type": "string" },
                        "checkable": { "type": "boolean" },
                        "not_checkable": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["statement", "reason"],
                                "properties": {
                                    "statement": { "type": "string" },
                                    "reason": { "type": "string" }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct Answer {
    sections: Vec<Verdict>,
}

#[derive(Deserialize)]
struct Verdict {
    anchor: String,
    checkable: bool,
    #[serde(default)]
    not_checkable: Vec<NotCheckable>,
}

/// Sections set aside, by anchor, with the rules they hold. A section the answer leaves out is
/// compiled: skipping a rule by mistake costs more than compiling a section for nothing.
pub fn run(
    llm: &dyn Llm,
    doc_name: &str,
    sections: &[&Section],
) -> Result<(HashMap<String, Vec<NotCheckable>>, Usage)> {
    let mut text = format!("Document: {doc_name}\n\n");
    for s in sections {
        text.push_str(&format!(
            "<section anchor=\"{}\">\n{}</section>\n\n",
            s.anchor, s.text
        ));
    }
    let messages = [json!({ "role": "user", "content": text })];
    let reply = llm.complete(SYSTEM, &messages, &schema(), "triage")?;
    let answer: Answer =
        serde_json::from_str(reply.text.trim()).context("the triage answer is not valid JSON")?;
    let skipped = answer
        .sections
        .into_iter()
        .filter(|v| !v.checkable && sections.iter().any(|s| s.anchor == v.anchor))
        .map(|v| (v.anchor, v.not_checkable))
        .collect();
    Ok((skipped, reply.usage))
}
