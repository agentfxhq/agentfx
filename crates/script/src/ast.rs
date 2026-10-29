//! Surface AST of Reel Script. Both the indentation text form and the JSON form
//! parse to exactly this tree. It is deliberately *untyped*: a node is a kind,
//! positional args, key=value props, an optional raw body and children. All
//! typing (times, colors, ids, eases ...) happens later against the catalog
//! (`catalog.rs` / `values.rs`), which is where error messages with fix hints live.
//!
//! JSON form (1:1, empty fields omitted):
//! `{"kind":"browser","args":["#app"],"props":{"url":"studio.example.test"},"body":"...","children":[...]}`
//! A document is `{"reel":"0", "nodes":[...]}`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// 1-based source position (line, column). `line == 0` means "no source" (built from JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// A scalar or list. Text-form tokens keep their exact spelling in `Str`
/// (e.g. `0.4s`, `#17845a`, `$accent`, `@Reiter+0.2s`, `spring(170,26)`, `0s:0.85`).
/// Only bare numbers without unit become `Num`. `[a, b]` becomes `List`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Num(f64),
    Str(String),
    List(Vec<Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    /// Spelling of the value as it would appear in text form (unquoted).
    pub fn raw(&self) -> String {
        match self {
            Value::Num(n) => fmt_num(*n),
            Value::Str(s) => s.clone(),
            Value::List(v) => format!("[{}]", v.iter().map(|x| x.raw()).collect::<Vec<_>>().join(", ")),
        }
    }
}

pub fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{:.6}", n);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Node {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<Value>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub props: IndexMap<String, Value>,
    /// Raw multi-line body (`kind ... |` followed by deeper-indented lines), dedented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
    #[serde(skip)]
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Document {
    /// Format version, always "0" for spec v0.
    #[serde(default = "v0")]
    pub reel: String,
    pub nodes: Vec<Node>,
}
fn v0() -> String {
    "0".into()
}

/// A syntax error. `hint` is a one-line suggested fix an agent can apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParseError {
    pub code: String,
    pub message: String,
    pub hint: String,
    pub span: Span,
}
