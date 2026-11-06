//! `kind: pattern`: an ast-grep rule. The law's rule fields are handed to ast-grep as they are,
//! so its whole rule language (patterns, `inside`, `has`, `not`, constraints, fix) is available.

use ast_grep_config::{GlobalRules, RuleConfig, SerializableRuleConfig};
use ast_grep_language::SupportLang;
use serde_yaml::{Mapping, Value};

use super::{Hit, Root};
use crate::diag::Fix;
use crate::law::{LawError, LawFile, invalid};

pub(crate) struct PatternCheck {
    pub lang: SupportLang,
    rule: RuleConfig<SupportLang>,
}

/// `$NAME`, `$$$NAME` in rule text.
fn vars(text: &str) -> std::collections::BTreeSet<String> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\$+([A-Z_][A-Z0-9_]*)").expect("regex"));
    RE.captures_iter(text).map(|c| c[1].to_string()).collect()
}

/// A `$VAR` in the message or the fix that the rule never captures would print as nothing
/// ("call to ``"), so it is an error.
fn undefined_vars(file: &LawFile, rule: &Value) -> Result<(), LawError> {
    let text = |v: &Value| serde_yaml::to_string(v).unwrap_or_default();
    let mut defined = vars(&text(rule));
    if let Some(u) = &file.utils {
        defined.extend(vars(&text(u)));
    }
    if let Some(Value::Mapping(t)) = &file.transform {
        defined.extend(t.keys().filter_map(Value::as_str).map(str::to_string));
    }
    let mut used = vars(&file.message);
    if let Some(f) = &file.fix {
        used.extend(vars(&text(f)));
    }
    let missing: Vec<String> = used
        .difference(&defined)
        .map(|v| format!("`${v}`"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        invalid(format!(
            "{} used in `message` or `fix` but never captured by `rule`",
            missing.join(", ")
        ))
    }
}

impl PatternCheck {
    pub fn compile(file: &LawFile) -> Result<Self, LawError> {
        let Some(language) = &file.language else {
            return invalid(
                "a pattern law needs `language` (typescript, python, rust, go, yaml, hcl…)",
            );
        };
        let lang: SupportLang = language
            .parse()
            .or_else(|_| invalid(format!("unknown language `{language}`")))?;
        let Some(rule) = &file.rule else {
            return invalid("a pattern law needs `rule` (an ast-grep rule)");
        };

        let mut map = Mapping::new();
        let mut put = |k: &str, v: Value| {
            map.insert(Value::from(k), v);
        };
        put("id", Value::from(file.id.as_str()));
        put("language", Value::from(language.as_str()));
        put("message", Value::from(file.message.as_str()));
        put("rule", rule.clone());
        for (key, value) in [
            ("constraints", &file.constraints),
            ("utils", &file.utils),
            ("transform", &file.transform),
            ("fix", &file.fix),
        ] {
            if let Some(v) = value {
                put(key, v.clone());
            }
        }
        undefined_vars(file, rule)?;
        let yaml = serde_yaml::to_string(&map)?;
        let config: SerializableRuleConfig<SupportLang> =
            ast_grep_config::from_str(&yaml).or_else(|e| invalid(format!("ast-grep rule: {e}")))?;
        let rule = RuleConfig::try_from(config, &GlobalRules::default())
            .or_else(|e| invalid(format!("ast-grep rule: {e}")))?;
        Ok(Self { lang, rule })
    }

    pub fn check(&self, root: &Root) -> Vec<Hit> {
        root.root()
            .find_all(&self.rule.matcher)
            .map(|m| {
                let range = m.range();
                let fix = self.rule.fixer.first().map(|fixer| {
                    let edit = m.replace_by(fixer);
                    Fix {
                        start: edit.position,
                        end: edit.position + edit.deleted_length,
                        replacement: String::from_utf8_lossy(&edit.inserted_text).into_owned(),
                    }
                });
                Hit {
                    start: range.start,
                    end: range.end,
                    message: Some(self.rule.get_message(&m)),
                    fix,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::Law;

    fn law(message: &str) -> Result<Law, crate::LawError> {
        Law::from_yaml(&format!(
            "id: x\nlanguage: typescript\nmessage: \"{message}\"\nrule: {{ pattern: fetch($URL) }}\n"
        ))
    }

    #[test]
    fn message_vars_must_be_captured() {
        assert!(law("call to $URL").is_ok());
        let e = law("call to $HOST").err().expect("an error").to_string();
        assert!(e.contains("`$HOST`"), "{e}");
    }
}

/// The way `tinstar compile`'s prompt tells the LLM to leave Rust's inline tests out.
#[cfg(test)]
mod rust_inline_tests {
    use std::path::Path;

    use crate::{Law, Lawbook};

    #[test]
    fn leaves_inline_tests_out() {
        let law = Law::from_yaml(
            r#"
id: no-unwrap
language: rust
message: unwrap panics
rule:
  pattern: $X.unwrap()
  not:
    inside:
      stopBy: end
      any:
        - kind: mod_item
          follows: { kind: attribute_item, regex: 'cfg\(test\)' }
        - kind: function_item
          follows: { kind: attribute_item, regex: '^#\[(\w+::)?test' }
"#,
        )
        .expect("the law loads");
        let src = r#"
pub fn parse(s: &str) -> u32 { s.parse().unwrap() }

#[cfg(test)]
mod tests {
    #[test]
    fn parses() { "1".parse::<u32>().unwrap(); }
}

#[gpui::test]
async fn top_level(cx: &mut TestAppContext) { "3".parse::<u32>().unwrap(); }
"#;
        let book = Lawbook::from_laws(Path::new("."), vec![law]);
        let hits = book.check_source(Path::new("src/lib.rs"), src);
        let lines: Vec<usize> = hits.iter().filter_map(|d| d.span.map(|s| s.line)).collect();
        assert_eq!(lines, [2]);
    }
}
