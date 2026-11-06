//! The LLM providers `tinstar compile` can use. Each one takes a conversation and returns the
//! answer's text plus the assistant message to append to the conversation for the next turn
//! (providers differ in what that message must contain).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Tokens spent, for the report: compiling is the only moment tinstar spends tokens.
#[derive(Default, Clone, Copy)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
}

pub struct Reply {
    pub text: String,
    /// The message to append to the conversation, as the provider wants it back.
    pub message: Value,
    pub usage: Usage,
}

pub trait Llm: Send + Sync {
    /// `messages` are `{role, content}` objects (user / assistant turns).
    fn complete(
        &self,
        system: &str,
        messages: &[Value],
        schema: &Value,
        tag: &str,
    ) -> Result<Reply>;
    fn describe(&self) -> String;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Provider {
    Anthropic,
    Deepseek,
    Openai,
    /// Any OpenAI-compatible server (Ollama, LM Studio, vLLM…): needs --base-url and --model.
    Compatible,
}

pub struct Choice {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub base_url: Option<String>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Picks the provider: the one asked for, else the first with a key in the environment.
pub fn from_choice(c: Choice) -> Result<Box<dyn Llm>> {
    if let Some(path) = env("TINSTAR_LLM_REPLAY") {
        return Ok(Box::new(Replay::load(&path)?));
    }
    let provider = match c.provider {
        Some(p) => p,
        None if env("DEEPSEEK_API_KEY").is_some() => Provider::Deepseek,
        None if env("ANTHROPIC_API_KEY").is_some() => Provider::Anthropic,
        None if env("OPENAI_API_KEY").is_some() => Provider::Openai,
        None => bail!(
            "no LLM configured: set DEEPSEEK_API_KEY, ANTHROPIC_API_KEY or OPENAI_API_KEY, \
             or use --provider compatible --base-url http://localhost:11434/v1 --model <name> for a local model"
        ),
    };
    let key = |name: &str| env(name).with_context(|| format!("{name} is not set"));
    Ok(match provider {
        Provider::Anthropic => Box::new(Anthropic {
            key: key("ANTHROPIC_API_KEY")?,
            model: c.model.unwrap_or_else(|| "claude-opus-5-5".into()),
            base_url: c
                .base_url
                .unwrap_or_else(|| "https://api.anthropic.com".into()),
        }),
        Provider::Deepseek => Box::new(OpenAiCompatible {
            key: Some(key("DEEPSEEK_API_KEY")?),
            model: c.model.unwrap_or_else(|| "deepseek-chat".into()),
            base_url: c
                .base_url
                .unwrap_or_else(|| "https://api.deepseek.com".into()),
            name: "DeepSeek",
        }),
        Provider::Openai => Box::new(OpenAiCompatible {
            key: Some(key("OPENAI_API_KEY")?),
            model: c
                .model
                .context("--model is required with --provider openai")?,
            base_url: c
                .base_url
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            name: "OpenAI",
        }),
        Provider::Compatible => Box::new(OpenAiCompatible {
            key: env("TINSTAR_LLM_API_KEY"),
            model: c
                .model
                .context("--model is required with --provider compatible")?,
            base_url: c
                .base_url
                .context("--base-url is required with --provider compatible")?,
            name: "OpenAI-compatible",
        }),
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into()
}

fn post(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<Value> {
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut resp = req.send_json(body).with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let text = resp
        .body_mut()
        .read_to_string()
        .context("reading the response")?;
    if !status.is_success() {
        bail!(
            "{url} answered {status}: {}",
            text.chars().take(600).collect::<String>()
        );
    }
    serde_json::from_str(&text).with_context(|| format!("{url} did not answer JSON"))
}

// ------------------------------------------------------------------------------------ Anthropic

struct Anthropic {
    key: String,
    model: String,
    base_url: String,
}

impl Llm for Anthropic {
    fn describe(&self) -> String {
        format!("Anthropic {}", self.model)
    }

    fn complete(
        &self,
        system: &str,
        messages: &[Value],
        schema: &Value,
        _tag: &str,
    ) -> Result<Reply> {
        let body = json!({
            "model": self.model,
            "max_tokens": 16000,
            "system": system,
            "messages": messages,
            "output_config": {
                "effort": "high",
                "format": { "type": "json_schema", "schema": schema },
            },
            // If a safety classifier declines, the API retries on a fallback model by itself.
            "fallbacks": "default",
        });
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let v = post(
            &url,
            &[
                ("x-api-key", &self.key),
                ("anthropic-version", "2026-10-21"),
                ("anthropic-beta", "server-side-fallback-2026-07-01"),
                ("content-type", "application/json"),
            ],
            &body,
        )?;
        match v["stop_reason"].as_str() {
            Some("refusal") => bail!("the model declined: {}", v["stop_details"]["explanation"]),
            Some("max_tokens") => bail!("the answer was cut at max_tokens"),
            _ => {}
        }
        let text: String = v["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect();
        Ok(Reply {
            text,
            // The whole content goes back, thinking blocks included, unchanged.
            message: json!({ "role": "assistant", "content": v["content"] }),
            usage: Usage {
                input: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
                output: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
            },
        })
    }
}

// --------------------------------------------------------------------------- OpenAI-compatible

struct OpenAiCompatible {
    key: Option<String>,
    model: String,
    base_url: String,
    name: &'static str,
}

impl Llm for OpenAiCompatible {
    fn describe(&self) -> String {
        format!("{} {}", self.name, self.model)
    }

    fn complete(
        &self,
        system: &str,
        messages: &[Value],
        schema: &Value,
        _tag: &str,
    ) -> Result<Reply> {
        let mut all = vec![json!({
            "role": "system",
            "content": format!("{system}\n\nAnswer with one JSON object matching this JSON Schema:\n{schema}"),
        })];
        all.extend(messages.iter().cloned());
        let body = json!({
            "model": self.model,
            "messages": all,
            "response_format": { "type": "json_object" },
            "max_tokens": 8000,
        });
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let auth = self.key.as_ref().map(|k| format!("Bearer {k}"));
        let mut headers = vec![("content-type", "application/json")];
        if let Some(a) = &auth {
            headers.push(("authorization", a));
        }
        let v = post(&url, &headers, &body)?;
        let choice = &v["choices"][0];
        if choice["finish_reason"] == "length" {
            bail!("the answer was cut at max_tokens");
        }
        let text = choice["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        Ok(Reply {
            message: json!({ "role": "assistant", "content": text }),
            text,
            usage: Usage {
                input: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                output: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            },
        })
    }
}

// ------------------------------------------------------------------------------------- Replay

/// Recorded answers, for tests and demos without a network: a JSON object mapping each section's
/// anchor to the list of answers to give, in order (`TINSTAR_LLM_REPLAY=answers.json`).
struct Replay {
    answers: Mutex<HashMap<String, Vec<Value>>>,
}

impl Replay {
    fn load(path: &str) -> Result<Replay> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        let map: HashMap<String, Vec<Value>> = serde_json::from_str(&text)?;
        Ok(Replay {
            answers: Mutex::new(map),
        })
    }
}

impl Llm for Replay {
    fn describe(&self) -> String {
        "replay".into()
    }

    fn complete(
        &self,
        _system: &str,
        _messages: &[Value],
        _schema: &Value,
        tag: &str,
    ) -> Result<Reply> {
        let mut answers = self.answers.lock().expect("replay lock");
        let list = answers
            .get_mut(tag)
            .with_context(|| format!("replay has no answers for `{tag}`"))?;
        if list.is_empty() {
            bail!("replay ran out of answers for `{tag}`");
        }
        let answer = list.remove(0);
        let text = answer.to_string();
        Ok(Reply {
            message: json!({ "role": "assistant", "content": text }),
            text,
            usage: Usage::default(),
        })
    }
}
