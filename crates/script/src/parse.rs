//! Line-oriented surface syntax with recoverable diagnostics.
use crate::ast::{Document, Node, ParseError, Span, Value};

type Failure = (&'static str, String, &'static str);
type Parsed<T> = Result<T, Failure>;

fn failure(code: &'static str, message: impl Into<String>, hint: &'static str) -> Failure {
    (code, message.into(), hint)
}

// All offsets are UTF-8 byte offsets; convert to character columns at the edge.
pub(crate) fn scan(src: &str, separator: fn(char) -> bool, comments: bool) -> Parsed<Vec<(usize, String)>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut previous_space = true;
    let mut end = src.len();
    for (i, ch) in src.char_indices() {
        if quoted {
            if escaped { escaped = false; }
            else if ch == '\\' { escaped = true; }
            else if ch == '"' { quoted = false; }
        } else {
            if comments && stack.is_empty() && previous_space && src[i..].starts_with("//") {
                end = i;
                break;
            }
            match ch {
                '"' => quoted = true,
                '(' | '[' => {
                    if stack.len() >= 128 {
                        return Err(failure("E_DEPTH", "nesting is too deep", "use at most 128 nested levels"));
                    }
                    stack.push(if ch == '(' { ')' } else { ']' });
                },
                ')' | ']' => {
                    if stack.pop() != Some(ch) {
                        return Err(failure("E_BRACKET", "unmatched closing delimiter", "balance brackets and parentheses"));
                    }
                }
                _ => {}
            }
            if stack.is_empty() && !quoted && separator(ch) {
                if start < i { result.push((start, src[start..i].to_owned())); }
                start = i + ch.len_utf8();
            }
        }
        previous_space = ch.is_whitespace();
    }
    if quoted { return Err(failure("E_STRING", "unterminated string", "close the string with \"")); }
    if !stack.is_empty() {
        return Err(failure("E_BRACKET", "unclosed delimiter", "balance brackets and parentheses"));
    }
    if start < end { result.push((start, src[start..end].to_owned())); }
    Ok(result)
}

pub(crate) fn numeric(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let mut digits = 0;
    let mut dots = 0;
    for c in s.chars() {
        if c.is_ascii_digit() { digits += 1; }
        else if c == '.' { dots += 1; }
        else { return false; }
    }
    digits > 0 && dots <= 1
}

pub(crate) fn top_equal(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if quoted {
            if escaped { escaped = false; }
            else if c == '\\' { escaped = true; }
            else if c == '"' { quoted = false; }
        } else {
            match c {
                '"' => quoted = true,
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                '=' if depth == 0 => return Some(i),
                _ => {}
            }
        }
    }
    None
}

fn unquote(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return if chars.next().is_none() { Some(out) } else { None },
            '\\' => match chars.next()? {
                'n' => out.push('\n'), 't' => out.push('\t'), 'r' => out.push('\r'),
                '"' => out.push('"'), '\\' => out.push('\\'),
                other => { out.push('\\'); out.push(other); }
            },
            other => out.push(other),
        }
    }
    None
}

fn value(s: &str) -> Parsed<Value> {
    if let Some(text) = unquote(s) { return Ok(Value::Str(text)); }
    if s.starts_with('[') {
        let inner = s.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
            .ok_or_else(|| failure("E_BRACKET", "invalid list", "enclose the list in [ and ]"))?;
        let tokens = scan(inner, |c| c.is_whitespace() || c == ',', false)?;
        return tokens.into_iter().map(|(_, s)| value(&s)).collect::<Parsed<Vec<_>>>().map(Value::List);
    }
    if numeric(s) {
        if let Ok(n) = s.parse::<f64>() {
            if n.is_finite() { return Ok(Value::Num(n)); }
        }
        return Err(failure("E_NUMBER", "number is out of range", "use a finite number"));
    }
    Ok(Value::Str(s.to_owned()))
}

fn identifier(s: &str, key: bool) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || (key && c == '_'))
        && chars.all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c) || (key && c == ':'))
}

fn error(f: Failure, span: Span) -> ParseError {
    ParseError { code: f.0.into(), message: f.1, hint: f.2.into(), span }
}

fn position(line: usize, src: &str, offset: usize) -> Span {
    Span { line: (line + 1) as u32, col: (src[..offset].chars().count() + 1) as u32 }
}

fn statement(src: &str, span: Span, errors: &mut Vec<ParseError>) -> Parsed<Node> {
    let tokens = scan(src, char::is_whitespace, false)?;
    let kind = &tokens[0].1;
    if !identifier(kind, false) {
        let hint = if kind.starts_with('=') { "write properties as key=value after an element kind" }
            else { "start the line with an element kind or verb, e.g. `text #x \"...\"`" };
        return Err(failure("E_KIND", format!("invalid kind {kind:?}"), hint));
    }
    let mut node = Node { kind: kind.clone(), span, ..Node::default() };
    let body = tokens.last().is_some_and(|(_, s)| s == "|");
    let end = tokens.len() - usize::from(body);
    for (_, token) in &tokens[1..end.max(1)] {
        if let Some(eq) = top_equal(token) {
            let key = &token[..eq];
            if identifier(key, true) {
                if eq + 1 == token.len() {
                    return Err(failure("E_EMPTY_VALUE", format!("missing value for {key}"), "put a value after =; use \"\" for an empty string"));
                }
                let v = value(&token[eq + 1..])?;
                if node.props.insert(key.to_owned(), v).is_some() {
                    errors.push(error(failure("E_DUP_PROP", format!("duplicate property {key}"), "specify each property once"), span));
                }
                continue;
            }
        }
        node.args.push(value(token)?);
    }
    if body { node.body = Some(String::new()); }
    Ok(node)
}

// An anchor takes ownership of the remainder of the line, including semicolons.
fn statements(parts: &[(usize, String)], line: usize, full: &str, errors: &mut Vec<ParseError>, depth: usize) -> Parsed<Vec<Node>> {
    if depth >= 128 {
        return Err(failure("E_DEPTH", "anchor nesting is too deep", "use at most 128 nested levels"));
    }
    let mut nodes = Vec::new();
    for (index, (offset, part)) in parts.iter().enumerate() {
        let trimmed = part.trim();
        if trimmed.is_empty() { continue; }
        let offset = offset + part.len() - part.trim_start().len();
        let span = position(line, full, offset);
        if let Some(anchor) = trimmed.strip_prefix('@') {
            let tokens = scan(anchor, char::is_whitespace, false)?;
            let Some((_, expr)) = tokens.first() else {
                return Err(failure("E_KIND", "empty anchor", "put an expression after @"));
            };
            let mut node = Node { kind: "at".into(), args: vec![Value::Str(unquote(expr).unwrap_or_else(|| expr.clone()))], span, ..Node::default() };
            let consumed = 1 + tokens[0].0 + expr.len();
            let mut rest = vec![(offset + consumed, trimmed[consumed..].to_owned())];
            rest.extend_from_slice(&parts[index + 1..]);
            node.children = statements(&rest, line, full, errors, depth + 1)?;
            nodes.push(node);
            break;
        }
        let node = statement(trimmed, span, errors)?;
        if node.body.is_some() && parts[index + 1..].iter().any(|(_, s)| !s.trim().is_empty()) {
            return Err(failure("E_BODY", "body marker before another statement", "put | on the last statement of the line"));
        }
        nodes.push(node);
    }
    Ok(nodes)
}

fn indentation(s: &str) -> (usize, usize) {
    let mut columns = 0;
    let mut bytes = 0;
    for c in s.chars() {
        match c { ' ' => columns += 1, '\t' => columns += 2, _ => break }
        bytes += c.len_utf8();
    }
    (columns, bytes)
}

fn dedent(s: &str, amount: usize) -> String {
    let mut columns = 0;
    let mut bytes = 0;
    for c in s.chars() {
        if columns >= amount { break; }
        match c { ' ' => columns += 1, '\t' => columns += 2, _ => break }
        bytes += c.len_utf8();
    }
    format!("{}{}", " ".repeat(columns.saturating_sub(amount)), &s[bytes..])
}

fn body_target(node: &mut Node) -> Option<&mut Node> {
    if node.body.is_some() { Some(node) }
    else { node.children.last_mut().and_then(body_target) }
}

struct Entry { node: Node, children: Vec<usize> }

/// Parse surface text, recovering at the next line after a malformed statement.
pub fn parse(src: &str) -> (Document, Vec<ParseError>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut errors = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut roots = Vec::new();
    let mut levels: Vec<(usize, Option<usize>)> = vec![(0, None)];
    let mut line = 0;
    while line < lines.len() {
        let full = lines[line];
        let (indent, bytes) = indentation(full);
        let span = position(line, full, bytes);
        let parsed = scan(&full[bytes..], |c| c == ';', true).and_then(|parts| {
            let parts: Vec<_> = parts.into_iter().map(|(i, s)| (i + bytes, s)).collect();
            statements(&parts, line, full, &mut errors, 0)
        });
        let mut nodes = match parsed {
            Ok(nodes) => nodes,
            Err(f) => { errors.push(error(f, span)); line += 1; continue; }
        };
        line += 1;
        if nodes.is_empty() { continue; }
        if let Some(target) = nodes.last_mut().and_then(body_target) {
            let start = line;
            while line < lines.len() && (lines[line].trim().is_empty() || indentation(lines[line]).0 > indent) { line += 1; }
            let mut end = line;
            while end > start && lines[end - 1].trim().is_empty() { end -= 1; }
            let minimum = lines[start..end].iter().filter(|s| !s.trim().is_empty())
                .map(|s| indentation(s).0).min().unwrap_or(indent + 2);
            target.body = Some(lines[start..end].iter().map(|s| dedent(s, minimum)).collect::<Vec<_>>().join("\n"));
        }
        let previous = levels.last().map_or(0, |l| l.0);
        if indent > previous {
            if levels.len() >= 128 {
                errors.push(error(failure("E_DEPTH", "indentation nesting is too deep", "use at most 128 nested levels"), span));
                continue;
            }
            levels.push((indent, None));
        }
        else if indent < previous {
            while levels.len() > 1 && levels.last().is_some_and(|l| l.0 > indent) { levels.pop(); }
            if levels.last().is_some_and(|l| l.0 != indent) {
                errors.push(error(failure("E_INDENT", "indentation matches no open level", "indent with 2 spaces per level; this line's indent matches no parent"), span));
            }
        }
        let parent = if levels.len() > 1 { levels[levels.len() - 2].1 } else { None };
        for node in nodes {
            let id = entries.len();
            entries.push(Entry { node, children: Vec::new() });
            if let Some(parent) = parent { entries[parent].children.push(id); }
            else { roots.push(id); }
            if let Some(level) = levels.last_mut() { level.1 = Some(id); }
        }
    }
    // Assemble from the leaves up, preserving inline anchor children first.
    let mut built: Vec<Option<Node>> = vec![None; entries.len()];
    for (id, mut entry) in entries.into_iter().enumerate().rev() {
        for child in entry.children {
            if let Some(node) = built[child].take() { entry.node.children.push(node); }
        }
        built[id] = Some(entry.node);
    }
    let nodes = roots.into_iter().filter_map(|id| built[id].take()).collect();
    (Document { reel: "0".into(), nodes }, errors)
}
