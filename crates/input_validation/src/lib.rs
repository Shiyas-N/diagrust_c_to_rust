//! # Input Validation Module (0.1)
//!
//! This module is the entry point of the DiagRust pipeline.
//! It accepts a C source file, validates its format and basic syntactic
//! correctness, and passes the validated code to the AST Processing stage.
//!
//! Responsibilities (per SRS §4.6.1):
//! - Accept `.c` files via CLI
//! - Handle preprocessor directives
//! - Validate syntactic correctness
//! - Report parse errors with line numbers
//! - Support batch processing

use anyhow::Result;
use regex::Regex;
use std::path::Path;
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
// Error types
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
// ValidatedSource – result of successful validation
// ─────────────────────────────────────────────────────────────────────────────

/// Carries the validated, preprocessed C source and metadata.
#[derive(Debug, Clone)]
pub struct ValidatedSource {
    /// Source code after lightweight preprocessing (macro expansion, comment stripping).
    pub source: String,
    /// Absolute or relative path of the input file.
    pub file_path: String,
    /// Line count of the original source.
    pub line_count: usize,
    /// Detected features (used for reporting).
    pub features: Vec<String>,
    /// Any non-fatal validation warnings.
    pub warnings: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// InputValidator
// ─────────────────────────────────────────────────────────────────────────────

/// Validates and lightly preprocesses C source code.
pub struct InputValidator {
    /// When `true`, abort on unsupported features; otherwise warn.
    pub strict_mode: bool,
}

impl InputValidator {
    pub fn new(strict_mode: bool) -> Self {
        Self { strict_mode }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Read a `.c` file from disk and validate it.
    pub fn validate_file(&self, path: &Path) -> Result<ValidatedSource, ValidationError> {
        if !path.exists() {
            return Err(ValidationError::FileError(format!(
                "File not found: {}",
                path.display()
            )));
        }
        if path.extension().and_then(|e| e.to_str()) != Some("c") {
            return Err(ValidationError::FileError(format!(
                "Expected a .c file, got: {}",
                path.display()
            )));
        }

        let raw = std::fs::read_to_string(path).map_err(|e| {
            ValidationError::FileError(format!("Cannot read {}: {}", path.display(), e))
        })?;

        let path_str = path.to_string_lossy().to_string();
        self.validate_source(&raw, &path_str)
    }

    /// Validate an in-memory C source string.
    pub fn validate_source(
        &self,
        raw_source: &str,
        file_name: &str,
    ) -> Result<ValidatedSource, ValidationError> {
        if raw_source.trim().is_empty() {
            return Err(ValidationError::EmptySource);
        }

        let line_count = raw_source.lines().count();
        let mut warnings = Vec::new();
        let mut features = Vec::new();

        // 1. Strip comments
        let source = Self::strip_comments(raw_source);

        // 2. Handle preprocessor directives
        let (source, pp_warnings) = self.handle_preprocessor(&source)?;
        warnings.extend(pp_warnings);

        // 3. Detect language features
        self.detect_features(&source, &mut features);

        // 4. Check unsupported features
        self.check_unsupported_features(&source, self.strict_mode, &mut warnings)?;

        // 5. Validate brace / paren balance
        self.check_balance(&source)?;

        // 6. Validate function definitions exist
        self.check_function_exists(&source)?;

        // 7. Strict-mode checks
        if self.strict_mode {
            self.check_type_mismatches(&source)?;
            self.check_missing_semicolons(&source, &mut warnings)?;
        }

        Ok(ValidatedSource {
            source,
            file_path: file_name.to_string(),
            line_count,
            features,
            warnings,
        })
    }

    // ── Preprocessing ─────────────────────────────────────────────────────────

    /// Strip C-style (`/* */`) and C++-style (`//`) comments.
    fn strip_comments(source: &str) -> String {
        let mut result = String::with_capacity(source.len());
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' {
                match chars.peek() {
                    Some('/') => {
                        // Line comment – consume to end of line
                        chars.next();
                        while let Some(&ch) = chars.peek() {
                            if ch == '\n' { break; }
                            chars.next();
                        }
                    }
                    Some('*') => {
                        // Block comment
                        chars.next(); // consume '*'
                        loop {
                            match chars.next() {
                                None => break,
                                Some('*') => {
                                    if chars.peek() == Some(&'/') {
                                        chars.next();
                                        break;
                                    }
                                }
                                Some('\n') => result.push('\n'), // preserve line numbers
                                _ => {}
                            }
                        }
                    }
                    _ => result.push(c),
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    /// Handle `#include`, `#define`, and conditional compilation guards.
    /// Returns the preprocessed source and any warnings.
    fn handle_preprocessor(
        &self,
        source: &str,
    ) -> Result<(String, Vec<String>), ValidationError> {
        let mut output = String::new();
        let mut warnings = Vec::new();
        let mut defines: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        for (line_no, line) in source.lines().enumerate() {
            let trimmed = line.trim();

            if trimmed.starts_with("#include") {
                // Strip includes – we operate on the raw AST, so headers are irrelevant
                warnings.push(format!(
                    "Line {}: #include directive stripped (not needed for transpilation)",
                    line_no + 1
                ));
                output.push('\n'); // preserve line numbers
                continue;
            }

            if trimmed.starts_with("#define") {
                // Capture simple object-like macros: #define NAME VALUE
                let re = Regex::new(r"#define\s+(\w+)\s+(.+)").unwrap();
                if let Some(cap) = re.captures(trimmed) {
                    defines.insert(cap[1].to_string(), cap[2].trim().to_string());
                    warnings.push(format!(
                        "Line {}: #define '{}' recorded for expansion",
                        line_no + 1,
                        &cap[1]
                    ));
                }
                output.push('\n');
                continue;
            }

            if trimmed.starts_with("#ifdef")
                || trimmed.starts_with("#ifndef")
                || trimmed.starts_with("#endif")
                || trimmed.starts_with("#if ")
                || trimmed.starts_with("#else")
                || trimmed.starts_with("#elif")
            {
                warnings.push(format!(
                    "Line {}: Conditional compilation directive stripped",
                    line_no + 1
                ));
                output.push('\n');
                continue;
            }

            // Expand simple defines in the line
            let mut expanded = line.to_string();
            for (name, value) in &defines {
                // Only replace whole-word occurrences
                let pattern = format!(r"\b{}\b", regex::escape(name));
                if let Ok(re) = Regex::new(&pattern) {
                    expanded = re.replace_all(&expanded, value.as_str()).to_string();
                }
            }

            output.push_str(&expanded);
            output.push('\n');
        }

        Ok((output, warnings))
    }

    // ── Feature detection ─────────────────────────────────────────────────────

    fn detect_features(&self, source: &str, features: &mut Vec<String>) {
        let checks: &[(&str, &str)] = &[
            ("*", "pointers"),
            ("[", "arrays"),
            ("struct", "structs"),
            ("malloc", "dynamic allocation"),
            ("free", "deallocation"),
            ("for", "for loops"),
            ("while", "while loops"),
            ("if", "conditional branching"),
        ];
        for (token, label) in checks {
            if source.contains(token) {
                features.push(label.to_string());
            }
        }
    }

    // ── Validation helpers ────────────────────────────────────────────────────

    fn check_unsupported_features(
        &self,
        source: &str,
        strict: bool,
        warnings: &mut Vec<String>,
    ) -> Result<(), ValidationError> {
        let unsupported: &[(&str, &str)] = &[
            ("goto", "goto statements"),
            ("asm", "inline assembly"),
            ("__asm__", "inline assembly"),
            ("union", "union types"),
            ("typedef", "typedef declarations"),
            ("extern", "extern declarations"),
            ("__attribute__", "GCC attributes"),
            ("longjmp", "longjmp/setjmp"),
            ("setjmp", "longjmp/setjmp"),
            ("va_list", "variadic functions"),
        ];

        for line in source.lines().enumerate().map(|(n, l)| (n + 1, l)) {
            let (line_no, text) = line;
            let trimmed = text.trim();
            if trimmed.starts_with("//") || trimmed.is_empty() { continue; }

            for &(keyword, feature) in unsupported {
                if trimmed.contains(keyword) {
                    let msg = format!("'{}' detected on line {}", feature, line_no);
                    if strict {
                        return Err(ValidationError::UnsupportedFeature {
                            line: line_no,
                            feature: feature.to_string(),
                        });
                    } else {
                        warnings.push(format!("Warning: {}", msg));
                    }
                }
            }
        }
        Ok(())
    }

    fn check_balance(&self, source: &str) -> Result<(), ValidationError> {
        let ob = source.matches('{').count();
        let cb = source.matches('}').count();
        if ob != cb {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: format!("Mismatched braces: {} '{{' vs {} '}}'", ob, cb),
            });
        }
        let op = source.matches('(').count();
        let cp = source.matches(')').count();
        if op != cp {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: format!("Mismatched parentheses: {} '(' vs {} ')'", op, cp),
            });
        }
        let ob2 = source.matches('[').count();
        let cb2 = source.matches(']').count();
        if ob2 != cb2 {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: format!("Mismatched brackets: {} '[' vs {} ']'", ob2, cb2),
            });
        }
        Ok(())
    }

    fn check_function_exists(&self, source: &str) -> Result<(), ValidationError> {
        let func_re =
            Regex::new(r"(?m)^\s*(int|void|char|float|double)\s+\w+\s*\([^)]*\)\s*\{").unwrap();
        if !func_re.is_match(source) {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: "No function definition found in source".to_string(),
            });
        }
        Ok(())
    }

    fn check_type_mismatches(&self, source: &str) -> Result<(), ValidationError> {
        for (line_no, line) in source.lines().enumerate() {
            let t = line.trim();
            // int x = "string"
            if t.starts_with("int") && t.contains('=') && t.contains('"') {
                return Err(ValidationError::SyntaxError {
                    line: line_no + 1,
                    message: format!("Type mismatch: integer assigned string literal: '{}'", t),
                });
            }
            // char x = "string" (not char*)
            if t.starts_with("char ") && !t.contains("char*") && t.contains('=') && t.contains('"') {
                return Err(ValidationError::SyntaxError {
                    line: line_no + 1,
                    message: format!("Type mismatch: char assigned string literal (use char*): '{}'", t),
                });
            }
        }
        Ok(())
    }

    fn check_missing_semicolons(
        &self,
        source: &str,
        warnings: &mut Vec<String>,
    ) -> Result<(), ValidationError> {
        let lines: Vec<&str> = source.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") { continue; }
            // Lines ending in word/number that look like incomplete statements
            if Self::looks_like_statement(t)
                && !t.ends_with(';')
                && !t.ends_with('{')
                && !t.ends_with('}')
                && !t.ends_with(',')
            {
                warnings.push(format!(
                    "Line {}: Possible missing semicolon: '{}'",
                    i + 1,
                    t
                ));
            }
        }
        Ok(())
    }

    fn looks_like_statement(line: &str) -> bool {
        let keywords = ["return", "break", "continue"];
        keywords.iter().any(|k| line.starts_with(k))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Convenience function
// ─────────────────────────────────────────────────────────────────────────────

/// Read a C source file and return its contents.
pub fn read_c_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))
}
