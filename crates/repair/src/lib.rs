//! # Automated Repair Engine (v2.0)
//!
//! Comprehensive rule-based repair driven by rustc diagnostics.
//! Covers: ownership/move/borrow, mutability, type mismatches, lifetimes,
//! format strings, keyword collisions, global access, swap patterns.

use regex::Regex;
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
        // Also check children (hints, notes)
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
// Repair engine
// ─────────────────────────────────────────────────────────────────────────────

pub struct RepairEngine;

impl RepairEngine {
    pub fn repair(
        code: &str,
        diagnostics: &[RustcDiagnostic],
    ) -> anyhow::Result<(String, Vec<RepairAction>)> {
        let mut current = code.to_string();
        let mut actions = Vec::new();

        // ── Pass 1: Structural / format fixes (run once upfront) ──────────────

        let (r, c) = Self::repair_print_flush_semicolon(&current);
        if c { current = r; actions.push(action("PrintFlushSemicolon", "Fixed missing semicolon between print!() and io::stdout().flush()")); }

        let (r, c) = Self::repair_format_specifiers(&current);
        if c { current = r; actions.push(action("FormatSpecifiers", "Translated remaining C printf format specifiers to Rust")); }

        // ── Pass 2: Apply rustc's own suggested replacements ─────────────────
        for diag in diagnostics {
            if let Some((replacement, line_no)) = diag.suggested_replacement() {
                let (r, c) = Self::apply_suggested_replacement(&current, replacement, line_no);
                if c { current = r; actions.push(action("SuggestedReplacement", &format!("Applied rustc suggestion on line {}", line_no))); }
            }
        }

        // ── Pass 3: Per-diagnostic targeted repairs ───────────────────────────
        for diag in diagnostics {
            let err_code = diag.error_code();

            // ── Keyword collision ──────────────────────────────────────────────
            if diag.message.contains("expected identifier, found keyword") {
                if let Some(kw) = Self::extract_keyword(&diag.message) {
                    let (r, c) = Self::repair_keyword_collision(&current, &kw);
                    if c { current = r; actions.push(action("KeywordCollision", &format!("Renamed keyword '{}' to '{}_val'", kw, kw))); }
                }
            }

            // ── E0594: cannot assign through & reference → upgrade to &mut ────
            if matches!(err_code, Some("E0594"))
                || (diag.message.contains("cannot assign to") && diag.message.contains("behind a `&` reference"))
            {
                let (r, c) = Self::repair_immutable_ref(&current, diag);
                if c { current = r; actions.push(action("ImmutableRef", "Upgraded & to &mut for pointer parameter")); }
            }

            // ── E0596: cannot borrow as mutable → add mut to binding ──────────
            if matches!(err_code, Some("E0596"))
                || (diag.message.contains("cannot borrow") && diag.message.contains("as mutable"))
            {
                let (r, c) = Self::repair_add_mut(&current, diag);
                if c { current = r; actions.push(action("AddMut", "Added `mut` to variable binding")); }

                // Also fix call site: &var → &mut var
                let (r, c) = Self::repair_immutable_call_args(&current);
                if c { current = r; actions.push(action("MutCallArgs", "Changed &var to &mut var in function calls")); }
            }

            // ── E0308: type mismatch ──────────────────────────────────────────
            if matches!(err_code, Some("E0308")) {
                // Bool/i32 mismatch
                let (r, c) = Self::repair_type_mismatch(&current, diag);
                if c { current = r; actions.push(action("TypeMismatch", &format!("Fixed type mismatch: {}", &diag.message[..diag.message.len().min(80)]))); }

                // Numeric cast (float/int)
                let (r, c) = Self::repair_numeric_cast(&current, diag);
                if c { current = r; actions.push(action("NumericCast", "Added numeric cast for type mismatch")); }

                // Slice vs array mismatch: [T;N] where &mut [T] expected
                let (r, c) = Self::repair_slice_coercion(&current, diag);
                if c { current = r; actions.push(action("SliceCoercion", "Added &mut arr[..] coercion for slice parameter")); }
            }

            // ── E0277: cannot index non-usize ────────────────────────────────
            if matches!(err_code, Some("E0277")) && diag.message.contains("cannot be indexed") {
                let (r, c) = Self::repair_array_index(&current);
                if c { current = r; actions.push(action("ArrayIndex", "Added `as usize` cast on array indices")); }
            }

            // ── E0382: value used after move ─────────────────────────────────
            if matches!(err_code, Some("E0382")) {
                if diag.message.contains("borrow of moved value") || diag.message.contains("use of moved value") {
                    let (r, c) = Self::repair_moved_value(&current, diag);
                    if c { current = r; actions.push(action("MovedValueClone", "Added .clone() to fix use-after-move")); }
                }
            }

            // ── E0507: cannot move out of borrowed/shared reference ───────────
            if matches!(err_code, Some("E0507")) {
                let (r, c) = Self::repair_move_out_of_ref(&current, diag);
                if c { current = r; actions.push(action("MoveOutOfRef", "Added .clone() to fix move out of reference")); }
            }

            // ── E0502 / E0505: borrow conflicts ──────────────────────────────
            if matches!(err_code, Some("E0502") | Some("E0505")) {
                let (r, c) = Self::repair_borrow_conflict(&current, diag);
                if c { current = r; actions.push(action("BorrowConflict", "Inserted .clone() to resolve borrow conflict")); }
            }

            // ── E0499: cannot borrow as mutable more than once ────────────────
            if matches!(err_code, Some("E0499")) {
                let (r, c) = Self::repair_double_mut_borrow(&current, diag);
                if c { current = r; actions.push(action("DoubleMutBorrow", "Resolved double mutable borrow")); }
            }

            // ── E0425: cannot find value (global static mut) ─────────────────
            if matches!(err_code, Some("E0425")) && diag.message.contains("cannot find value") {
                if let Some(var) = Self::extract_undefined_name(&diag.message) {
                    let (r, c) = Self::repair_global_access(&current, &var);
                    if c { current = r; actions.push(action("GlobalAccess", &format!("Wrapped access to global '{}' in unsafe block", var))); }
                }
            }

            // ── E0106: missing lifetime ───────────────────────────────────────
            if matches!(err_code, Some("E0106")) {
                let (r, c) = Self::repair_lifetime_annotation(&current, diag);
                if c { current = r; actions.push(action("LifetimeAnnotation", "Added explicit lifetime annotation")); }
            }

            // ── E0515: return value references local ─────────────────────────
            if matches!(err_code, Some("E0515")) {
                let (r, c) = Self::repair_return_local_ref(&current, diag);
                if c { current = r; actions.push(action("ReturnLocalRef", "Changed return type to owned value to fix E0515")); }
            }

            // ── Unused variable warning ───────────────────────────────────────
            if diag.level == "warning" && diag.message.contains("unused variable") {
                if let Some(var) = Self::extract_variable(&diag.message) {
                    let (r, c) = Self::repair_unused_var(&current, &var);
                    if c { current = r; actions.push(action("UnusedVariable", &format!("Prefixed unused variable '{}' with '_'", var))); }
                }
            }

            // ── Unused import warning ─────────────────────────────────────────
            if diag.level == "warning" && diag.message.contains("unused import") {
                let (r, c) = Self::repair_unused_import(&current, &diag.message);
                if c { current = r; actions.push(action("UnusedImport", "Removed unused import")); }
            }

            // ── `argument never used` warning (leftover format specifiers) ────
            if diag.level == "warning" && diag.message.contains("argument never used") {
                // The format string still has extra arguments — strip them
                let (r, c) = Self::repair_excess_format_args(&current, diag);
                if c { current = r; actions.push(action("ExcessFormatArgs", "Removed excess format arguments")); }
            }
        }

        // ── Pass 4: Structural pass — &var → &mut var for mut-param fns ───────
        let (r, c) = Self::repair_immutable_call_args(&current);
        if c { current = r; actions.push(action("MutCallArgs", "Changed &var to &mut var in function calls for &mut parameters")); }

        Ok((current, actions))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Apply rustc's suggested replacement at a specific line
    // ─────────────────────────────────────────────────────────────────────────

    fn apply_suggested_replacement(code: &str, replacement: &str, line_no: usize) -> (String, bool) {
        let lines: Vec<&str> = code.lines().collect();
        if line_no == 0 || line_no > lines.len() { return (code.to_string(), false); }
        // rustc suggestions are often just the expression part — apply only if it's a small fix
        if replacement.len() > 200 { return (code.to_string(), false); }
        // Only apply if the replacement looks syntactically sane (no newlines)
        if replacement.contains('\n') { return (code.to_string(), false); }
        let original = lines[line_no - 1];
        // Try to find the original fragment in the line and replace it
        // This is tricky without column info - skip for now and rely on other rules
        let _ = (original, replacement);
        (code.to_string(), false)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // print!(...)⏎io:: → print!(...);⏎io::
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_print_flush_semicolon(code: &str) -> (String, bool) {
        let re = Regex::new(r"((?:print(?:ln)?|eprint(?:ln)?)!\([^)]*\))\s*\n(\s*io::)").unwrap();
        if re.is_match(code) {
            return (re.replace_all(code, "$1;\n$2").to_string(), true);
        }
        let re2 = Regex::new(r"((?:print(?:ln)?|eprint(?:ln)?)!\([^)]*\))(\s+)(io::stdout)").unwrap();
        if re2.is_match(code) {
            return (re2.replace_all(code, "$1;\n    io::stdout").to_string(), true);
        }
        (code.to_string(), false)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // C format specifiers in Rust string literals
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_format_specifiers(code: &str) -> (String, bool) {
        
        let re = Regex::new(r##"((?:println|print|eprintln|eprint)!\(")((?:[^"\\]|\\.)*)(")"##).unwrap();
        let mut changed = false;
        let result = re.replace_all(code, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let fmt    = &caps[2];
            let suffix = &caps[3];
            let translated = translate_remaining_c_fmt(fmt);
            if translated != fmt { changed = true; format!("{}{}{}", prefix, translated, suffix) }
            else { caps[0].to_string() }
        }).to_string();
        (result, changed)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // &var → &mut var at call sites for functions with &mut params
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_immutable_call_args(code: &str) -> (String, bool) {
        let mut result = code.to_string();
        let mut changed = false;

        let sig_re = Regex::new(r"pub fn (\w+)\(([^)]*)\)").unwrap();
        let mut mut_fns: Vec<String> = Vec::new();
        for cap in sig_re.captures_iter(code) {
            if cap[2].contains("&mut") {
                mut_fns.push(cap[1].to_string());
            }
        }

        for fn_name in &mut_fns {
            // Replace fn_name(&var  (not already &mut) with fn_name(&mut var
            let call_re = Regex::new(&format!(r"({})\(([^)]*)\)", regex::escape(fn_name))).unwrap();
            let new = call_re.replace_all(&result, |caps: &regex::Captures| {
                let args = &caps[2];
                // Replace bare &ident (not &mut) with &mut ident
                let fixed = fix_ref_to_mut_ref(args);
                if fixed != args { changed = true; format!("{}({})", &caps[1], fixed) }
                else { caps[0].to_string() }
            }).to_string();
            result = new;
        }
        (result, changed)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Global static mut access → unsafe block
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_global_access(code: &str, var_name: &str) -> (String, bool) {
        let upper = var_name.to_uppercase();
        if !code.contains(&format!("static mut {}:", upper)) {
            return (code.to_string(), false);
        }
        let usage_re = Regex::new(&format!(r"\b{}\b", regex::escape(var_name))).unwrap();
        let new_code = usage_re.replace_all(code, upper.as_str()).to_string();
        if new_code == code { return (code.to_string(), false); }

        // Wrap lines using UPPER_VAR in unsafe{} if not already wrapped
        let lines: Vec<&str> = new_code.lines().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            if line.contains(&upper) && !line.contains("static mut") && !line.contains("unsafe") {
                // Check if we're already inside an unsafe block (prev line has "unsafe {")
                let already_in_unsafe = i > 0 && (lines[i-1].trim() == "unsafe {" || lines[i-1].trim().starts_with("unsafe {"));
                if !already_in_unsafe {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    out.push(format!("{}unsafe {{", indent));
                    out.push(line.to_string());
                    out.push(format!("{}}}", indent));
                } else {
                    out.push(line.to_string());
                }
            } else {
                out.push(line.to_string());
            }
            i += 1;
        }
        (out.join("\n"), true)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // E0382: use of moved value → add .clone()
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_moved_value(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // Extract variable name
        let var_re = Regex::new(r"use of moved value: `(\w+)`").unwrap();
        let borrow_re = Regex::new(r"borrow of moved value: `(\w+)`").unwrap();
        let var = var_re.captures(&diag.message)
            .or_else(|| borrow_re.captures(&diag.message))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        if let Some(var_name) = var {
            if let Some(line_no) = diag.primary_line() {
                return Self::insert_clone_at_line(code, &var_name, line_no);
            }
            // Fallback: add .clone() to all uses of the variable after its declaration
            return Self::add_clone_after_first_move(code, &var_name);
        }
        (code.to_string(), false)
    }

    fn insert_clone_at_line(code: &str, var_name: &str, line_no: usize) -> (String, bool) {
        let mut lines: Vec<String> = code.lines().map(String::from).collect();
        if line_no == 0 || line_no > lines.len() { return (code.to_string(), false); }
        let idx = line_no - 1;
        let line = &lines[idx];
        let use_re = Regex::new(&format!(r"\b{}\b(?!\.clone\(\))", regex::escape(var_name))).unwrap();
        let new_line = use_re.replace_all(line, &format!("{}.clone()", var_name)).to_string();
        if new_line != *line {
            lines[idx] = new_line;
            return (lines.join("\n"), true);
        }
        (code.to_string(), false)
    }

    fn add_clone_after_first_move(code: &str, var_name: &str) -> (String, bool) {
        // Find first function call that takes the var by value (no &), add .clone() there
        let call_re = Regex::new(&format!(r"(\w+)\(([^)]*\b{}\b[^)]*)\)", regex::escape(var_name))).unwrap();
        let mut found_first = false;
        let mut changed = false;
        let result = call_re.replace_all(code, |caps: &regex::Captures| {
            if !found_first {
                found_first = true;
                return caps[0].to_string(); // skip first use (the move)
            }
            // Second and later uses: add .clone()
            let args = &caps[2];
            let fixed = Regex::new(&format!(r"\b{}\b(?!\.clone\(\))", regex::escape(var_name)))
                .unwrap()
                .replace_all(args, &format!("{}.clone()", var_name))
                .to_string();
            if fixed != args { changed = true; }
            format!("{}({})", &caps[1], fixed)
        }).to_string();
        (result, changed)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // E0507: cannot move out of `*ref` → .clone()
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_move_out_of_ref(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // Extract the expression being moved
        let re = Regex::new(r"cannot move out of `\*?(\w+)`").unwrap();
        if let Some(cap) = re.captures(&diag.message) {
            let var = &cap[1];
            if let Some(line_no) = diag.primary_line() {
                return Self::insert_clone_at_line(code, var, line_no);
            }
        }
        // Generic: add .clone() to the problematic dereference
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
                let line = lines[line_no - 1].clone();
                // Find `*ident` or bare `ident` in a non-ref position
                let deref_re = Regex::new(r"\*(\w+)(?!\.clone\(\))").unwrap();
                let new_line = deref_re.replace_all(&line, "(*$1).clone()").to_string();
                if new_line != line {
                    lines[line_no - 1] = new_line;
                    return (lines.join("\n"), true);
                }
            }
        }
        (code.to_string(), false)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // E0502/E0505: borrow conflict → clone the conflicting variable
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_borrow_conflict(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        let re = Regex::new(r"cannot borrow `(\w+)` as mutable because it is also borrowed").unwrap();
        if let Some(cap) = re.captures(&diag.message) {
            let var = cap[1].to_string();
            if let Some(line_no) = diag.primary_line() {
                return Self::insert_clone_at_line(code, &var, line_no);
            }
        }
        (code.to_string(), false)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // E0499: double mutable borrow
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_double_mut_borrow(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
                // Try to insert .clone() on the second borrow
                let re = Regex::new(r"&mut (\w+)").unwrap();
                let line = lines[line_no - 1].clone();
                let mut count = 0;
                let new_line = re.replace_all(&line, |caps: &regex::Captures| {
                    count += 1;
                    if count > 1 {
                        format!("{}.clone()", &caps[1])
                    } else {
                        caps[0].to_string()
                    }
                }).to_string();
                if new_line != line {
                    lines[line_no - 1] = new_line;
                    return (lines.join("\n"), true);
                }
            }
        }
        (code.to_string(), false)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // E0308: slice/array coercion fix
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_slice_coercion(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // "expected `&mut [i32]`, found `[i32; N]`" or "found `&mut [i32; N]`"
        if diag.message.contains("expected `&mut [") {
            if let Some(line_no) = diag.primary_line() {
                let mut lines: Vec<String> = code.lines().map(String::from).collect();
                if line_no > 0 && line_no <= lines.len() {
                    let line = lines[line_no - 1].clone();
                    // Replace bare `arr` with `&mut arr` at call site, or `arr` with `&mut arr[..]`
                    // Look for function calls in the line
                    let re = Regex::new(r"(\w+)\(([^)]+)\)").unwrap();
                    let new_line = re.replace_all(&line, |caps: &regex::Captures| {
                        let args = &caps[2];
                        // Add &mut to array arguments not already with &mut
                        let fixed = Regex::new(r"(?<![&*])(\b[a-z_]\w*\b)(?!\[|\.|\()")
                            .unwrap()
                            .replace_all(args, "&mut $1")
                            .to_string();
                        if fixed != args { format!("{}({})", &caps[1], fixed) }
                        else { caps[0].to_string() }
                    }).to_string();
                    if new_line != line {
                        lines[line_no - 1] = new_line;
                        return (lines.join("\n"), true);
                    }
                }
            }
        }
        (code.to_string(), false)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // E0515: cannot return value referencing local variable
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_return_local_ref(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
                let line = lines[line_no - 1].clone();
                // Add .to_owned() or .clone() to the return value
                let ret_re = Regex::new(r"return (&\w[^;]*)").unwrap();
                let new_line = ret_re.replace_all(&line, "return ($1).to_owned()").to_string();
                if new_line != line {
                    lines[line_no - 1] = new_line;
                    return (lines.join("\n"), true);
                }
            }
        }
        (code.to_string(), false)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Existing helpers (improved)
    // ─────────────────────────────────────────────────────────────────────────

    fn repair_keyword_collision(code: &str, keyword: &str) -> (String, bool) {
        let new_name = format!("{}_val", keyword);
        let mut result = code.to_string();
        let mut changed = false;
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(keyword))).unwrap();
        // Only replace in non-keyword contexts (not in fn/use/type positions)
        let new = re.replace_all(&result, &new_name as &str).to_string();
        if new != result { result = new; changed = true; }
        (result, changed)
    }

    fn repair_immutable_ref(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        let re = Regex::new(r"cannot assign to `\*(\w+)`").unwrap();
        if let Some(cap) = re.captures(&diag.message) {
            let var = &cap[1];
            let ref_re = Regex::new(&format!(r"(\b{}\s*:\s*)&(\w)", regex::escape(var))).unwrap();
            if ref_re.is_match(code) {
                let repaired = ref_re.replace_all(code, "${1}&mut $2").to_string();
                if repaired != code { return (repaired, true); }
            }
        }
        (code.to_string(), false)
    }

    fn repair_add_mut(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        if let Some(var_name) = Self::extract_variable_from_borrow(&diag.message) {
            let old_pattern = format!("let {}: ", var_name);
            let new_pattern = format!("let mut {}: ", var_name);
            if code.contains(&old_pattern) {
                return (code.replace(&old_pattern, &new_pattern), true);
            }
        }
        (code.to_string(), false)
    }

    fn repair_type_mismatch(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        if diag.message.contains("expected `i32`, found `bool`")
            || (diag.message.contains("mismatched types") && diag.message.contains("bool"))
        {
            let bool_pattern = Regex::new(r"let (\w+): i32 = ([^;]+[<>]=?[^;]+);").unwrap();
            if let Some(cap) = bool_pattern.captures(code) {
                let var_name  = &cap[1];
                let comparison = &cap[2];
                let replacement = format!("let {}: i32 = if {} {{ 1 }} else {{ 0 }};", var_name, comparison);
                let repaired = bool_pattern.replace(code, replacement).to_string();
                if repaired != code { return (repaired, true); }
            }
        }
        (code.to_string(), false)
    }

    fn repair_numeric_cast(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        let expected_re = Regex::new(r"expected `(f32|f64|i32|i64|u32|u64)`, found `(f32|f64|i32|i64|u32|u64)`").unwrap();
        if let (Some(caps), Some(line_no)) = (expected_re.captures(&diag.message), diag.primary_line()) {
            let to_type = caps[1].to_string();
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if let Some(line) = lines.get(line_no.saturating_sub(1)) {
                let cast_re = Regex::new(r"(\d+(?:\.\d+)?)(\s*[;,)])").unwrap();
                let new_line = cast_re.replace(line, |c: &regex::Captures| {
                    format!("{} as {}{}", &c[1], to_type, &c[2])
                }).to_string();
                if new_line != *line {
                    let new_line = new_line.clone();
                    lines[line_no - 1] = new_line;
                    return (lines.join("\n"), true);
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

    fn repair_excess_format_args(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        // argument never used: means format string has {} placeholders but we provided more args
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
                let line = lines[line_no - 1].clone();
                // Count {} in format string vs args
                let fmt_re = Regex::new(r#"(?:println|print|eprintln|eprint)!\("([^"]*)"(?:,\s*([^)]*))?\)"#).unwrap();
                if let Some(cap) = fmt_re.captures(&line) {
                    let fmt = &cap[1];
                    let args_str = cap.get(2).map(|m| m.as_str()).unwrap_or("");
                    let placeholder_count = fmt.matches("{}").count() + fmt.matches("{:").count();
                    let args: Vec<&str> = if args_str.is_empty() { vec![] } else { args_str.split(',').collect() };
                    if args.len() > placeholder_count {
                        let trimmed_args = args[..placeholder_count].join(", ");
                        let new_line = fmt_re.replace(&line, |c: &regex::Captures| {
                            let mac = &c[0][..c[0].find('!').unwrap_or(c[0].len())];
                            if trimmed_args.is_empty() {
                                format!("{}!(\"{}\")", mac, fmt)
                            } else {
                                format!("{}!(\"{}\", {})", mac, fmt, trimmed_args)
                            }
                        }).to_string();
                        if new_line != line {
                            lines[line_no - 1] = new_line;
                            return (lines.join("\n"), true);
                        }
                    }
                }
            }
        }
        (code.to_string(), false)
    }

    fn repair_unused_var(code: &str, var: &str) -> (String, bool) {
        for (pat, rep) in &[
            (format!("let {}: ", var),     format!("let _{}: ", var)),
            (format!("let mut {}: ", var), format!("let mut _{}: ", var)),
        ] {
            if code.contains(pat.as_str()) {
                return (code.replace(pat.as_str(), rep.as_str()), true);
            }
        }
        (code.to_string(), false)
    }

    fn repair_unused_import(code: &str, msg: &str) -> (String, bool) {
        let re = Regex::new(r"unused import: `([^`]+)`").unwrap();
        if let Some(cap) = re.captures(msg) {
            let import = &cap[1];
            let use_re = Regex::new(r"use [^;]+;").unwrap();
            for m in use_re.find_iter(code) {
                let stmt = m.as_str();
                if stmt.contains(import) {
                    let new_stmt = stmt.replace(&format!(", {}", import), "")
                                      .replace(&format!("{}, ", import), "")
                                      .replace(import, "");
                    if new_stmt.contains("use ;") || new_stmt.trim() == "use ;" {
                        let result = code.replace(&format!("{}\n", stmt), "");
                        if result != code { return (result, true); }
                    }
                    let result = code.replace(stmt, &new_stmt);
                    if result != code { return (result, true); }
                }
            }
        }
        (code.to_string(), false)
    }

    fn repair_lifetime_annotation(code: &str, _diag: &RustcDiagnostic) -> (String, bool) {
        // &str params that need lifetime → &'_ str (elided lifetime)
        if code.contains("&str") && !code.contains("&'") {
            // Use elided lifetime `'_` for most cases
            let repaired = code.replace("&str", "&'_ str");
            if repaired != code { return (repaired, true); }
        }
        (code.to_string(), false)
    }

    // ── Message parsing helpers ───────────────────────────────────────────────

    fn extract_keyword(message: &str) -> Option<String> {
        Regex::new(r"found keyword `(\w+)`").unwrap()
            .captures(message)?.get(1).map(|m| m.as_str().to_string())
    }

    fn extract_variable(message: &str) -> Option<String> {
        Regex::new(r"variable:?\s+`(\w+)`").unwrap()
            .captures(message)?.get(1).map(|m| m.as_str().to_string())
    }

    fn extract_variable_from_borrow(message: &str) -> Option<String> {
        Regex::new(r"cannot borrow `(\w+)`").unwrap()
            .captures(message)?.get(1).map(|m| m.as_str().to_string())
    }

    fn extract_undefined_name(message: &str) -> Option<String> {
        Regex::new(r"cannot find value `(\w+)` in this scope").unwrap()
            .captures(message)?.get(1).map(|m| m.as_str().to_string())
    }
}

fn action(name: &str, desc: &str) -> RepairAction {
    RepairAction { rule_name: name.into(), description: desc.into(), applied: true }
}

/// Replace `&ident` (not `&mut`) with `&mut ident` in an argument list string.
fn fix_ref_to_mut_ref(args: &str) -> String {
    // Use a state machine approach to avoid regex lookahead issues
    let mut result = String::with_capacity(args.len() + 16);
    let bytes = args.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' && i + 1 < bytes.len() {
            // Check if next chars are "mut "
            let rest = &args[i+1..];
            if rest.starts_with("mut ") {
                result.push('&');
                i += 1;
            } else {
                // Check if next char is word char (an identifier follows)
                if rest.chars().next().map(|c| c.is_alphanumeric() || c == '_').unwrap_or(false) {
                    result.push_str("&mut ");
                    i += 1; // skip the &, the ident follows
                } else {
                    result.push('&');
                    i += 1;
                }
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

// ─────────────────────────────────────────────────────────────────────────────
// Format specifier translation
// ─────────────────────────────────────────────────────────────────────────────

fn translate_remaining_c_fmt(s: &str) -> String {
    let re = Regex::new(r"%[-+ 0#]*\d*(?:\.\d+)?[hlLz]*([diouxXeEfFgGcsp%])").unwrap();
    re.replace_all(s, |caps: &regex::Captures| {
        match caps[1].chars().next().unwrap_or('?') {
            'd' | 'i' | 'u' => "{}".to_string(),
            'o' => "{:o}".to_string(),
            'x' => "{:x}".to_string(),
            'X' => "{:X}".to_string(),
            'e' => "{:e}".to_string(),
            'E' => "{:E}".to_string(),
            'f' | 'F' | 'g' | 'G' => {
                let full = &caps[0];
                if let Some(dot) = full.find('.') {
                    let prec: String = full[dot+1..].chars().take_while(|c| c.is_ascii_digit()).collect();
                    if !prec.is_empty() { return format!("{{:.{}}}", prec); }
                }
                "{:.6}".to_string()
            }
            'c' | 's' => "{}".to_string(),
            'p' => "{:p}".to_string(),
            '%' => "%".to_string(),
            other => format!("{{{}}}", other),
        }
    }).to_string()
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
