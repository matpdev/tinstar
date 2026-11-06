//! `kind: config`: assertions on YAML / JSON documents (Kubernetes manifests, compose files,
//! CI workflows, package.json…).
//!
//! The documents are read from the same syntax tree the pattern laws use (tree-sitter, through
//! ast-grep), so a broken assertion points at the exact key or value, and a file is parsed once
//! for every law.

use ast_grep_core::Node;
use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_language::SupportLang;
use regex::Regex;
use serde_yaml::Value;

use super::{Hit, Root};
use crate::law::{Assertion, LawError, LawFile, invalid};

type TsNode<'r> = Node<'r, StrDoc<SupportLang>>;

pub(crate) struct ConfigCheck {
    pub lang: SupportLang,
    message: String,
    filter: Vec<(Vec<Segment>, Vec<String>)>,
    asserts: Vec<CompiledAssertion>,
}

struct CompiledAssertion {
    path: Vec<Segment>,
    exists: Option<bool>,
    matches: Option<Regex>,
    not_matches: Option<Regex>,
    one_of: Option<Vec<String>>,
    message: Option<String>,
}

impl ConfigCheck {
    pub fn compile(file: &LawFile) -> Result<Self, LawError> {
        let lang = match file.language.as_deref() {
            Some("yaml") | Some("yml") => SupportLang::Yaml,
            Some("json") => SupportLang::Json,
            Some(other) => return invalid(format!("config laws read yaml or json, not `{other}`")),
            None => return invalid("a config law needs `language: yaml` or `language: json`"),
        };
        if file.asserts.is_empty() {
            return invalid("a config law needs at least one `assert`");
        }
        let mut filter = Vec::new();
        for (path, expected) in &file.match_ {
            let values = match expected {
                Value::Sequence(items) => items.iter().filter_map(scalar_string).collect(),
                v => scalar_string(v).into_iter().collect(),
            };
            filter.push((parse_path(path)?, values));
        }
        let asserts = file
            .asserts
            .iter()
            .map(compile_assertion)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            lang,
            message: file.message.clone(),
            filter,
            asserts,
        })
    }

    pub fn check(&self, root: &Root) -> Vec<Hit> {
        let mut hits = Vec::new();
        for doc in documents(&root.root(), self.lang) {
            if !self.selects(&doc) {
                continue;
            }
            for a in &self.asserts {
                for reached in resolve(&doc, &a.path) {
                    if let Some(hit) = a.judge(&reached, &self.message) {
                        hits.push(hit);
                    }
                }
            }
        }
        hits
    }

    /// `match:` — every listed path must reach one of the expected values.
    fn selects(&self, doc: &CNode) -> bool {
        self.filter.iter().all(|(path, expected)| {
            resolve(doc, path).iter().any(|r| match r {
                Reached::Found(n) => n.scalar().is_some_and(|s| expected.iter().any(|e| e == s)),
                Reached::Missing { .. } => false,
            })
        })
    }
}

fn compile_assertion(a: &Assertion) -> Result<CompiledAssertion, LawError> {
    let regex = |r: &Option<String>| -> Result<Option<Regex>, LawError> {
        r.as_deref()
            .map(|r| Regex::new(r).or_else(|e| invalid(format!("bad regex `{r}`: {e}"))))
            .transpose()
    };
    let mut one_of: Option<Vec<String>> = a
        .one_of
        .as_ref()
        .map(|vs| vs.iter().filter_map(scalar_string).collect());
    if let Some(eq) = &a.equals {
        one_of
            .get_or_insert_with(Vec::new)
            .extend(scalar_string(eq));
    }
    let c = CompiledAssertion {
        path: parse_path(&a.path)?,
        exists: a.exists,
        matches: regex(&a.matches)?,
        not_matches: regex(&a.not_matches)?,
        one_of,
        message: a.message.clone(),
    };
    if c.exists.is_none() && c.matches.is_none() && c.not_matches.is_none() && c.one_of.is_none() {
        return invalid(format!(
            "assertion on `{}` checks nothing: add exists, matches, not_matches, equals or one_of",
            a.path
        ));
    }
    Ok(c)
}

impl CompiledAssertion {
    fn judge(&self, reached: &Reached, law_message: &str) -> Option<Hit> {
        let message = self.message.as_deref().unwrap_or(law_message);
        let hit = |n: &CNode, detail: String| Hit {
            start: n.start,
            end: n.end,
            message: Some(format!("{message} ({detail})")),
            fix: None,
        };
        match reached {
            Reached::Missing { at, key } => {
                (self.exists == Some(true)).then(|| hit(at, format!("missing `{key}`")))
            }
            Reached::Found(n) => {
                if self.exists == Some(false) {
                    return Some(hit(n, "should not be set".into()));
                }
                let value = n.scalar()?;
                let shown = format!("found `{value}`");
                if self.matches.as_ref().is_some_and(|r| !r.is_match(value))
                    || self.not_matches.as_ref().is_some_and(|r| r.is_match(value))
                    || self
                        .one_of
                        .as_ref()
                        .is_some_and(|vs| !vs.iter().any(|v| v == value))
                {
                    return Some(hit(n, shown));
                }
                None
            }
        }
    }
}

fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".into()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------- paths

#[derive(Debug, Clone, PartialEq)]
enum Segment {
    Key(String),
    AnyKey,
    Index(usize),
    AnyItem,
}

/// `spec.containers[*].image`, `jobs.*.steps[0].uses`.
fn parse_path(path: &str) -> Result<Vec<Segment>, LawError> {
    let mut segments = Vec::new();
    for part in path.split('.') {
        let (key, mut rest) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        match key {
            "" if rest.is_empty() => return invalid(format!("empty segment in path `{path}`")),
            "" => {}
            "*" => segments.push(Segment::AnyKey),
            k => segments.push(Segment::Key(k.to_string())),
        }
        while !rest.is_empty() {
            let Some(close) = rest.find(']') else {
                return invalid(format!("unclosed `[` in path `{path}`"));
            };
            let inner = &rest[1..close];
            segments.push(match inner {
                "*" => Segment::AnyItem,
                n => Segment::Index(
                    n.parse()
                        .or_else(|_| invalid(format!("bad index `[{n}]` in path `{path}`")))?,
                ),
            });
            rest = &rest[close + 1..];
        }
    }
    Ok(segments)
}

enum Reached<'a> {
    Found(&'a CNode),
    /// The path stopped at `at`, which has no `key`.
    Missing {
        at: &'a CNode,
        key: String,
    },
}

fn resolve<'a>(node: &'a CNode, path: &[Segment]) -> Vec<Reached<'a>> {
    let Some((first, rest)) = path.split_first() else {
        return vec![Reached::Found(node)];
    };
    match (first, &node.value) {
        (Segment::Key(k), CValue::Map(entries)) => match entries.iter().find(|(key, _)| key == k) {
            Some((_, child)) => resolve(child, rest),
            None => vec![Reached::Missing {
                at: node,
                key: k.clone(),
            }],
        },
        (Segment::Key(k), _) => vec![Reached::Missing {
            at: node,
            key: k.clone(),
        }],
        (Segment::AnyKey, CValue::Map(entries)) => entries
            .iter()
            .flat_map(|(_, child)| resolve(child, rest))
            .collect(),
        (Segment::AnyItem, CValue::Seq(items)) => {
            items.iter().flat_map(|c| resolve(c, rest)).collect()
        }
        (Segment::Index(i), CValue::Seq(items)) => match items.get(*i) {
            Some(child) => resolve(child, rest),
            None => vec![Reached::Missing {
                at: node,
                key: format!("[{i}]"),
            }],
        },
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------------------- documents with spans

/// A YAML / JSON value that remembers where it is in the file.
#[derive(Debug)]
struct CNode {
    value: CValue,
    start: usize,
    end: usize,
}

#[derive(Debug)]
enum CValue {
    Map(Vec<(String, CNode)>),
    Seq(Vec<CNode>),
    Scalar(String),
    Null,
}

impl CNode {
    fn scalar(&self) -> Option<&str> {
        match &self.value {
            CValue::Scalar(s) => Some(s),
            _ => None,
        }
    }

    fn at(n: &TsNode, value: CValue) -> CNode {
        let r = n.range();
        CNode {
            value,
            start: r.start,
            end: r.end,
        }
    }
}

fn documents(root: &TsNode, lang: SupportLang) -> Vec<CNode> {
    let docs: Vec<TsNode> = match root.kind().as_ref() {
        "stream" => root.children().filter(|c| c.kind() == "document").collect(),
        _ => vec![root.clone()],
    };
    docs.iter()
        .map(|d| match first_content(d) {
            Some(c) => convert(&c, lang),
            None => CNode::at(d, CValue::Null),
        })
        .collect()
}

/// The value inside a wrapper node (document, block_node, flow_node), skipping tags, anchors and
/// comments.
fn first_content<'r>(n: &TsNode<'r>) -> Option<TsNode<'r>> {
    n.children()
        .find(|c| c.is_named() && !matches!(c.kind().as_ref(), "tag" | "anchor" | "comment"))
}

fn convert(n: &TsNode, lang: SupportLang) -> CNode {
    let kind = n.kind();
    let value = match kind.as_ref() {
        "document" | "block_node" | "flow_node" => {
            return match first_content(n) {
                Some(c) => convert(&c, lang),
                None => CNode::at(n, CValue::Null),
            };
        }
        // YAML
        "block_mapping" | "flow_mapping" => CValue::Map(
            n.children()
                .filter(|c| matches!(c.kind().as_ref(), "block_mapping_pair" | "flow_pair"))
                .map(|pair| pair_entry(&pair, lang))
                .collect(),
        ),
        "block_sequence" => CValue::Seq(
            n.children()
                .filter(|c| c.kind() == "block_sequence_item")
                .map(|item| match first_content(&item) {
                    Some(c) => convert(&c, lang),
                    None => CNode::at(&item, CValue::Null),
                })
                .collect(),
        ),
        "flow_sequence" => CValue::Seq(
            n.children()
                .filter(|c| c.is_named() && c.kind() != "comment")
                .map(|c| convert(&c, lang))
                .collect(),
        ),
        "plain_scalar" => match n.text().trim() {
            "null" | "Null" | "NULL" | "~" | "" => CValue::Null,
            t => CValue::Scalar(t.to_string()),
        },
        "double_quote_scalar" => CValue::Scalar(unquote_double(&n.text())),
        "single_quote_scalar" => {
            let t = n.text();
            CValue::Scalar(t[1..t.len().saturating_sub(1).max(1)].replace("''", "'"))
        }
        "block_scalar" => CValue::Scalar(block_scalar(&n.text())),
        // JSON
        "object" => CValue::Map(
            n.children()
                .filter(|c| c.kind() == "pair")
                .map(|pair| pair_entry(&pair, lang))
                .collect(),
        ),
        "array" => CValue::Seq(
            n.children()
                .filter(|c| c.is_named() && c.kind() != "comment")
                .map(|c| convert(&c, lang))
                .collect(),
        ),
        "string" => CValue::Scalar(unquote_double(&n.text())),
        "null" => CValue::Null,
        _ => CValue::Scalar(n.text().trim().to_string()),
    };
    CNode::at(n, value)
}

fn pair_entry(pair: &TsNode, lang: SupportLang) -> (String, CNode) {
    let named: Vec<TsNode> = pair
        .children()
        .filter(|c| c.is_named() && c.kind() != "comment")
        .collect();
    let key = pair.field("key").or_else(|| named.first().cloned());
    let value = pair.field("value").or_else(|| named.get(1).cloned());
    let key = key
        .map(|k| match convert(&k, lang).value {
            CValue::Scalar(s) => s,
            _ => k.text().trim().to_string(),
        })
        .unwrap_or_default();
    let value = match value {
        Some(v) => convert(&v, lang),
        // `key:` with nothing after it: null, pointing at the pair.
        None => CNode::at(pair, CValue::Null),
    };
    (key, value)
}

fn unquote_double(t: &str) -> String {
    let inner = t
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// `|` / `>` blocks: the lines after the indicator, without their common indentation.
fn block_scalar(t: &str) -> String {
    let lines: Vec<&str> = t.lines().skip(1).collect();
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| l.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn debug_tree(n: &CNode) -> std::collections::BTreeMap<String, String> {
    // Handy while writing tests: flattens a document into `path -> scalar`.
    fn walk(n: &CNode, prefix: String, out: &mut std::collections::BTreeMap<String, String>) {
        match &n.value {
            CValue::Map(es) => es
                .iter()
                .for_each(|(k, c)| walk(c, format!("{prefix}.{k}"), out)),
            CValue::Seq(is) => is
                .iter()
                .enumerate()
                .for_each(|(i, c)| walk(c, format!("{prefix}[{i}]"), out)),
            CValue::Scalar(s) => {
                out.insert(prefix, s.clone());
            }
            CValue::Null => {
                out.insert(prefix, "null".into());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(n, String::new(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast_grep_core::tree_sitter::LanguageExt;

    fn flat(lang: SupportLang, src: &str) -> Vec<std::collections::BTreeMap<String, String>> {
        let root = lang.ast_grep(src);
        documents(&root.root(), lang)
            .iter()
            .map(debug_tree)
            .collect()
    }

    #[test]
    fn yaml_documents() {
        let docs = flat(
            SupportLang::Yaml,
            "kind: Deployment\nspec:\n  containers:\n    - name: api\n      image: \"shop:1.2\"\n      ports: [80, 443]\n---\nkind: Service\nmeta: {a: 1, 'b': x}\nempty:\n",
        );
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0][".kind"], "Deployment");
        assert_eq!(docs[0][".spec.containers[0].image"], "shop:1.2");
        assert_eq!(docs[0][".spec.containers[0].ports[1]"], "443");
        assert_eq!(docs[1][".meta.b"], "x");
        assert_eq!(docs[1][".empty"], "null");
    }

    #[test]
    fn json_document() {
        let docs = flat(SupportLang::Json, r#"{"a": [1, {"b": "x\"y"}], "c": null}"#);
        assert_eq!(docs[0][".a[1].b"], "x\"y");
        assert_eq!(docs[0][".c"], "null");
    }

    #[test]
    fn paths() {
        use Segment::*;
        assert_eq!(
            parse_path("jobs.*.steps[*].uses").unwrap(),
            vec![
                Key("jobs".into()),
                AnyKey,
                Key("steps".into()),
                AnyItem,
                Key("uses".into())
            ]
        );
        assert_eq!(
            parse_path("a[0][1]").unwrap(),
            vec![Key("a".into()), Index(0), Index(1)]
        );
        assert!(parse_path("a..b").is_err());
    }
}
