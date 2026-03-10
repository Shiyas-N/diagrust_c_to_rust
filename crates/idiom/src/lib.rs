//! # Idiomatic Rust Pass
//!
//! Source-level transformations that convert verbose C-style Rust patterns
//! into idiomatic Rust. Each transform is a pure `fn(&str) -> String` that
//! operates on Rust source text.
//!
//! This pass runs **after** successful `rustc` compilation so it only makes
//! cosmetic improvements that are verified not to break anything.

use regex::Regex;

/// Apply all idiomatic transforms in sequence.
/// Returns the transformed code (unchanged if nothing applied).
pub fn apply_all(code: &str) -> String {
    // Normalize CRLF to LF — the transpiler always works with LF internally.
    let mut out = code.replace("\r\n", "\n");
    out = compound_assignment(&out);
    out = trailing_return(&out);
    out = dead_else(&out);
    out = while_true_to_loop(&out);
    out = for_range(&out);
    out = double_semicolons(&out);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Individual transforms
// ─────────────────────────────────────────────────────────────────────────────

/// `x = x OP expr;` -> `x OP= expr;`
///
/// Uses line-by-line matching instead of regex backreferences, since the Rust
/// `regex` crate does not support backreferences.
pub fn compound_assignment(code: &str) -> String {
    let assign_re = Regex::new(r"^(\s*)(\w+) = (\w+) ([+\-*/%]) (.+);$").unwrap();
    let mut lines: Vec<String> = Vec::new();
    let mut changed = false;
    for line in code.lines() {
        if let Some(caps) = assign_re.captures(line) {
            let indent = &caps[1];
            let lhs = &caps[2];
            let rhs_var = &caps[3];
            let op = &caps[4];
            let rest = &caps[5];
            if lhs == rhs_var {
                let compound = match op {
                    "+" => "+=",
                    "-" => "-=",
                    "*" => "*=",
                    "/" => "/=",
                    "%" => "%=",
                    _ => {
                        lines.push(line.to_string());
                        continue;
                    }
                };
                lines.push(format!("{}{} {} {};", indent, lhs, compound, rest));
                changed = true;
                continue;
            }
        }
        lines.push(line.to_string());
    }
    if changed {
        lines.join("\n")
    } else {
        code.to_string()
    }
}

/// `return expr;` as the last statement before `}` -> trailing expression.
pub fn trailing_return(code: &str) -> String {
    let return_re = Regex::new(r"(?m)^(    )return ([^;\{\}\n]+);\n\}").unwrap();
    return_re
        .replace_all(code, |caps: &regex::Captures| {
            format!("    {}\n}}", &caps[2])
        })
        .to_string()
}

/// Remove empty `else {}` blocks.
pub fn dead_else(code: &str) -> String {
    let empty_else = Regex::new(r" else \{\s*\}").unwrap();
    empty_else.replace_all(code, "").to_string()
}

/// `while true {` -> `loop {`
pub fn while_true_to_loop(code: &str) -> String {
    code.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("while true {") {
                let indent = &line[..line.len() - trimmed.len()];
                format!("{}loop {}", indent, &trimmed["while true ".len()..])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detect `let mut i: T = START; while i < END { ... i += 1; }` and
/// convert to `for i in START..END { ... }`.
///
/// Uses line-by-line parsing to avoid regex backreferences (which the Rust
/// `regex` crate does not support).
pub fn for_range(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    let let_re = Regex::new(r"^(\s*)let\s+mut\s+(\w+)\s*:\s*\w+\s*=\s*(\w+)\s*;$").unwrap();
    let while_re = Regex::new(r"^(\s*)while\s+(\w+)\s*<\s*(.+?)\s*\{$").unwrap();

    while i < lines.len() {
        // Look for the pattern: `let mut VAR: TYPE = START;` followed by
        // `while VAR < END {`
        if i + 1 < lines.len() {
            if let (Some(let_caps), Some(while_caps)) =
                (let_re.captures(lines[i]), while_re.captures(lines[i + 1]))
            {
                let let_indent = &let_caps[1];
                let let_var = &let_caps[2];
                let start_val = &let_caps[3];
                let while_var = &while_caps[2];
                let end_val = while_caps[3].trim();

                if let_var == while_var {
                    // Found a candidate. Now find the matching close brace and
                    // the `VAR += 1;` line just before it.
                    let incr_re = Regex::new(&format!(
                        r"^\s*{}\s*\+=\s*1\s*;",
                        regex::escape(let_var)
                    ))
                    .unwrap();

                    let body_start = i + 2;
                    let mut depth = 1i32;
                    let mut incr_line: Option<usize> = None;
                    let mut close_line: Option<usize> = None;

                    for j in body_start..lines.len() {
                        let trimmed = lines[j].trim();
                        depth += trimmed.matches('{').count() as i32;
                        depth -= trimmed.matches('}').count() as i32;
                        if depth == 0 {
                            close_line = Some(j);
                            if j > body_start && incr_re.is_match(lines[j - 1]) {
                                incr_line = Some(j - 1);
                            }
                            break;
                        }
                    }

                    if let (Some(_incr_idx), Some(close_idx)) = (incr_line, close_line) {
                        let body_end = _incr_idx;
                        // Check the body doesn't reassign the loop variable
                        let assign_re = Regex::new(&format!(
                            r"\b{}\s*=\s",
                            regex::escape(let_var)
                        ))
                        .unwrap();
                        let body_lines = &lines[body_start..body_end];
                        let body_str: String = body_lines.join("\n");
                        if !assign_re.is_match(&body_str) {
                            // Emit the for-range
                            if start_val == "0" {
                                result.push(format!(
                                    "{}for {} in 0..{} {{",
                                    let_indent, let_var, end_val
                                ));
                            } else {
                                result.push(format!(
                                    "{}for {} in {}..{} {{",
                                    let_indent, let_var, start_val, end_val
                                ));
                            }
                            for bl in body_lines {
                                result.push(bl.to_string());
                            }
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

/// Fix double semicolons left over from repair passes.
pub fn double_semicolons(code: &str) -> String {
    code.replace(";;", ";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compound_assignment() {
        assert_eq!(compound_assignment("    x = x + 1;"), "    x += 1;");
    }

    #[test]
    fn test_compound_no_match() {
        let input = "    y = x + 1;";
        assert_eq!(compound_assignment(input), input);
    }

    #[test]
    fn test_trailing_return() {
        let input = "    return 42;\n}";
        let expected = "    42\n}";
        assert_eq!(trailing_return(input), expected);
    }

    #[test]
    fn test_dead_else() {
        let input = "} else { }";
        assert_eq!(dead_else(input), "}");
    }

    #[test]
    fn test_while_true() {
        let input = "    while true {";
        let expected = "    loop {";
        assert_eq!(while_true_to_loop(input), expected);
    }

    #[test]
    fn test_double_semicolons() {
        assert_eq!(double_semicolons("x = 1;;"), "x = 1;");
    }

    #[test]
    fn test_for_range_basic() {
        let input = "    let mut i: i32 = 0;\n    while i < n {\n        println!(\"{}\", i);\n        i += 1;\n    }";
        let output = for_range(input);
        assert!(output.contains("for i in 0..n {"), "Got: {}", output);
        assert!(!output.contains("let mut i"));
        assert!(!output.contains("i += 1"));
    }
}
