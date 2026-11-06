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
