//! Diagnostics shared by parser, timing, lint and compiler. Every diagnostic carries a
//! machine-readable code, a message and a concrete fix hint (one line an agent can apply).
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diag {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub hint: String,
    /// 1-based source line/col (0 = unknown).
    pub line: u32,
    pub col: u32,
    /// Element id concerned, if any (without '#').
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Video time in seconds the problem shows up at, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    /// Output format the problem applies to (e.g. "9:16"), if format specific.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl Diag {
    pub fn new(sev: Severity, code: &str, message: impl Into<String>, hint: impl Into<String>, span: crate::ast::Span) -> Self {
        Diag { code: code.into(), severity: sev, message: message.into(), hint: hint.into(), line: span.line, col: span.col, id: None, at: None, format: None }
    }
    pub fn error(code: &str, message: impl Into<String>, hint: impl Into<String>, span: crate::ast::Span) -> Self {
        Self::new(Severity::Error, code, message, hint, span)
    }
    pub fn warn(code: &str, message: impl Into<String>, hint: impl Into<String>, span: crate::ast::Span) -> Self {
        Self::new(Severity::Warning, code, message, hint, span)
    }
    pub fn info(code: &str, message: impl Into<String>, hint: impl Into<String>, span: crate::ast::Span) -> Self {
        Self::new(Severity::Info, code, message, hint, span)
    }
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
    pub fn with_at(mut self, t: f64) -> Self {
        self.at = Some((t * 1000.0).round() / 1000.0);
        self
    }
}
