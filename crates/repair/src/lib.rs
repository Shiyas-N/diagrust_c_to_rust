//! # Automated Repair Engine (Process 0.5.2)
//!
//! Implements rule-based correction strategies driven by `rustc` diagnostics.
//! Repair rules handle:
//!  - Missing mutability (`mut` keyword)
//!  - Borrow checker errors (E0502, E0505, E0506, E0507)
//!  - Type mismatches (E0308)
//!  - Ownership violations (E0382, E0502)
//!  - Array indexing with wrong type (E0277)
//!  - Rust keyword collisions
//!  - Unused variable warnings

use regex::Regex;
use serde::Deserialize;

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostic types (parsed from rustc JSON output)
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
// Repair engine
// ─────────────────────────────────────────────────────────────────────────────

pub struct RepairEngine;

impl RepairEngine {
    /// Apply all applicable repair rules to `code` given the list of diagnostics.
    /// Returns the repaired code and a log of actions taken.
    pub fn repair(
        code: &str,
        diagnostics: &[RustcDiagnostic],
    ) -> anyhow::Result<(String, Vec<RepairAction>)> {
        let mut current = code.to_string();
        let mut actions = Vec::new();

        for diag in diagnostics {
            let err_code = diag.error_code();

            // ── Keyword collision ─────────────────────────────────────────────
            if diag.message.contains("expected identifier, found keyword") {
                if let Some(kw) = Self::extract_keyword(&diag.message) {
                    let (repaired, changed) = Self::repair_keyword_collision(&current, &kw);
                    if changed {
                        current = repaired;
                        actions.push(RepairAction {
                            rule_name: "KeywordCollision".into(),
                            description: format!("Renamed keyword '{}' to '{}_val'", kw, kw),
                            applied: true,
                        });
                    }
                }
            }

            // ── Cannot assign through shared ref (E0594) ─────────────────────
            // e.g. "cannot assign to `*x`, which is behind a `&` reference"
            // Fix: change the parameter type from `&T` to `&mut T`
            if matches!(err_code, Some("E0594")) || diag.message.contains("cannot assign to") && diag.message.contains("behind a `&` reference") {
                let (repaired, changed) = Self::repair_immutable_ref(&current, diag);
                if changed {
                    current = repaired;
                    actions.push(RepairAction {
                        rule_name: "ImmutableRef".into(),
                        description: format!("Upgraded & to &mut for pointer parameter: {}", diag.message),
                        applied: true,
                    });
                }
            }

            // ── Missing `mut` (E0596) ─────────────────────────────────────────
            if matches!(err_code, Some("E0596")) || diag.message.contains("cannot borrow") && diag.message.contains("as mutable") {
                let (repaired, changed) = Self::repair_add_mut(&current, diag);
                if changed {
                    current = repaired;
                    actions.push(RepairAction {
                        rule_name: "AddMut".into(),
                        description: "Added `mut` to borrow or binding".into(),
                        applied: true,
                    });
                }
            }

            // ── Type mismatch (E0308) ─────────────────────────────────────────
            if matches!(err_code, Some("E0308")) {
                let (repaired, changed) = Self::repair_type_mismatch(&current, diag);
                if changed {
                    current = repaired;
                    actions.push(RepairAction {
                        rule_name: "TypeMismatch".into(),
                        description: format!("Fixed type mismatch: {}", diag.message),
                        applied: true,
                    });
                }
            }

            // ── Array index must be usize (E0277) ────────────────────────────
            if matches!(err_code, Some("E0277")) && diag.message.contains("cannot be indexed") {
                let (repaired, changed) = Self::repair_array_index(&current);
                if changed {
                    current = repaired;
                    actions.push(RepairAction {
                        rule_name: "ArrayIndex".into(),
                        description: "Added `as usize` cast on array indices".into(),
                        applied: true,
                    });
                }
            }

            // ── Move / borrow checker (E0382, E0502, E0505, E0507) ────────────
            if matches!(err_code, Some("E0382") | Some("E0502") | Some("E0505") | Some("E0507")) {
                let (repaired, changed) = Self::repair_ownership(&current, diag);
                if changed {
                    current = repaired;
                    actions.push(RepairAction {
                        rule_name: "OwnershipFix".into(),
                        description: format!("Applied ownership repair for {}", err_code.unwrap_or("?")),
                        applied: true,
                    });
                }
            }

            // ── Missing return (E0308 with expected `()`) ────────────────────
            if matches!(err_code, Some("E0308"))
                && (diag.message.contains("expected `()`") || diag.message.contains("mismatched types"))
            {
                let (repaired, changed) = Self::repair_missing_return(&current);
                if changed {
                    current = repaired;
                    actions.push(RepairAction {
                        rule_name: "MissingReturn".into(),
                        description: "Added missing return statement".into(),
                        applied: true,
                    });
                }
            }

            // ── Unused variables (warning, convert to `_name`) ───────────────
            if diag.level == "warning" && diag.message.contains("unused variable") {
                if let Some(var) = Self::extract_variable(&diag.message) {
                    let (repaired, changed) = Self::repair_unused_var(&current, &var);
                    if changed {
                        current = repaired;
                        actions.push(RepairAction {
                            rule_name: "UnusedVariable".into(),
                            description: format!("Prefixed unused variable '{}' with '_'", var),
                            applied: true,
                        });
                    }
                }
            }

            // ── Lifetime elision issues (E0106) ──────────────────────────────
            if matches!(err_code, Some("E0106")) {
                let (repaired, changed) = Self::repair_lifetime_annotation(&current, diag);
                if changed {
                    current = repaired;
                    actions.push(RepairAction {
                        rule_name: "LifetimeAnnotation".into(),
                        description: "Added explicit lifetime annotation".into(),
                        applied: true,
                    });
                }
            }
        }

        Ok((current, actions))
    }

    // ── Repair implementations ────────────────────────────────────────────────

    fn repair_keyword_collision(code: &str, keyword: &str) -> (String, bool) {
        let new_name = format!("{}_val", keyword);
        let patterns: &[(&str, &str)] = &[];  // placeholder; built dynamically below
        let _ = patterns;

        let mut result = code.to_string();
        let mut changed = false;

        // Build all replacement patterns
        let replacements: Vec<(String, String)> = vec![
            // Variable declarations
            (format!("let {}: ", keyword),         format!("let {}: ", new_name)),
            (format!("let mut {}: ", keyword),     format!("let mut {}: ", new_name)),
            // Return statement
            (format!("return {};", keyword),        format!("return {};", new_name)),
            (format!(" {};", keyword),              format!(" {};", new_name)),
            // Function signature parameters: (keyword: type) and (mut keyword: type)
            (format!("({}: ", keyword),             format!("({}: ", new_name)),
            (format!("(mut {}: ", keyword),         format!("(mut {}: ", new_name)),
            (format!(", {}: ", keyword),            format!(", {}: ", new_name)),
            (format!(", mut {}: ", keyword),        format!(", mut {}: ", new_name)),
        ];

        for (from, to) in &replacements {
            if result.contains(from.as_str()) {
                result = result.replace(from.as_str(), to.as_str());
                changed = true;
            }
        }

        // Replace remaining uses in expression context (space-delimited word boundaries)
        for suffix in &[" ", ";", ")", ",", "+", "-", "*", "/", "%", "="] {
            for prefix in &[" ", "(", "=", "+", "-", "*", "/", "%", ","] {
                let from = format!("{}{}{}", prefix, keyword, suffix);
                let to   = format!("{}{}{}", prefix, new_name, suffix);
                if result.contains(&from) {
                    result = result.replace(&from, &to);
                    changed = true;
                }
            }
        }

        (result, changed)
    }

    fn repair_immutable_ref(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // Extract variable name from message: "cannot assign to `*x`, which is behind a `&` reference"
        let re = Regex::new(r"cannot assign to `\*(\w+)`").unwrap();
        if let Some(cap) = re.captures(&diag.message) {
            let var = &cap[1];
            // Replace `var: &T` with `var: &mut T` in function signature
            // Patterns: "var: &i32", "var: &f32", "var: &u8", etc.
            let ref_re = Regex::new(&format!(r"(\b{}\s*:\s*)&(\w)", regex::escape(var))).unwrap();
            if ref_re.is_match(code) {
                let repaired = ref_re.replace_all(code, format!("${{1}}&mut $2")).to_string();
                if repaired != code {
                    return (repaired, true);
                }
            }
        }
        (code.to_string(), false)
    }

    fn repair_add_mut(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // Look for the variable name mentioned in the diagnostic
        let var = Self::extract_variable_from_borrow(&diag.message);
        if let Some(var_name) = var {
            let pattern = format!("let {}: ", var_name);
            let replacement = format!("let mut {}: ", var_name);
            if code.contains(&pattern) {
                return (code.replace(&pattern, &replacement), true);
            }
        }
        (code.to_string(), false)
    }

    fn repair_type_mismatch(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // bool assigned where i32 expected
        if diag.message.contains("expected `i32`, found `bool`")
            || (diag.message.contains("mismatched types") && diag.message.contains("bool"))
        {
            let bool_pattern = Regex::new(r"let (\w+): i32 = ([^;]+[<>]=?[^;]+);").unwrap();
            if let Some(cap) = bool_pattern.captures(code) {
                let var_name = &cap[1];
                let comparison = &cap[2];
                let replacement = format!(
                    "let {}: i32 = if {} {{ 1 }} else {{ 0 }};",
                    var_name, comparison
                );
                let repaired = bool_pattern.replace(code, replacement).to_string();
                if repaired != code {
                    return (repaired, true);
                }
            }
        }
        (code.to_string(), false)
    }

    fn repair_array_index(code: &str) -> (String, bool) {
        let idx_re = Regex::new(r"\[([^\[\]]+)\]").unwrap();
        let new_code = idx_re.replace_all(code, |caps: &regex::Captures| {
            let inner = &caps[1];
            if inner.contains("as usize") || inner.parse::<usize>().is_ok() {
                format!("[{}]", inner)
            } else {
                format!("[({}) as usize]", inner)
            }
        }).to_string();
        let changed = new_code != code;
        (new_code, changed)
    }

    fn repair_ownership(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // For moved values: try inserting .clone()
        if diag.message.contains("value moved") || diag.message.contains("borrow of moved value") {
            if let Some(var) = Self::extract_variable(&diag.message) {
                // Replace uses of the variable (not its declaration) with var.clone()
                let decl_re = Regex::new(&format!(r"let (?:mut )?{}\b", regex::escape(&var))).unwrap();
                let use_re  = Regex::new(&format!(r"\b{}\b", regex::escape(&var))).unwrap();
                let mut lines: Vec<String> = code.lines().map(String::from).collect();
                let mut changed = false;
                for line in lines.iter_mut() {
                    if !decl_re.is_match(line) && use_re.is_match(line) {
                        *line = use_re.replace_all(line, format!("{}.clone()", var)).to_string();
                        changed = true;
                    }
                }
                if changed {
                    return (lines.join("\n"), true);
                }
            }
        }
        (code.to_string(), false)
    }

    fn repair_missing_return(code: &str) -> (String, bool) {
        if !code.contains("return;") {
            if let Some(pos) = code.rfind('}') {
                let mut result = code.to_string();
                result.insert_str(pos, "    return;\n");
                return (result, true);
            }
        }
        (code.to_string(), false)
    }

    fn repair_unused_var(code: &str, var: &str) -> (String, bool) {
        let pattern = format!("let {}: ", var);
        let replacement = format!("let _{}: ", var);
        if code.contains(&pattern) {
            return (code.replace(&pattern, &replacement), true);
        }
        let pattern2 = format!("let mut {}: ", var);
        let replacement2 = format!("let mut _{}: ", var);
        if code.contains(&pattern2) {
            return (code.replace(&pattern2, &replacement2), true);
        }
        (code.to_string(), false)
    }

    fn repair_lifetime_annotation(code: &str, _diag: &RustcDiagnostic) -> (String, bool) {
        // Basic: add 'static lifetime to string references that lack annotations
        if code.contains("&str") && !code.contains("&'") {
            let repaired = code.replace("&str", "&'static str");
            return (repaired, true);
        }
        (code.to_string(), false)
    }

    // ── Message parsing helpers ───────────────────────────────────────────────

    fn extract_keyword(message: &str) -> Option<String> {
        let re = Regex::new(r"found keyword `(\w+)`").unwrap();
        re.captures(message)?.get(1).map(|m| m.as_str().to_string())
    }

    fn extract_variable(message: &str) -> Option<String> {
        let re = Regex::new(r"variable:?\s+`(\w+)`").unwrap();
        re.captures(message)?.get(1).map(|m| m.as_str().to_string())
    }

    fn extract_variable_from_borrow(message: &str) -> Option<String> {
        // "cannot borrow `x` as mutable"
        let re = Regex::new(r"cannot borrow `(\w+)`").unwrap();
        re.captures(message)?.get(1).map(|m| m.as_str().to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostic parser (converts raw rustc JSON stderr → diagnostics)
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
                    "error" => errors.push(diag),
                    "warning" => warnings.push(diag),
                    _ => {}
                }
            }
        }

        (errors, warnings)
    }
}
