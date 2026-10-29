//! Serde-compatible JSON with boolean scalar normalization.
use crate::ast::{Document, ParseError, Span};
use serde_json::Value;

fn error(code: &str, message: String, line: usize, col: usize) -> ParseError {
    ParseError {
        code: code.into(), message,
        hint: "see schema/reel-script.schema.json".into(),
        span: Span { line: line as u32, col: col as u32 },
    }
}

fn normalize(value: &mut Value) -> Result<(), ParseError> {
    match value {
        Value::Null => return Err(error("E_JSON_NULL", "null is not a Reel Script value".into(), 0, 0)),
        Value::Bool(b) => *value = Value::String(b.to_string()),
        Value::Array(items) => for item in items { normalize(item)?; },
        Value::Object(fields) => for item in fields.values_mut() { normalize(item)?; },
        _ => {}
    }
    Ok(())
}

/// Serialize the AST using its derived shape (spans are omitted).
pub fn to_json(doc: &Document) -> String {
    // Document has only string keys and serializable scalar/list values.
    serde_json::to_string_pretty(doc).expect("Document serialization is infallible")
}

/// Decode JSON, accepting booleans as their string spellings and rejecting null.
pub fn from_json(src: &str) -> Result<Document, ParseError> {
    let convert = |e: serde_json::Error| error("E_JSON", e.to_string(), e.line(), e.column());
    let mut value: Value = serde_json::from_str(src).map_err(convert)?;
    normalize(&mut value)?;
    serde_json::from_value(value).map_err(convert)
}
