//! Canonical two-space surface formatting.
use crate::ast::{self, Document, Node, Value};
use crate::parse::{numeric, scan, top_equal};

fn quote(s: &str) -> String {
    let mut result = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => result.push_str("\\\""), '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"), '\t' => result.push_str("\\t"),
            '\r' => result.push_str("\\r"), other => result.push(other),
        }
    }
    result.push('"');
    result
}

fn bare(s: &str, argument: bool, list: bool) -> bool {
    if s.is_empty() || s.starts_with(['[', '"']) || s.starts_with("//")
        || (argument && (s.starts_with('@') || top_equal(s).is_some() || s == "|"))
        || numeric(s) || s.contains(['\n', '\r']) {
        return false;
    }
    // A list element additionally needs protection against top-level commas.
    let statements = scan(s, |c| c == ';', true);
    let tokens = if list { scan(s, |c| c.is_whitespace() || c == ',', false) }
        else { scan(s, char::is_whitespace, false) };
    let single = |result: Result<Vec<(usize, String)>, _>| {
        result.is_ok_and(|parts| parts.len() == 1 && parts[0].1 == s)
    };
    if !single(statements) || !single(tokens) { return false; }
    // Quote strings containing whitespace or quotes outside parentheses, even
    // where a bracket group could have protected them during tokenization.
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for c in s.chars() {
        if depth == 0 && (c.is_whitespace() || c == '"') { return false; }
        if quoted {
            if escaped { escaped = false; }
            else if c == '\\' { escaped = true; }
            else if c == '"' { quoted = false; }
        } else {
            match c {
                '(' => depth += 1, ')' => depth = depth.saturating_sub(1),
                '"' => quoted = true, _ => {}
            }
        }
    }
    true
}

fn value(v: &Value, argument: bool, list: bool) -> String {
    match v {
        Value::Str(s) => if bare(s, argument, list) { s.clone() } else { quote(s) },
        Value::Num(n) => {
            let formatted = ast::fmt_num(*n);
            // fmt_num rounds to six decimals; retain additional input precision
            // when necessary for the text round-trip guarantee.
            if formatted.parse::<f64>().ok() == Some(*n) { formatted } else { n.to_string() }
        }
        Value::List(values) => format!("[{}]", values.iter().map(|v| value(v, false, true)).collect::<Vec<_>>().join(", ")),
    }
}

fn head(node: &Node) -> String {
    let mut text = node.kind.clone();
    for arg in &node.args { text.push(' '); text.push_str(&value(arg, true, false)); }
    for (key, v) in &node.props {
        text.push(' '); text.push_str(key); text.push('='); text.push_str(&value(v, false, false));
    }
    text
}

fn emit(node: &Node, level: usize, output: &mut String) {
    let indent = "  ".repeat(level);
    output.push_str(&indent);
    if node.kind == "at" && matches!(node.args.as_slice(), [Value::Str(_)]) && node.props.is_empty() && node.body.is_none() {
        let expr = node.args[0].raw();
        // Anchors always parse as strings, including numeric-looking expressions.
        let safe = !expr.is_empty() && !expr.chars().any(char::is_whitespace)
            && !expr.contains(['"', ';', '\n', '\r'])
            && scan(&expr, char::is_whitespace, false).is_ok();
        output.push('@');
        output.push_str(&if safe { expr } else { quote(&expr) });
        let inline = node.children.len() <= 3 && node.children.iter()
            .all(|child| child.children.is_empty() && child.body.is_none() && child.kind != "at");
        if inline {
            for (i, child) in node.children.iter().enumerate() {
                output.push_str(if i == 0 { " " } else { " ; " });
                output.push_str(&head(child));
            }
            output.push('\n');
            return;
        }
    } else {
        output.push_str(&head(node));
    }
    if let Some(body) = &node.body {
        output.push_str(" |\n");
        if !body.is_empty() {
            for line in body.split('\n') {
                output.push_str(&indent); output.push_str("  ");
                output.push_str(line); output.push('\n');
            }
        }
    } else {
        output.push('\n');
        for child in &node.children { emit(child, level + 1, output); }
    }
}

/// Format a document with insertion-ordered properties and a final newline.
pub fn print(doc: &Document) -> String {
    let mut result = String::new();
    for node in &doc.nodes { emit(node, 0, &mut result); }
    if result.is_empty() { result.push('\n'); }
    result
}
