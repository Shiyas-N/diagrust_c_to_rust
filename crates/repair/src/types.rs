// repair/src/types.rs
//
// Diagnostic and repair action types.

use serde::Deserialize;

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostic types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct RustcDiagnostic {
    pub message: String,
    #[serde(default)]
    pub code: Option<DiagnosticCode>,
    pub level: String,
    #[serde(default)]
    pub spans: Vec<DiagnosticSpan>,
    #[serde(default)]
    pub children: Vec<RustcDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiagnosticCode {
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiagnosticSpan {
    pub file_name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub column_start: usize,
    pub column_end: usize,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub suggested_replacement: Option<String>,
}

impl RustcDiagnostic {
    pub fn error_code(&self) -> Option<&str> {
        self.code.as_ref().map(|c| c.code.as_str())
    }
    pub fn primary_line(&self) -> Option<usize> {
        self.spans.iter().find(|s| s.is_primary).map(|s| s.line_start)
    }
    /// Get rustc's own suggested replacement if available
    pub fn suggested_replacement(&self) -> Option<(&str, usize)> {
        for span in &self.spans {
            if let Some(ref rep) = span.suggested_replacement {
                if !rep.is_empty() {
                    return Some((rep.as_str(), span.line_start));
                }
            }
        }
        for child in &self.children {
            for span in &child.spans {
                if let Some(ref rep) = span.suggested_replacement {
                    if !rep.is_empty() {
                        return Some((rep.as_str(), span.line_start));
                    }
                }
            }
        }
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Repair record
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RepairAction {
    pub rule_name: String,
    pub description: String,
    pub applied: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostic parser
// ─────────────────────────────────────────────────────────────────────────────

pub struct DiagnosticParser;

impl DiagnosticParser {
    pub fn parse(raw_stderr: &str) -> (Vec<RustcDiagnostic>, Vec<RustcDiagnostic>) {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        for line in raw_stderr.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || !trimmed.starts_with('{') { continue; }
            if let Ok(diag) = serde_json::from_str::<RustcDiagnostic>(trimmed) {
                match diag.level.as_str() {
                    "error"   => errors.push(diag),
                    "warning" => warnings.push(diag),
                    _ => {}
                }
            }
        }
        (errors, warnings)
    }
}
