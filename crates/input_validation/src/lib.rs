//! # Input Validation Module (2.0)
//!
//! Entry point of the DiagRust pipeline.  Accepts C source, performs rich
//! syntactic and semantic pre-flight checks, and returns structured feedback
//! so that both the CLI and the egui UI can display actionable diagnostics.

use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostic types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity { Info, Warning, Error }

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info    => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error   => write!(f, "error"),
        }
    }
}

/// A single structured diagnostic produced during validation.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub line:     usize,
    pub message:  String,
    pub context:  Option<String>,
    pub hint:     String,
}

impl Diagnostic {
    fn new(severity: Severity, line: usize, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { severity, line, message: message.into(), context: None, hint: hint.into() }
    }
    fn info(line: usize, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, line, message, "")
    }
    fn warning(line: usize, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(Severity::Warning, line, message, hint)
    }
    fn error(line: usize, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(Severity::Error, line, message, hint)
    }
    fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Unsupported feature at line {line}: {feature}")]
    UnsupportedFeature { line: usize, feature: String },
    #[error("Syntax error at line {line}: {message}")]
    SyntaxError { line: usize, message: String },
    #[error("Empty or invalid C source")]
    EmptySource,
    #[error("File error: {0}")]
    FileError(String),
    #[error("Preprocessor error: {0}")]
    PreprocessorError(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// ValidatedSource
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ValidatedSource {
    pub source:      String,
    pub file_path:   String,
    pub line_count:  usize,
    pub features:    Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub complexity:  u8,
    pub defines:     HashMap<String, String>,
    // Legacy fields for backward compat
    pub warnings:    Vec<String>,
}

impl ValidatedSource {
    pub fn is_ok(&self) -> bool {
        !self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }

    /// Produce a human-readable summary for the UI panel.
    pub fn summary(&self) -> String {
        let errors   = self.diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
        let warnings = self.diagnostics.iter().filter(|d| d.severity == Severity::Warning).count();
        let mut out = format!(
            "Validation: {} error(s), {} warning(s) — complexity ~{}%\n",
            errors, warnings, self.complexity
        );
        if !self.features.is_empty() {
            out.push_str(&format!("Features: {}\n", self.features.join(", ")));
        }
        for d in &self.diagnostics {
            let prefix = match d.severity {
                Severity::Error   => "✗ ERROR",
                Severity::Warning => "⚠ WARN ",
                Severity::Info    => "ℹ INFO ",
            };
            let loc = if d.line > 0 { format!(" L{}", d.line) } else { String::new() };
            out.push_str(&format!("  {}{}: {}\n", prefix, loc, d.message));
            if !d.hint.is_empty() { out.push_str(&format!("    → {}\n", d.hint)); }
            if let Some(ctx) = &d.context { out.push_str(&format!("    | {}\n", ctx)); }
        }
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InputValidator
// ─────────────────────────────────────────────────────────────────────────────

pub struct InputValidator {
    pub strict_mode: bool,
}

impl InputValidator {
    pub fn new(strict_mode: bool) -> Self { Self { strict_mode } }

    // ── Public API ────────────────────────────────────────────────────────────

    pub fn validate_file(&self, path: &Path) -> Result<ValidatedSource, ValidationError> {
        if !path.exists() {
            return Err(ValidationError::FileError(format!("File not found: {}", path.display())));
        }
        if path.extension().and_then(|e| e.to_str()) != Some("c") {
            return Err(ValidationError::FileError(format!("Expected a .c file, got: {}", path.display())));
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| ValidationError::FileError(format!("Cannot read {}: {}", path.display(), e)))?;
        self.validate_source(&raw, &path.to_string_lossy())
    }

    pub fn validate_source(&self, raw: &str, file_name: &str) -> Result<ValidatedSource, ValidationError> {
        if raw.trim().is_empty() { return Err(ValidationError::EmptySource); }

        if raw.contains('\0') {
            return Err(ValidationError::SyntaxError {
                line: 0, message: "Source contains NUL byte(s)".to_string()
            });
        }

        let line_count = raw.lines().count();
        let mut diags: Vec<Diagnostic> = Vec::new();

        // Step 1: Strip comments
        let stripped = Self::strip_comments(raw);

        // Step 2: Preprocessor
        let (source, defines, mut pp_diags) = self.handle_preprocessor(&stripped);
        diags.append(&mut pp_diags);

        // Step 3: Balance check (brace/paren/bracket)
        self.check_balance(&source, &mut diags);

        // Step 4: Function existence
        self.check_function_exists(&source, &mut diags);

        // Step 5: Unsupported features
        self.check_unsupported_features(&source, &mut diags);

        // Step 6: Portability hints
        self.check_portability(&source, &mut diags);

        // Step 7: Code quality
        self.check_quality(&source, &mut diags);

        // Step 8: Strict mode
        if self.strict_mode {
            self.check_type_mismatches(&source, &mut diags);
            self.check_missing_semicolons(&source, &mut diags);
            self.check_suspicious_patterns(&source, &mut diags);
        }

        // Step 9: Features + complexity
        let features   = self.detect_features(&source);
        let complexity = self.estimate_complexity(&source);

        // Legacy warnings vec
        let warnings: Vec<String> = diags.iter()
            .filter(|d| d.severity >= Severity::Warning)
            .map(|d| format!("[{}] line {}: {}", d.severity, d.line, d.message))
            .collect();

        // In strict mode, fail on first Error diagnostic
        if self.strict_mode {
            if let Some(d) = diags.iter().find(|d| d.severity == Severity::Error) {
                return Err(ValidationError::UnsupportedFeature {
                    line: d.line, feature: d.message.clone()
                });
            }
        }

        Ok(ValidatedSource { source, file_path: file_name.to_string(), line_count, features, diagnostics: diags, complexity, defines, warnings })
    }

    // ── Comment stripping ─────────────────────────────────────────────────────

    fn strip_comments(source: &str) -> String {
        let mut result = String::with_capacity(source.len());
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' {
                match chars.peek() {
                    Some('/') => {
                        chars.next();
                        while let Some(&ch) = chars.peek() { if ch == '\n' { break; } chars.next(); }
                    }
                    Some('*') => {
                        chars.next();
                        loop {
                            match chars.next() {
                                None => break,
                                Some('*') => { if chars.peek() == Some(&'/') { chars.next(); break; } }
                                Some('\n') => result.push('\n'),
                                _ => {}
                            }
                        }
                    }
                    _ => result.push(c),
                }
            } else if c == '"' {
                result.push(c);
                loop {
                    match chars.next() {
                        None => break,
                        Some('\\') => { result.push('\\'); if let Some(ec) = chars.next() { result.push(ec); } }
                        Some('"') => { result.push('"'); break; }
                        Some(ch) => result.push(ch),
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    // ── Preprocessor ──────────────────────────────────────────────────────────

    fn handle_preprocessor(&self, source: &str) -> (String, HashMap<String, String>, Vec<Diagnostic>) {
        let mut output  = String::new();
        let mut defines = HashMap::new();
        let mut diags   = Vec::new();

        let stdlib_headers = &[
            "stdio.h","stdlib.h","string.h","math.h","ctype.h","stdint.h",
            "stddef.h","stdbool.h","limits.h","float.h","assert.h","errno.h",
            "time.h","locale.h","signal.h","setjmp.h",
        ];
        let define_re = Regex::new(r"#define\s+(\w+)(?:\s+(.*))?").unwrap();

        for (ln, line) in source.lines().enumerate() {
            let line_1  = ln + 1;
            let trimmed = line.trim();

            if trimmed.starts_with("#include") {
                let is_stdlib = stdlib_headers.iter().any(|h| trimmed.contains(h));
                if !is_stdlib {
                    let is_sys = trimmed.contains('<') && trimmed.contains('>');
                    if is_sys {
                        diags.push(Diagnostic::warning(line_1,
                            format!("Non-standard header: {}", trimmed),
                            "Check crates.io for a Rust equivalent"));
                    } else {
                        diags.push(Diagnostic::warning(line_1,
                            format!("User #include stripped: {}", trimmed),
                            "Move declarations into this file or rely on type inference"));
                    }
                }
                output.push('\n');
                continue;
            }

            if trimmed.starts_with("#define") {
                if let Some(cap) = define_re.captures(trimmed) {
                    let name  = cap[1].to_string();
                    let value = cap.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
                    if !name.is_empty() && !value.is_empty() {
                        defines.insert(name.clone(), value.clone());
                        diags.push(Diagnostic::info(line_1, format!("#define {} = \"{}\"", name, value)));
                    } else if name.contains('(') {
                        diags.push(Diagnostic::warning(line_1,
                            format!("Function-like macro '{}' ignored", name),
                            "Convert to a Rust inline function or const fn"));
                    }
                }
                output.push('\n');
                continue;
            }

            if trimmed.starts_with("#pragma") {
                diags.push(Diagnostic::info(line_1, format!("#pragma ignored: {}", trimmed)));
                output.push('\n');
                continue;
            }

            if trimmed.starts_with('#') {
                diags.push(Diagnostic::info(line_1, format!("Directive stripped: {}", trimmed)));
                output.push('\n');
                continue;
            }

            // Expand simple object-like macros
            let mut expanded = line.to_string();
            for (name, value) in &defines {
                if !value.is_empty() {
                    let pat = format!(r"\b{}\b", regex::escape(name));
                    if let Ok(re) = Regex::new(&pat) {
                        expanded = re.replace_all(&expanded, value.as_str()).to_string();
                    }
                }
            }
            output.push_str(&expanded);
            output.push('\n');
        }

        (output, defines, diags)
    }

    // ── Balance check ─────────────────────────────────────────────────────────

    fn check_balance(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let (mut bd, mut pd, mut brd) = (0i32, 0i32, 0i32);
        let mut brace_open_line = 0;
        for (ln, line) in source.lines().enumerate() {
            let line_1 = ln + 1;
            for ch in line.chars() {
                match ch {
                    '{' => { if bd == 0 { brace_open_line = line_1; } bd += 1; }
                    '}' => { bd -= 1; if bd < 0 {
                        diags.push(Diagnostic::error(line_1, "Unmatched '}'", "Check that every '{' has a matching '}'").with_context(line.trim()));
                        bd = 0;
                    }}
                    '(' => pd += 1,
                    ')' => { pd -= 1; if pd < 0 {
                        diags.push(Diagnostic::error(line_1, "Unmatched ')'", "Check all parentheses are balanced").with_context(line.trim()));
                        pd = 0;
                    }}
                    '[' => brd += 1,
                    ']' => { brd -= 1; if brd < 0 {
                        diags.push(Diagnostic::error(line_1, "Unmatched ']'", "Check all brackets are balanced").with_context(line.trim()));
                        brd = 0;
                    }}
                    _ => {}
                }
            }
        }
        if bd > 0 { diags.push(Diagnostic::error(brace_open_line, format!("{} unclosed brace(s)", bd), "Add the missing closing '}'(s)")); }
        if pd > 0 { diags.push(Diagnostic::error(0, format!("{} unclosed parenthesis(es)", pd), "Check for-loops and function calls")); }
        if brd > 0 { diags.push(Diagnostic::error(0, format!("{} unclosed bracket(s)", brd), "Check array declarations and accesses")); }
    }

    // ── Function existence ────────────────────────────────────────────────────

    fn check_function_exists(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let func_re = Regex::new(
            r"(?m)^\s*(int|void|char|float|double|long|unsigned)\s+\w+\s*\([^;]*\)\s*\{"
        ).unwrap();
        if !func_re.is_match(source) {
            diags.push(Diagnostic::error(0,
                "No function definition found",
                "Add `int main() { ... }` or another function definition"));
        }
    }

    // ── Unsupported features ──────────────────────────────────────────────────

    fn check_unsupported_features(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let checks: &[(&str, &str, &str)] = &[
            ("goto",         "goto statement",         "Replace with structured control flow"),
            ("asm",          "inline assembly",        "Wrap in a Rust unsafe fn"),
            ("__asm__",      "inline assembly (GCC)",  "Wrap in a Rust unsafe fn"),
            ("union ",       "union type",             "Use Rust enum with data instead"),
            ("extern ",      "extern declaration",     "Use a Rust static or lazy_static!"),
            ("__attribute__","GCC attribute",          "No direct Rust equivalent"),
            ("longjmp",      "longjmp",                "Use Rust ? operator or panic!"),
            ("setjmp",       "setjmp",                 "Use Rust ? operator or panic!"),
            ("va_list",      "variadic function",      "Use a slice parameter or macro"),
            ("va_arg",       "va_arg",                 "Use a slice parameter or macro"),
            ("_Complex",     "C99 complex type",       "Use the `num-complex` crate"),
            ("_Atomic",      "C11 atomic type",        "Use std::sync::atomic"),
            ("restrict",     "restrict qualifier",     "Rust ownership handles this automatically"),
        ];
        for (ln, line) in source.lines().enumerate() {
            let t = line.trim();
            if t.is_empty() { continue; }
            for &(kw, feat, hint) in checks {
                if t.contains(kw) {
                    let d = if self.strict_mode {
                        Diagnostic::error(ln+1, format!("Unsupported: '{}'", feat), hint)
                    } else {
                        Diagnostic::warning(ln+1, format!("May cause issues: '{}'", feat), hint)
                    };
                    diags.push(d.with_context(t));
                    break;
                }
            }
        }
    }

    // ── Portability ───────────────────────────────────────────────────────────

    fn check_portability(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let checks: &[(&str, &str, &str)] = &[
            ("windows.h",  "Windows-specific header",   "May not compile on Linux/macOS"),
            ("unistd.h",   "POSIX header",              "POSIX-only; may not compile on Windows"),
            ("sys/socket", "POSIX socket API",          "Use std::net or tokio in Rust"),
            ("pthread",    "POSIX threads",             "Use std::thread in Rust"),
            ("gets(",      "gets() is banned (C11)",    "Use fgets() or Rust's read_line()"),
            ("INT_MAX",    "limits.h constant",         "Use i32::MAX in Rust"),
            ("UINT_MAX",   "limits.h constant",         "Use u32::MAX in Rust"),
            ("FLT_MAX",    "float limits constant",     "Use f32::MAX in Rust"),
        ];
        for (ln, line) in source.lines().enumerate() {
            let t = line.trim();
            for &(pat, feat, hint) in checks {
                if t.contains(pat) {
                    diags.push(Diagnostic::info(ln+1, format!("Portability: {}", feat), ).with_context(hint));
                }
            }
        }
    }

    // ── Code quality ──────────────────────────────────────────────────────────

    fn check_quality(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let long_re = Regex::new(r"^.{121,}$").unwrap();
        for (ln, line) in source.lines().enumerate() {
            if long_re.is_match(line) {
                diags.push(Diagnostic::info(ln+1,
                    format!("Long line ({} chars)", line.len())));
            }
        }
        let depth = self.max_nesting_depth(source);
        if depth > 5 {
            diags.push(Diagnostic::warning(0,
                format!("Deep nesting (max {})", depth),
                "Consider refactoring into separate functions"));
        }
    }

    // ── Strict checks ─────────────────────────────────────────────────────────

    fn check_type_mismatches(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let neg_idx_re = Regex::new(r"\[-[0-9]+\]").unwrap();
        for (ln, line) in source.lines().enumerate() {
            let t = line.trim();
            let l = ln + 1;
            if t.starts_with("int") && t.contains('=') && t.contains('"') {
                diags.push(Diagnostic::error(l, "Integer assigned string literal", "Use char* or a string type").with_context(t));
            }
            if t.starts_with("char ") && !t.contains("char*") && !t.contains("char *") && t.contains('=') && t.contains('"') {
                diags.push(Diagnostic::error(l, "char assigned string literal (use char*)", "In Rust use &str or String").with_context(t));
            }
            if let Some(idx) = t.find("/ 0") {
                if !matches!(t[idx+3..].chars().next(), Some('.'|'0'..='9')) {
                    diags.push(Diagnostic::error(l, "Potential division by zero", "Ensure divisor is non-zero").with_context(t));
                }
            }
            if neg_idx_re.is_match(t) {
                diags.push(Diagnostic::error(l, "Negative array index — UB in C, panic in Rust", "Ensure indices are always >= 0").with_context(t));
            }
        }
    }

    fn check_missing_semicolons(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let kws = ["return", "break", "continue"];
        for (ln, line) in source.lines().enumerate() {
            let t = line.trim();
            if t.is_empty() { continue; }
            if kws.iter().any(|k| t.starts_with(k))
                && !t.ends_with(';') && !t.ends_with('{') && !t.ends_with('}') && !t.ends_with(',')
            {
                diags.push(Diagnostic::warning(ln+1, format!("Possible missing semicolon: '{}'", t), "Add ';' at end of statement"));
            }
        }
    }

    fn check_suspicious_patterns(&self, source: &str, diags: &mut Vec<Diagnostic>) {
        let assign_in_cond = Regex::new(r"if\s*\([^)]*[^=!<>]=[^=][^)]*\)").unwrap();
        for (ln, line) in source.lines().enumerate() {
            let t = line.trim();
            let l = ln + 1;
            if assign_in_cond.is_match(t) {
                diags.push(Diagnostic::warning(l, "Assignment inside condition (did you mean '=='?)", "C allows this but it is almost always a bug").with_context(t));
            }
            if t.contains("scanf") && t.contains("%s") {
                diags.push(Diagnostic::warning(l, "scanf %s has no bounds check", "Use %255s or fgets() to limit input length").with_context(t));
            }
            if t.contains("gets(") {
                diags.push(Diagnostic::error(l, "gets() is unsafe and removed in C11", "Use fgets() or Rust io::stdin().read_line()").with_context(t));
            }
            if t.contains("strcpy(") {
                diags.push(Diagnostic::warning(l, "strcpy() has no bounds check", "Use strncpy() or Rust String operations").with_context(t));
            }
        }
    }

    // ── Feature detection ─────────────────────────────────────────────────────

    fn detect_features(&self, source: &str) -> Vec<String> {
        let checks: &[(&str, &str)] = &[
            ("struct ",  "structs"),
            ("*",        "pointers"),
            ("[",        "arrays"),
            ("malloc",   "dynamic allocation"),
            ("free(",    "deallocation"),
            ("int main", "executable"),
            ("for ",     "for loops"),
            ("while ",   "while loops"),
            ("do {",     "do-while loops"),
            ("if (",     "conditionals"),
            ("switch ",  "switch statements"),
            ("typedef ", "typedefs"),
            ("enum ",    "enumerations"),
            ("printf",   "formatted output"),
            ("scanf",    "formatted input"),
            ("->",       "pointer member access"),
            ("FILE",     "file I/O"),
        ];
        checks.iter()
            .filter(|(k, _)| source.contains(k))
            .map(|(_, f)| f.to_string())
            .collect()
    }

    // ── Complexity estimation ─────────────────────────────────────────────────

    fn estimate_complexity(&self, source: &str) -> u8 {
        let mut score: u32 = 0;
        let func_re = Regex::new(r"(?m)^\s*(int|void|char|float|double)\s+\w+\s*\(").unwrap();
        score += (func_re.find_iter(source).count() as u32 * 5).min(20);
        score += (source.lines().count() as u32 / 10).min(20);
        let cf_re = Regex::new(r"\b(if|for|while|do|switch|case)\b").unwrap();
        score += (cf_re.find_iter(source).count() as u32 * 3).min(30);
        if source.contains('*')      { score += 5; }
        if source.contains("struct") { score += 5; }
        if source.contains('[')      { score += 3; }
        if source.contains("malloc") { score += 5; }
        score += (self.max_nesting_depth(source) as u32 * 2).min(10);
        score.min(100) as u8
    }

    fn max_nesting_depth(&self, source: &str) -> usize {
        let (mut cur, mut max) = (0usize, 0usize);
        for ch in source.chars() {
            match ch {
                '{' => { cur += 1; if cur > max { max = cur; } }
                '}' => { if cur > 0 { cur -= 1; } }
                _ => {}
            }
        }
        max
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Convenience
// ─────────────────────────────────────────────────────────────────────────────

pub fn read_c_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))
}

pub fn quick_validate(source: &str, file_name: &str) -> Result<ValidatedSource, ValidationError> {
    InputValidator::new(false).validate_source(source, file_name)
}

pub fn strict_validate(source: &str, file_name: &str) -> Result<ValidatedSource, ValidationError> {
    InputValidator::new(true).validate_source(source, file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_source() {
        let src = "#include <stdio.h>\nint main() { printf(\"hello\"); return 0; }";
        let v = quick_validate(src, "test.c").unwrap();
        assert!(v.is_ok());
    }

    #[test]
    fn test_empty() {
        assert!(matches!(quick_validate("   ", "e.c"), Err(ValidationError::EmptySource)));
    }

    #[test]
    fn test_unmatched_brace() {
        let v = quick_validate("int main() { int x = 1;", "b.c").unwrap();
        assert!(!v.is_ok());
    }

    #[test]
    fn test_define_expansion() {
        let src = "#define N 10\nint arr[N];";
        let v = quick_validate(src, "t.c").unwrap();
        assert!(v.source.contains("10"));
    }
}
