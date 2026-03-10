// repair/src/engine.rs
//
// Repair engine — all repair rules and helper functions.

use regex::Regex;
use crate::types::*;

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

        // ── Pass 0: Proactive structural fixes ────────────────────────────────
        let (r, c) = Self::repair_loop_var_mutability(&current);
        if c { current = r; actions.push(action("LoopVarMut", "Made for-loop variable mutable")); }

        let (r, c) = Self::repair_compound_assignments(&current);
        if c { current = r; actions.push(action("CompoundAssign", "Replaced x = x OP expr with x OP= expr")); }

        // ── Pass 1: Structural / format fixes ─────────────────────────────────
        let (r, c) = Self::repair_print_flush_semicolon(&current);
        if c { current = r; actions.push(action("PrintFlushSemicolon", "Fixed missing semicolon between print!() and io::stdout().flush()")); }

        let (r, c) = Self::repair_format_specifiers(&current);
        if c { current = r; actions.push(action("FormatSpecifiers", "Translated remaining C printf format specifiers to Rust")); }

        // ── Pass 2: Apply rustc's own suggested replacements ──────────────────
        for diag in diagnostics {
            if let Some((replacement, line_no)) = diag.suggested_replacement() {
                let (r, c) = Self::apply_suggested_replacement(&current, replacement, line_no);
                if c { current = r; actions.push(action("SuggestedReplacement", &format!("Applied rustc suggestion on line {}", line_no))); }
            }
        }

        // ── Pass 3: Per-diagnostic targeted repairs ───────────────────────────
        for diag in diagnostics {
            let err_code = diag.error_code();

            if diag.message.contains("expected identifier, found keyword") {
                if let Some(kw) = Self::extract_keyword(&diag.message) {
                    let (r, c) = Self::repair_keyword_collision(&current, &kw);
                    if c { current = r; actions.push(action("KeywordCollision", &format!("Renamed keyword '{}' to '{}_val'", kw, kw))); }
                }
            }

            if matches!(err_code, Some("E0594"))
                || (diag.message.contains("cannot assign to") && diag.message.contains("behind a `&` reference"))
            {
                let (r, c) = Self::repair_immutable_ref(&current, diag);
                if c { current = r; actions.push(action("ImmutableRef", "Upgraded & to &mut for pointer parameter")); }
            }

            if matches!(err_code, Some("E0596"))
                || (diag.message.contains("cannot borrow") && diag.message.contains("as mutable"))
            {
                let (r, c) = Self::repair_add_mut(&current, diag);
                if c { current = r; actions.push(action("AddMut", "Added `mut` to variable binding")); }

                let (r, c) = Self::repair_immutable_call_args(&current);
                if c { current = r; actions.push(action("MutCallArgs", "Changed &var to &mut var in function calls")); }
            }

            if matches!(err_code, Some("E0308")) {
                if diag.message.contains("expected `f32`") || diag.message.contains("expected `f64`") {
                    let (r, c) = Self::repair_int_to_float_literal(&current, diag);
                    if c { current = r; actions.push(action("IntToFloat", "Changed integer literal to float literal")); }
                }

                let (r, c) = Self::repair_type_mismatch(&current, diag);
                if c { current = r; actions.push(action("TypeMismatch", &format!("Fixed type mismatch: {}", &diag.message[..diag.message.len().min(80)]))); }

                let (r, c) = Self::repair_numeric_cast(&current, diag);
                if c { current = r; actions.push(action("NumericCast", "Added numeric cast for type mismatch")); }

                let (r, c) = Self::repair_slice_coercion(&current, diag);
                if c { current = r; actions.push(action("SliceCoercion", "Added &mut arr[..] coercion for slice parameter")); }
            }

            if matches!(err_code, Some("E0277"))
                && diag.message.contains("no implementation for `f")
                && diag.message.contains("/ i")
            {
                let (r, c) = Self::repair_float_int_division(&current, diag);
                if c { current = r; actions.push(action("FloatDivCast", "Added `as f32` cast on integer divisor")); }
            }

            if matches!(err_code, Some("E0277")) && diag.message.contains("cannot be indexed") {
                let (r, c) = Self::repair_array_index(&current);
                if c { current = r; actions.push(action("ArrayIndex", "Added `as usize` cast on array indices")); }
            }

            if matches!(err_code, Some("E0382")) {
                if diag.message.contains("borrow of moved value") || diag.message.contains("use of moved value") {
                    let (r, c) = Self::repair_moved_value(&current, diag);
                    if c { current = r; actions.push(action("MovedValueClone", "Added .clone() to fix use-after-move")); }
                }
            }

            if matches!(err_code, Some("E0507")) {
                let (r, c) = Self::repair_move_out_of_ref(&current, diag);
                if c { current = r; actions.push(action("MoveOutOfRef", "Added .clone() to fix move out of reference")); }
            }

            if matches!(err_code, Some("E0502") | Some("E0505")) {
                let (r, c) = Self::repair_borrow_conflict(&current, diag);
                if c { current = r; actions.push(action("BorrowConflict", "Inserted .clone() to resolve borrow conflict")); }
            }

            if matches!(err_code, Some("E0499")) {
                let (r, c) = Self::repair_double_mut_borrow(&current, diag);
                if c { current = r; actions.push(action("DoubleMutBorrow", "Resolved double mutable borrow")); }
            }

            if matches!(err_code, Some("E0425")) && diag.message.contains("cannot find value") {
                if let Some(var) = Self::extract_undefined_name(&diag.message) {
                    let (r, c) = Self::repair_hoist_let_from_unsafe(&current, &var);
                    if c {
                        current = r;
                        actions.push(action("HoistLet", &format!("Hoisted '{}' declaration out of unsafe block", var)));
                    } else {
                        let (r, c) = Self::repair_global_access(&current, &var);
                        if c { current = r; actions.push(action("GlobalAccess", &format!("Wrapped access to global '{}' in unsafe block", var))); }
                    }
                }
            }

            if matches!(err_code, Some("E0106")) {
                let (r, c) = Self::repair_lifetime_annotation(&current, diag);
                if c { current = r; actions.push(action("LifetimeAnnotation", "Added explicit lifetime annotation")); }
            }

            if matches!(err_code, Some("E0515")) {
                let (r, c) = Self::repair_return_local_ref(&current, diag);
                if c { current = r; actions.push(action("ReturnLocalRef", "Changed return type to owned value to fix E0515")); }
            }

            if diag.level == "warning" && diag.message.contains("unused variable") {
                if let Some(var) = Self::extract_variable(&diag.message) {
                    let (r, c) = Self::repair_unused_var(&current, &var);
                    if c { current = r; actions.push(action("UnusedVariable", &format!("Prefixed unused variable '{}' with '_'", var))); }
                }
            }

            if diag.level == "warning" && diag.message.contains("unused import") {
                let (r, c) = Self::repair_unused_import(&current, &diag.message);
                if c { current = r; actions.push(action("UnusedImport", "Removed unused import")); }
            }

            if diag.level == "warning" && diag.message.contains("argument never used") {
                let (r, c) = Self::repair_excess_format_args(&current, diag);
                if c { current = r; actions.push(action("ExcessFormatArgs", "Removed excess format arguments")); }
            }
        }

        // ── Pass 4: &var → &mut var for mut-param fns ─────────────────────────
        let (r, c) = Self::repair_immutable_call_args(&current);
        if c { current = r; actions.push(action("MutCallArgs", "Changed &var to &mut var in function calls for &mut parameters")); }

        Ok((current, actions))
    }

    // ── All repair helper functions ──────────────────────────────────────────

    fn apply_suggested_replacement(code: &str, replacement: &str, line_no: usize) -> (String, bool) {
        let lines: Vec<&str> = code.lines().collect();
        if line_no == 0 || line_no > lines.len() { return (code.to_string(), false); }
        if replacement.len() > 200 { return (code.to_string(), false); }
        if replacement.contains('\n') { return (code.to_string(), false); }
        let original = lines[line_no - 1];
        let _ = (original, replacement);
        (code.to_string(), false)
    }

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

    fn repair_format_specifiers(code: &str) -> (String, bool) {
        let re = Regex::new(r##"((?:println|print|eprintln|eprint)!\(")((?:[^"\\]|\\.)*)(")""##).unwrap();
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
            let call_re = Regex::new(&format!(r"({})\(([^)]*)\)", regex::escape(fn_name))).unwrap();
            let new = call_re.replace_all(&result, |caps: &regex::Captures| {
                let args = &caps[2];
                let fixed = fix_ref_to_mut_ref(args);
                if fixed != args { changed = true; format!("{}({})", &caps[1], fixed) }
                else { caps[0].to_string() }
            }).to_string();
            result = new;
        }
        (result, changed)
    }

    fn repair_global_access(code: &str, var_name: &str) -> (String, bool) {
        let upper = var_name.to_uppercase();
        if !code.contains(&format!("static mut {}:", upper)) {
            return (code.to_string(), false);
        }
        let usage_re = Regex::new(&format!(r"\b{}\b", regex::escape(var_name))).unwrap();
        let new_code = usage_re.replace_all(code, upper.as_str()).to_string();
        if new_code == code { return (code.to_string(), false); }

        let lines: Vec<&str> = new_code.lines().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            if line.contains(&upper) && !line.contains("static mut") && !line.contains("unsafe") {
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

    fn repair_moved_value(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        let var_re    = Regex::new(r"use of moved value: `(\w+)`").unwrap();
        let borrow_re = Regex::new(r"borrow of moved value: `(\w+)`").unwrap();
        let var = var_re.captures(&diag.message)
            .or_else(|| borrow_re.captures(&diag.message))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        if let Some(var_name) = var {
            return Self::clone_after_move_in_calls(code, &var_name);
        }
        (code.to_string(), false)
    }

    fn clone_after_move_in_calls(code: &str, var_name: &str) -> (String, bool) {
        let word_re = Regex::new(&format!(r"\b{}\b", regex::escape(var_name))).unwrap();
        let clone_suffix = format!("{}.clone()", var_name);
        let lines: Vec<&str> = code.lines().collect();
        let mut new_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let mut move_seen = false;
        let mut changed = false;

        for (i, line) in lines.iter().enumerate() {
            let is_decl = Regex::new(&format!(r"let\s+(mut\s+)?{}\b", regex::escape(var_name)))
                .unwrap().is_match(line);
            if is_decl { continue; }

            let in_call = line.contains('(') && word_re.is_match(line);
            if !in_call { continue; }

            if !move_seen {
                move_seen = true;
                continue;
            }

            let mut new_line = String::new();
            let mut last = 0usize;
            for m in word_re.find_iter(line) {
                let after = &line[m.end()..];
                if after.starts_with(".clone()") {
                    new_line.push_str(&line[last..m.end()]);
                    last = m.end();
                    continue;
                }
                new_line.push_str(&line[last..m.start()]);
                new_line.push_str(&clone_suffix);
                last = m.end();
                changed = true;
            }
            new_line.push_str(&line[last..]);
            new_lines[i] = new_line;
        }

        if changed { (new_lines.join("\n"), true) } else { (code.to_string(), false) }
    }

    fn insert_clone_at_line(code: &str, var_name: &str, line_no: usize) -> (String, bool) {
        let mut lines: Vec<String> = code.lines().map(String::from).collect();
        if line_no == 0 || line_no > lines.len() { return (code.to_string(), false); }
        let idx = line_no - 1;
        let line = &lines[idx];
        let word_re = Regex::new(&format!(r"\b{}\b", regex::escape(var_name))).unwrap();
        let mut new_line = String::new();
        let mut last = 0usize;
        let clone_suffix = format!("{}.clone()", var_name);
        for m in word_re.find_iter(line) {
            let after = &line[m.end()..];
            if after.starts_with(".clone()") {
                new_line.push_str(&line[last..m.end()]);
                last = m.end();
                continue;
            }
            new_line.push_str(&line[last..m.start()]);
            new_line.push_str(&clone_suffix);
            last = m.end();
        }
        new_line.push_str(&line[last..]);
        if new_line != *line {
            lines[idx] = new_line;
            return (lines.join("\n"), true);
        }
        (code.to_string(), false)
    }

    fn repair_move_out_of_ref(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        let re = Regex::new(r"cannot move out of `\*?(\w+)`").unwrap();
        if let Some(cap) = re.captures(&diag.message) {
            let var = &cap[1];
            if let Some(line_no) = diag.primary_line() {
                return Self::insert_clone_at_line(code, var, line_no);
            }
        }
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
                let line = lines[line_no - 1].clone();
                let deref_re = Regex::new(r"\*(\w+)").unwrap();
                let mut new_line = String::new();
                let mut last = 0usize;
                for m in deref_re.find_iter(&line) {
                    let after = &line[m.end()..];
                    new_line.push_str(&line[last..m.start()]);
                    if after.starts_with(".clone()") {
                        new_line.push_str(m.as_str());
                    } else {
                        let cap_name = &m.as_str()[1..];
                        new_line.push_str(&format!("(*{}).clone()", cap_name));
                    }
                    last = m.end();
                }
                new_line.push_str(&line[last..]);
                if new_line != line {
                    lines[line_no - 1] = new_line;
                    return (lines.join("\n"), true);
                }
            }
        }
        (code.to_string(), false)
    }

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

    fn repair_double_mut_borrow(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
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

    fn repair_slice_coercion(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        if diag.message.contains("expected `&mut [") {
            if let Some(line_no) = diag.primary_line() {
                let mut lines: Vec<String> = code.lines().map(String::from).collect();
                if line_no > 0 && line_no <= lines.len() {
                    let line = lines[line_no - 1].clone();
                    let re = Regex::new(r"(\w+)\(([^)]+)\)").unwrap();
                    let word_re_inner = Regex::new(r"[a-z_]\w*").unwrap();
                    let new_line = re.replace_all(&line, |caps: &regex::Captures| {
                        let args = &caps[2];
                        let mut fixed = String::new();
                        let mut last = 0usize;
                        for m in word_re_inner.find_iter(args) {
                            let before = &args[..m.start()];
                            let after  = &args[m.end()..];
                            let preceded = before.ends_with('&') || before.ends_with('*');
                            let followed = after.starts_with('[') || after.starts_with('.') || after.starts_with('(');
                            fixed.push_str(&args[last..m.start()]);
                            if !preceded && !followed { fixed.push_str("&mut "); }
                            fixed.push_str(m.as_str());
                            last = m.end();
                        }
                        fixed.push_str(&args[last..]);
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

    fn repair_return_local_ref(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
                let line = lines[line_no - 1].clone();
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

    fn repair_keyword_collision(code: &str, keyword: &str) -> (String, bool) {
        let new_name = format!("{}_val", keyword);
        let mut result = code.to_string();
        let mut changed = false;
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(keyword))).unwrap();
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
            let old_typed = format!("let {}: ", var_name);
            let new_typed = format!("let mut {}: ", var_name);
            if code.contains(&old_typed) {
                return (code.replacen(&old_typed, &new_typed, 1), true);
            }
            let old_plain = format!("let {} =", var_name);
            let new_plain = format!("let mut {} =", var_name);
            if code.contains(&old_plain) {
                return (code.replacen(&old_plain, &new_plain, 1), true);
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
        if let Some(line_no) = diag.primary_line() {
            let mut lines: Vec<String> = code.lines().map(String::from).collect();
            if line_no > 0 && line_no <= lines.len() {
                let line = lines[line_no - 1].clone();
                let fmt_re = Regex::new(r#"(?:println|print|eprintln|eprint)!\("([^"]*)"(?:,\s*([^)]*))?\\)"#).unwrap();
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
        if code.contains("&str") && !code.contains("&'") {
            let repaired = code.replace("&str", "&'_ str");
            if repaired != code { return (repaired, true); }
        }
        (code.to_string(), false)
    }

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

    fn repair_hoist_let_from_unsafe(code: &str, var: &str) -> (String, bool) {
        let lines: Vec<&str> = code.lines().collect();
        let decl_pat = format!("let {}: ", var);
        let decl_pat_mut = format!("let mut {}: ", var);

        let decl_line_idx = lines.iter().position(|l| {
            let t = l.trim();
            (t.starts_with(&decl_pat) || t.starts_with(&decl_pat_mut))
        });
        let Some(decl_idx) = decl_line_idx else { return (code.to_string(), false); };

        let mut unsafe_open_idx = None;
        let mut depth = 0i32;
        for i in (0..=decl_idx).rev() {
            let l = lines[i].trim();
            depth += l.chars().filter(|&c| c == '}').count() as i32;
            depth -= l.chars().filter(|&c| c == '{').count() as i32;
            if (l.starts_with("unsafe {") || l == "unsafe {") && depth < 0 {
                unsafe_open_idx = Some(i);
                break;
            }
        }
        let Some(unsafe_idx) = unsafe_open_idx else { return (code.to_string(), false); };

        let decl_line = lines[decl_idx];
        let hoisted_line = if decl_line.starts_with("        ") {
            format!("    {}", decl_line.trim())
        } else {
            decl_line.trim().to_string()
        };

        let mut result_lines: Vec<String> = lines.iter()
            .enumerate()
            .filter_map(|(i, &l)| if i == decl_idx { None } else { Some(l.to_string()) })
            .collect();

        let insert_at = if decl_idx < unsafe_idx { unsafe_idx } else { unsafe_idx };
        result_lines.insert(insert_at, hoisted_line);

        (result_lines.join("\n"), true)
    }

    fn repair_int_to_float_literal(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        let line_no = match diag.primary_line() { Some(l) => l, None => return (code.to_string(), false) };
        let mut lines: Vec<String> = code.lines().map(String::from).collect();
        if line_no == 0 || line_no > lines.len() { return (code.to_string(), false); }
        let line = lines[line_no - 1].clone();
        let int_re = Regex::new(r"=\s*(-?\d+);").unwrap();
        let new_line = int_re.replace_all(&line, |caps: &regex::Captures| {
            format!("= {}.0;", &caps[1])
        }).to_string();
        if new_line != line {
            lines[line_no - 1] = new_line;
            return (lines.join("\n"), true);
        }
        (code.to_string(), false)
    }

    fn repair_float_int_division(code: &str, diag: &RustcDiagnostic) -> (String, bool) {
        let line_no = match diag.primary_line() { Some(l) => l, None => return (code.to_string(), false) };
        let mut lines: Vec<String> = code.lines().map(String::from).collect();
        if line_no == 0 || line_no > lines.len() { return (code.to_string(), false); }
        let line = lines[line_no - 1].clone();
        let div_re = Regex::new(r"(\w[\w.\[\]]*)\\s*/\s*(\w+)").unwrap();
        let new_line = div_re.replace_all(&line, |caps: &regex::Captures| {
            let rhs = &caps[2];
            if rhs.ends_with("f32") || rhs.ends_with("f64") {
                caps[0].to_string()
            } else {
                format!("{} / {} as f32", &caps[1], rhs)
            }
        }).to_string();
        if new_line != line {
            lines[line_no - 1] = new_line;
            return (lines.join("\n"), true);
        }
        (code.to_string(), false)
    }

    fn repair_loop_var_mutability(code: &str) -> (String, bool) {
        let decl_re = Regex::new(r"(?m)^\s*let (\w+):").unwrap();
        let mut changed = false;
        let mut result = code.to_string();

        let candidates: Vec<String> = decl_re.captures_iter(code)
            .filter(|cap| {
                let full = cap.get(0).unwrap().as_str();
                !full.contains("let mut ")
            })
            .map(|cap| cap[1].to_string())
            .collect();

        for var in candidates {
            let assign_re = Regex::new(&format!(r"(?m)^\s+{} =", regex::escape(&var))).unwrap();
            if assign_re.is_match(code) {
                let old = format!("let {}: ", var);
                let new = format!("let mut {}: ", var);
                if result.contains(&old) {
                    result = result.replacen(&old, &new, 1);
                    changed = true;
                }
            }
        }
        (result, changed)
    }

    fn repair_compound_assignments(code: &str) -> (String, bool) {
        let mut result = code.to_string();
        let mut changed = false;
        for (op, compound) in &[("+", "+="), ("-", "-="), ("*", "*="), ("/", "/="), ("%", "%=")] {
            let pat = format!(
                r"(?m)^(\s*)(\w+) = \2 \{} ([^;\n]+);",
                op
            );
            if let Ok(re) = Regex::new(&pat) {
                let new = re.replace_all(&result, |caps: &regex::Captures| {
                    format!("{}{} {} {};", &caps[1], &caps[2], compound, &caps[3])
                }).to_string();
                if new != result { result = new; changed = true; }
            }
        }
        (result, changed)
    }
}

fn action(name: &str, desc: &str) -> RepairAction {
    RepairAction { rule_name: name.into(), description: desc.into(), applied: true }
}

/// Replace `&ident` (not `&mut`) with `&mut ident` in an argument list string.
fn fix_ref_to_mut_ref(args: &str) -> String {
    let mut result = String::with_capacity(args.len() + 16);
    let bytes = args.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' && i + 1 < bytes.len() {
            let rest = &args[i+1..];
            if rest.starts_with("mut ") {
                result.push('&');
                i += 1;
            } else {
                if rest.chars().next().map(|c| c.is_alphanumeric() || c == '_').unwrap_or(false) {
                    result.push_str("&mut ");
                    i += 1;
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
