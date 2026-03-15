//! # Idiomatic Rust Pass
//!
//! Source-level transformations converting verbose C-style Rust into idiomatic
//! Rust.  Every transform is a pure `fn(&str) -> String` that operates on
//! source text and is only applied *after* a successful `rustc` compilation,
//! so correctness is always verified.

use regex::Regex;

/// Apply all idiomatic transforms in sequence.
pub fn apply_all(code: &str) -> String {
    let mut out = code.replace("\r\n", "\n");

    // ── Structural / expression simplifications ─────────────────────────────
    out = compound_assignment(&out);
    out = simplify_index_cast(&out);
    out = println_empty_string(&out);
    out = bool_comparison_simplify(&out);
    out = remove_redundant_block(&out);

    // ── Control-flow idioms ──────────────────────────────────────────────────
    out = while_true_to_loop(&out);
    // Run for_range transforms twice so nested loops get converted on the
    // second pass (outer loop is handled first, then inner loop is exposed).
    out = for_range_separated(&out);
    out = for_range_separated(&out);
    out = for_range(&out);
    out = for_range(&out);
    out = for_range_le(&out);
    out = for_range_le(&out);
    out = array_iter_read(&out);

    // ── Misc cleanup ─────────────────────────────────────────────────────────
    out = trailing_return(&out);
    out = dead_else(&out);
    out = double_semicolons(&out);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Compound assignment  x = x OP expr → x OP= expr
// ─────────────────────────────────────────────────────────────────────────────

pub fn compound_assignment(code: &str) -> String {
    let re = Regex::new(r"^(\s*)(\w+) = (\w+) ([+\-*/%]) (.+);$").unwrap();
    let mut out = Vec::new();
    let mut changed = false;
    for line in code.lines() {
        if let Some(caps) = re.captures(line) {
            let (indent, lhs, rhs_var, op, rest) =
                (&caps[1], &caps[2], &caps[3], &caps[4], &caps[5]);
            if lhs == rhs_var {
                let op2 = match op { "+" => "+=", "-" => "-=", "*" => "*=",
                                     "/" => "/=", "%" => "%=", _ => { out.push(line.to_string()); continue; } };
                out.push(format!("{}{} {} {};", indent, lhs, op2, rest));
                changed = true;
                continue;
            }
        }
        out.push(line.to_string());
    }
    if changed { out.join("\n") } else { code.to_string() }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Simplify index casts  (i) as usize → i as usize
//    and (0) as usize → 0  (literal zero cast)
// ─────────────────────────────────────────────────────────────────────────────

pub fn simplify_index_cast(code: &str) -> String {
    // (simple_ident) as usize → simple_ident as usize
    let paren_ident = Regex::new(r"\((\w+)\) as usize").unwrap();
    let out = paren_ident.replace_all(code, "$1 as usize").to_string();
    // literal (0) as usize → 0_usize  (or just 0 since context is array index)
    let zero_cast = Regex::new(r"\b0 as usize\b").unwrap();
    zero_cast.replace_all(&out, "0").to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. println!("") → println!()
// ─────────────────────────────────────────────────────────────────────────────

pub fn println_empty_string(code: &str) -> String {
    let re = Regex::new(r#"println!\(""\)"#).unwrap();
    re.replace_all(code, "println!()").to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Boolean comparison simplification
//    expr == true  → expr
//    expr != false → expr
//    expr == false → !expr   (only for simple identifiers / field accesses)
// ─────────────────────────────────────────────────────────────────────────────

pub fn bool_comparison_simplify(code: &str) -> String {
    let eq_true  = Regex::new(r"\b(\w[\w.]*)\s*==\s*true\b").unwrap();
    let ne_false = Regex::new(r"\b(\w[\w.]*)\s*!=\s*false\b").unwrap();
    let eq_false = Regex::new(r"\b(\w[\w.]*)\s*==\s*false\b").unwrap();
    let out = eq_true.replace_all(code, "$1").to_string();
    let out = ne_false.replace_all(&out, "$1").to_string();
    eq_false.replace_all(&out, "!$1").to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Remove redundant single-statement blocks: { x.push_str(...); }
//    Only for expression statements on one line inside a bare block.
// ─────────────────────────────────────────────────────────────────────────────

pub fn remove_redundant_block(code: &str) -> String {
    // `{ stmt; }` on its own line at statement level → `stmt;`
    let re = Regex::new(r"(?m)^\s*\{\s*([^{}\n]+);\s*\}\s*$").unwrap();
    re.replace_all(code, |caps: &regex::Captures| {
        let inner = caps[1].trim();
        let is_control = inner.starts_with("if ") || inner.starts_with("while ")
            || inner.starts_with("for ") || inner.starts_with("unsafe")
            || inner.starts_with("match ") || inner.starts_with("loop");
        if is_control {
            caps[0].to_string()
        } else {
            let full = caps[0].to_string();
            let indent: String = full.chars().take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            format!("{}{};", indent, inner)
        }
    }).to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. while true → loop
// ─────────────────────────────────────────────────────────────────────────────

pub fn while_true_to_loop(code: &str) -> String {
    code.lines().map(|line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("while true {") {
            let indent = &line[..line.len() - trimmed.len()];
            format!("{}loop {}", indent, &trimmed["while true ".len()..])
        } else { line.to_string() }
    }).collect::<Vec<_>>().join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. for_range_separated
//
//    Handles the C-style pattern where declaration and initialisation are split:
//
//      let mut i: i32;          ← declaration (may be several lines earlier)
//      ...
//      i = START;               ← assignment immediately before while
//      while i < END {
//          ...body...
//          i += 1;
//      }
//
//    →  for i in START..END {
//          ...body...
//       }
//    (the `let mut i` declaration line is also removed)
// ─────────────────────────────────────────────────────────────────────────────

pub fn for_range_separated(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    // assign_re: `INDENT VAR = EXPR;`  (plain assign, not let)
    let assign_re  = Regex::new(r"^(\s*)(\w+)\s*=\s*([^=][^;]*)\s*;$").unwrap();
    // while_re: `INDENT while VAR < EXPR {`
    let while_lt   = Regex::new(r"^(\s*)while\s+(\w+)\s*<\s*(.+?)\s*\{$").unwrap();
    // decl_re: `let mut VAR: TYPE;`  (no init)
    let decl_re    = Regex::new(r"^(\s*)let\s+mut\s+(\w+)\s*:\s*\w+\s*;\s*$").unwrap();

    let mut result: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let mut removed: std::collections::HashSet<usize> = Default::default();
    let mut i = 0;

    while i < result.len() {
        if removed.contains(&i) { i += 1; continue; }
        let cur_line = result[i].clone();
        let next_line = if i + 1 < result.len() { Some(result[i + 1].clone()) } else { None };

        // Look for `VAR = START;` immediately before `while VAR < END {`
        let next_caps = next_line.as_ref().and_then(|l| while_lt.captures(l.as_str()));
        if let (Some(ac), Some(wc)) = (
            assign_re.captures(&cur_line),
            next_caps,
        ) {
            let a_indent   = &ac[1].to_string();
            let a_var      = ac[2].to_string();
            let a_start    = ac[3].trim().to_string();
            let w_var      = wc[2].to_string();
            let w_end      = wc[3].trim().to_string();

            if a_var != w_var { i += 1; continue; }
            // Bail if start contains `=` (would be mis-parsed as assignment)
            if a_start.contains('=') { i += 1; continue; }

            let body_start = i + 2;
            let (incr_line, close_line) =
                find_loop_bounds(&result, body_start, &a_var);

            if let (Some(incr_idx), Some(close_idx)) = (incr_line, close_line) {
                let body_lines: Vec<String> = result[body_start..incr_idx].to_vec();
                let body_str = body_lines.join("\n");
                // Safe only if loop var not reassigned inside body
                if !body_var_reassigned(&body_str, &a_var) {
                    let range = if a_start == "0" {
                        format!("0..{}", w_end)
                    } else {
                        format!("{}..{}", a_start, w_end)
                    };
                    // Find and remove the `let mut VAR: TYPE;` declaration above
                    for k in (0..i).rev() {
                        if removed.contains(&k) { continue; }
                        let k_line = result[k].clone();
                        if let Some(dc) = decl_re.captures(&k_line) {
                            if dc[2].trim() == a_var { removed.insert(k); break; }
                        }
                        // Stop searching backwards once we hit a non-blank, non-decl line
                        // that isn't another let-decl (avoids removing wrong things)
                        let t = result[k].trim();
                        if !t.is_empty() && !t.starts_with("let ") && !t.starts_with("//") {
                            break;
                        }
                    }

                    // Replace: assign line → for, while line → (skip), body stays,
                    // incr line → (skip), close brace stays
                    result[i]     = format!("{}for {} in {} {{", a_indent, a_var, range);
                    removed.insert(i + 1); // while line
                    removed.insert(incr_idx); // i += 1 line
                    // The close_line stays as-is
                    i = close_idx + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    result.into_iter().enumerate()
        .filter(|(idx, _)| !removed.contains(idx))
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. for_range  (original: same-line init)
//
//    let mut i: T = START;
//    while i < END {
//        ...
//        i += 1;
//    }
//    →  for i in START..END { ... }
// ─────────────────────────────────────────────────────────────────────────────

pub fn for_range(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let let_re   = Regex::new(r"^(\s*)let\s+mut\s+(\w+)\s*:\s*\w+\s*=\s*(\w+)\s*;$").unwrap();
    let while_re = Regex::new(r"^(\s*)while\s+(\w+)\s*<\s*(.+?)\s*\{$").unwrap();
    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if i + 1 < lines.len() {
            if let (Some(lc), Some(wc)) =
                (let_re.captures(lines[i]), while_re.captures(lines[i + 1]))
            {
                let let_var   = &lc[2];
                let start_val = &lc[3];
                let while_var = &wc[2];
                let end_val   = wc[3].trim();
                let indent    = &lc[1];

                if let_var == while_var {
                    let body_start = i + 2;
                    let lines_owned: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
                    let (incr_line, close_line) =
                        find_loop_bounds(&lines_owned, body_start, let_var);

                    if let (Some(incr_idx), Some(close_idx)) = (incr_line, close_line) {
                        let body: Vec<&str> = lines[body_start..incr_idx].to_vec();
                        let body_str = body.join("\n");
                        if !body_var_reassigned(&body_str, let_var) {
                            let range = if start_val == "0" {
                                format!("0..{}", end_val)
                            } else {
                                format!("{}..{}", start_val, end_val)
                            };
                            result.push(format!("{}for {} in {} {{", indent, let_var, range));
                            result.extend(body.iter().map(|l| l.to_string()));
                            result.push(lines[close_idx].to_string());
                            i = close_idx + 1;
                            continue;
                        }
                    }
                }
            }
        }
        result.push(lines[i].to_string());
        i += 1;
    }
    result.join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. for_range_le — inclusive upper bound (≤ / <=)
//
//    let mut i: T = START;   or   i = START;
//    while i <= END {
//        ...
//        i += 1;
//    }
//    →  for i in START..=END { ... }
// ─────────────────────────────────────────────────────────────────────────────

pub fn for_range_le(code: &str) -> String {
    let lines: Vec<String> = code.lines().map(|l| l.to_string()).collect();
    let let_re   = Regex::new(r"^(\s*)let\s+mut\s+(\w+)\s*:\s*\w+\s*=\s*(\S+)\s*;$").unwrap();
    let assign_re= Regex::new(r"^(\s*)(\w+)\s*=\s*([^=][^;]*)\s*;$").unwrap();
    let while_re = Regex::new(r"^(\s*)while\s+(\w+)\s*<=\s*(.+?)\s*\{$").unwrap();
    let decl_re  = Regex::new(r"^(\s*)let\s+mut\s+(\w+)\s*:\s*\w+\s*;\s*$").unwrap();
    let mut result = lines.clone();
    let mut removed: std::collections::HashSet<usize> = Default::default();
    let mut i = 0;

    while i < result.len() {
        if removed.contains(&i) { i += 1; continue; }

        // Try both same-line let and separated assign
        let cur = result[i].clone();
        let detected: Option<(String, String, String, bool)> =
            if let Some(lc) = let_re.captures(&cur) {
                Some((lc[1].to_string(), lc[2].to_string(), lc[3].to_string(), true))
            } else if let Some(ac) = assign_re.captures(&cur) {
                if !ac[3].contains('=') {
                    Some((ac[1].to_string(), ac[2].to_string(), ac[3].trim().to_string(), false))
                } else { None }
            } else { None };
        let (init_indent, init_var, start_val, is_let) = match detected {
            Some(d) => d,
            None    => { i += 1; continue; }
        };

        let next = i + 1;
        if next >= result.len() { i += 1; continue; }
        let next_line = result[next].clone();
        let wc = match while_re.captures(&next_line) {
            Some(c) => c,
            None    => { i += 1; continue; }
        };
        if wc[2].trim() != init_var { i += 1; continue; }
        let end_val = wc[3].trim().to_string();
        let indent  = init_indent.clone();

        let body_start = next + 1;
        let (incr_line, close_line) = find_loop_bounds(&result, body_start, &init_var);
        if let (Some(incr_idx), Some(close_idx)) = (incr_line, close_line) {
            let body_str = result[body_start..incr_idx].join("\n");
            if !body_var_reassigned(&body_str, &init_var) {
                let range = if start_val == "0" {
                    format!("0..={}", end_val)
                } else {
                    format!("{}..={}", start_val, end_val)
                };
                if !is_let {
                    // Remove bare-decl above
                    for k in (0..i).rev() {
                        if removed.contains(&k) { continue; }
                        let k_line = result[k].clone();
                        if let Some(dc) = decl_re.captures(&k_line) {
                            if dc[2].trim() == init_var { removed.insert(k); break; }
                        }
                        let t = result[k].trim();
                        if !t.is_empty() && !t.starts_with("let ") && !t.starts_with("//") { break; }
                    }
                }
                result[i] = format!("{}for {} in {} {{", indent, init_var, range);
                removed.insert(next);     // while line
                removed.insert(incr_idx); // i += 1 line
                i = close_idx + 1;
                continue;
            }
        }
        i += 1;
    }

    result.into_iter().enumerate()
        .filter(|(idx, _)| !removed.contains(idx))
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────
// 10. array_iter_read
//
//     Convert `for i in 0..arr.len() { ... arr[i as usize] ... }` where `i`
//     is only used as a read-only index into a single array to:
//     `for (i, item) in arr.iter().enumerate() { ... item ... }`
//
//     Only applies when:
//       • the for loop is `for i in 0..EXPR.len()` (starts at 0)
//       • i is only used as `arr[i as usize]` or `arr[i]` (no writes, no other use)
//       • the array is not modified inside the body
// ─────────────────────────────────────────────────────────────────────────────

pub fn array_iter_read(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let for_re = Regex::new(
        r"^(\s*)for\s+(\w+)\s+in\s+0\.\.(\w+)\.len\(\)\s*\{$"
    ).unwrap();
    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if let Some(caps) = for_re.captures(line) {
            let indent   = caps[1].to_string();
            let idx_var  = caps[2].to_string();
            let arr_name = caps[3].to_string();

            // Find the matching close brace
            let body_start = i + 1;
            let mut depth = 1i32;
            let mut close_idx = None;
            for j in body_start..lines.len() {
                depth += lines[j].matches('{').count() as i32;
                depth -= lines[j].matches('}').count() as i32;
                if depth == 0 { close_idx = Some(j); break; }
            }

            if let Some(close) = close_idx {
                let body_lines: Vec<&str> = lines[body_start..close].to_vec();
                let body_str = body_lines.join("\n");

                // Check safety:
                // 1. idx_var only appears as arr[idx_var as usize] or arr[idx_var]
                // 2. no write to arr inside body
                let write_re = Regex::new(&format!(
                    r"\b{}\s*\[.*\]\s*=", regex::escape(&arr_name)
                )).unwrap();
                let other_use_re = Regex::new(&format!(
                    r"\b{}\b", regex::escape(&idx_var)
                )).unwrap();
                // All occurrences of idx_var must be inside arr[idx_var ...]
                let index_use_re = Regex::new(&format!(
                    r"\b{}\[\s*{}\s*(as\s+usize)?\s*\]",
                    regex::escape(&arr_name), regex::escape(&idx_var)
                )).unwrap();

                let uses: Vec<_> = other_use_re.find_iter(&body_str).collect();
                let indexed: Vec<_> = index_use_re.find_iter(&body_str).collect();

                let only_indexed = uses.len() == indexed.len() && !uses.is_empty();
                let no_writes = !write_re.is_match(&body_str);

                if only_indexed && no_writes {
                    // Substitute arr[idx_var as usize] / arr[idx_var] → item
                    let item_name = format!("_{}_item", arr_name);
                    let new_body = index_use_re.replace_all(
                        &body_str, item_name.as_str()
                    ).to_string();

                    result.push(format!(
                        "{}for {} in {}.iter() {{", indent, item_name, arr_name
                    ));
                    for bl in new_body.lines() {
                        result.push(bl.to_string());
                    }
                    result.push(lines[close].to_string());
                    i = close + 1;
                    continue;
                }
            }
        }
        result.push(line.to_string());
        i += 1;
    }
    result.join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────
// 11. trailing_return  — `return expr;` as last statement → bare expression
// ─────────────────────────────────────────────────────────────────────────────

pub fn trailing_return(code: &str) -> String {
    let re = Regex::new(r"(?m)^(    )return ([^;\{\}\n]+);\n\}").unwrap();
    re.replace_all(code, |caps: &regex::Captures| {
        format!("    {}\n}}", &caps[2])
    }).to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// 12. dead_else  — remove empty `else {}`
// ─────────────────────────────────────────────────────────────────────────────

pub fn dead_else(code: &str) -> String {
    let re = Regex::new(r" else \{\s*\}").unwrap();
    re.replace_all(code, "").to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// 13. double_semicolons
// ─────────────────────────────────────────────────────────────────────────────

pub fn double_semicolons(code: &str) -> String {
    code.replace(";;", ";")
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Find the `VAR += 1;` increment line and the matching `}` close of a while body.
/// Returns `(increment_line_index, close_brace_index)`.
fn find_loop_bounds(
    lines: &[String],
    body_start: usize,
    var: &str,
) -> (Option<usize>, Option<usize>) {
    let incr_re = Regex::new(&format!(
        r"^\s*{}\s*\+=\s*1\s*;", regex::escape(var)
    )).unwrap();
    let mut depth = 1i32;
    let mut incr_line = None;
    let mut close_line = None;

    for j in body_start..lines.len() {
        let t = lines[j].trim();
        depth += t.matches('{').count() as i32;
        depth -= t.matches('}').count() as i32;
        if depth == 0 {
            close_line = Some(j);
            // Increment must be at depth==1, immediately before the close
            if j > body_start && incr_re.is_match(&lines[j - 1]) {
                incr_line = Some(j - 1);
            }
            break;
        }
    }
    (incr_line, close_line)
}

/// Returns true if `var` appears as the LHS of an assignment inside `body`.
fn body_var_reassigned(body: &str, var: &str) -> bool {
    let re = Regex::new(&format!(
        r"\b{}\s*=\s", regex::escape(var)
    )).unwrap();
    re.is_match(body)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compound_assignment() {
        assert_eq!(compound_assignment("    x = x + 1;"), "    x += 1;");
        assert_eq!(compound_assignment("    y = x + 1;"), "    y = x + 1;");
    }

    #[test]
    fn test_simplify_index_cast() {
        assert_eq!(simplify_index_cast("arr[(i) as usize]"), "arr[i as usize]");
        assert_eq!(simplify_index_cast("arr[0 as usize]"), "arr[0]");
        // complex expr stays
        assert_eq!(simplify_index_cast("arr[(i + 1) as usize]"), "arr[(i + 1) as usize]");
    }

    #[test]
    fn test_println_empty() {
        assert_eq!(println_empty_string(r#"println!("")"#), "println!()");
        assert_eq!(println_empty_string(r#"println!("hi")"#), r#"println!("hi")"#);
    }

    #[test]
    fn test_bool_simplify() {
        assert_eq!(bool_comparison_simplify("if x == true {"), "if x {");
        assert_eq!(bool_comparison_simplify("if x != false {"), "if x {");
        assert_eq!(bool_comparison_simplify("if x == false {"), "if !x {");
    }

    #[test]
    fn test_while_true_loop() {
        assert_eq!(while_true_to_loop("    while true {"), "    loop {");
    }

    #[test]
    fn test_trailing_return() {
        let input = "    return 42;\n}";
        assert_eq!(trailing_return(input), "    42\n}");
    }

    #[test]
    fn test_dead_else() {
        assert_eq!(dead_else("} else { }"), "}");
    }

    #[test]
    fn test_for_range_basic() {
        let input = "    let mut i: i32 = 0;\n    while i < n {\n        println!(\"{}\", i);\n        i += 1;\n    }";
        let out = for_range(input);
        assert!(out.contains("for i in 0..n {"), "Got: {}", out);
        assert!(!out.contains("let mut i"));
        assert!(!out.contains("i += 1"));
    }

    #[test]
    fn test_for_range_separated() {
        let input = "    let mut i: i32;\n    i = 0;\n    while i < 5 {\n        println!(\"{}\", i);\n        i += 1;\n    }";
        let out = for_range_separated(input);
        assert!(out.contains("for i in 0..5 {"), "Got: {}", out);
        assert!(!out.contains("let mut i"));
        assert!(!out.contains("i += 1"));
        assert!(!out.contains("i = 0"));
    }

    #[test]
    fn test_for_range_le() {
        let input = "    let mut i: i32 = 1;\n    while i <= n {\n        println!(\"{}\", i);\n        i += 1;\n    }";
        let out = for_range_le(input);
        assert!(out.contains("for i in 1..=n {"), "Got: {}", out);
    }

    #[test]
    fn test_for_range_nonzero_start() {
        let input = "    let mut i: i32 = 2;\n    while i < n {\n        println!(\"{}\", i);\n        i += 1;\n    }";
        let out = for_range(input);
        assert!(out.contains("for i in 2..n {"), "Got: {}", out);
    }

    #[test]
    fn test_double_semicolons() {
        assert_eq!(double_semicolons("x = 1;;"), "x = 1;");
    }

    #[test]
    fn test_bubble_sort_pattern() {
        // The outer loop of bubble_sort: separated decl + init
        let input = concat!(
            "    let mut i: i32;\n",
            "    let mut j: i32;\n",
            "    i = 0;\n",
            "    while i < n - 1 {\n",
            "        j = 0;\n",
            "        while j < n - i - 1 {\n",
            "            println!(\"{}\", j);\n",
            "            j += 1;\n",
            "        }\n",
            "        i += 1;\n",
            "    }",
        );
        let out = for_range_separated(input);
        assert!(out.contains("for i in 0..n - 1 {"), "outer loop: Got:\n{}", out);
    }
}
