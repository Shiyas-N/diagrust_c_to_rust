// crates/compiler_loop/Cargo.toml
/*
[package]
name = "compiler_loop"
version.workspace = true
edition.workspace = true

[dependencies]
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
regex = "1.10"
*/

// crates/compiler_loop/src/lib.rs
use anyhow::{Context, Result};
use regex;
use serde::Deserialize;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct CompilationResult {
    pub success: bool,
    pub errors: Vec<RustcDiagnostic>,
    pub warnings: Vec<RustcDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RustcDiagnostic {
    pub message: String,
    #[serde(default)]
    pub code: Option<DiagnosticCode>,
    pub level: String,
    #[serde(default)]
    pub spans: Vec<DiagnosticSpan>,
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
}

/// Compiler loop - compiles Rust code and analyzes diagnostics
pub struct CompilerLoop {
    max_iterations: usize,
}

impl CompilerLoop {
    pub fn new(max_iterations: usize) -> Self {
        Self { max_iterations }
    }
    
    /// Compile Rust code and return diagnostics
    pub fn compile_rust_code(&self, code: &str, temp_dir: &Path) -> Result<CompilationResult> {
        // Write code to temporary file
        let rust_file = temp_dir.join("temp_code.rs");
        std::fs::write(&rust_file, code)
            .context("Failed to write temporary Rust file")?;
        
        // Run rustc with JSON output
        let output = Command::new("rustc")
            .arg("--crate-type")
            .arg("lib")
            .arg("--error-format=json")
            .arg(&rust_file)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to run rustc")?;
        
        let success = output.status.success();
        
        // Parse diagnostics from stderr
        let stderr = String::from_utf8_lossy(&output.stderr);
        let (errors, warnings) = self.parse_diagnostics(&stderr)?;
        
        Ok(CompilationResult {
            success,
            errors,
            warnings,
        })
    }
    
    fn parse_diagnostics(&self, output: &str) -> Result<(Vec<RustcDiagnostic>, Vec<RustcDiagnostic>)> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        
        for line in output.lines() {
            if line.trim().is_empty() {
                continue;
            }
            
            // Try to parse as JSON diagnostic
            if let Ok(diag) = serde_json::from_str::<RustcDiagnostic>(line) {
                match diag.level.as_str() {
                    "error" => errors.push(diag),
                    "warning" => warnings.push(diag),
                    _ => {}
                }
            }
        }
        
        Ok((errors, warnings))
    }
    
    /// Basic repair suggestions (MVP - simple text replacements)
    pub fn suggest_repairs(&self, code: &str, errors: &[RustcDiagnostic]) -> Result<Option<String>> {
        let mut repaired_code = code.to_string();
        let mut repairs_applied = false;
        
        for error in errors {
            // Get error code if available
            let error_code = error.code.as_ref().map(|c| c.code.as_str());
            
            // Repair Rule 1: Rust keyword collision
            if error.message.contains("expected identifier, found keyword") {
                // Extract the keyword from error message
                if let Some(keyword) = Self::extract_keyword_from_message(&error.message) {
                    println!("   🔧 Repair: Renaming Rust keyword '{}' to '{}_val'", keyword, keyword);
                    
                    // Replace variable declarations with keyword
                    let pattern = format!(r"let {}: ", keyword);
                    let replacement = format!("let {}_val: ", keyword);
                    repaired_code = repaired_code.replace(&pattern, &replacement);
                    
                    // Replace variable uses
                    let pattern = format!(r"return {};", keyword);
                    let replacement = format!("return {}_val;", keyword);
                    repaired_code = repaired_code.replace(&pattern, &replacement);
                    
                    repairs_applied = true;
                }
            }
            
            // Repair Rule 2: Type mismatch - bool assigned to i32
            if let Some("E0308") = error_code {
                if error.message.contains("expected `i32`, found `bool`") 
                    || error.message.contains("mismatched types") {
                    println!("   🔧 Repair: Converting bool comparison to i32");
                    
                    // Find pattern: let var: i32 = comparison;
                    let bool_pattern = regex::Regex::new(
                        r"let (\w+): i32 = ([^;]+[<>]=?[^;]+);"
                    ).unwrap();
                    
                    if let Some(cap) = bool_pattern.captures(&repaired_code) {
                        let var_name = &cap[1];
                        let comparison = &cap[2];
                        let replacement = format!(
                            "let {}: i32 = if {} {{ 1 }} else {{ 0 }};",
                            var_name, comparison
                        );
                        repaired_code = bool_pattern.replace(&repaired_code, replacement).to_string();
                        repairs_applied = true;
                    }
                }
            }
            
            // Repair Rule 3: Unused variable warning
            if error.message.contains("unused variable") || error.message.contains("never read") {
                // Extract variable name
                if let Some(var_name) = Self::extract_variable_from_message(&error.message) {
                    println!("   🔧 Repair: Prefixing unused variable '{}' with '_'", var_name);
                    
                    let pattern = format!("let {}: ", var_name);
                    let replacement = format!("let _{}: ", var_name);
                    repaired_code = repaired_code.replace(&pattern, &replacement);
                    repairs_applied = true;
                }
            }
            
            // Repair Rule 5: Array indexed by i32 instead of usize (E0277)
            if let Some("E0277") = error_code {
                if error.message.contains("cannot be indexed by `i32`")
                    || error.message.contains("cannot be indexed by") {
                    println!("   🔧 Repair: Adding `as usize` casts on array indices");

                    // Replace patterns like arr[expr] where expr is not already usize
                    // We replace `arr[<anything not containing as usize>]`
                    let idx_pattern = regex::Regex::new(
                        r"\[([^\[\]]+)\]"
                    ).unwrap();

                    let new_code = idx_pattern.replace_all(&repaired_code, |caps: &regex::Captures| {
                        let inner = &caps[1];
                        // Skip if already has usize cast or is a plain literal
                        if inner.contains("as usize") || inner.parse::<usize>().is_ok() {
                            format!("[{}]", inner)
                        } else {
                            format!("[({}) as usize]", inner)
                        }
                    }).to_string();

                    if new_code != repaired_code {
                        repaired_code = new_code;
                        repairs_applied = true;
                    }
                }
            }

            // Repair Rule 4: Missing return statement
            if error.message.contains("mismatched types") && error.message.contains("expected `()`") {
                if !repaired_code.contains("return;") {
                    println!("   🔧 Repair: Adding missing return statement");
                    // Add return; before closing brace
                    if let Some(pos) = repaired_code.rfind('}') {
                        repaired_code.insert_str(pos, "    return;\n");
                        repairs_applied = true;
                    }
                }
            }
        }
        
        if repairs_applied {
            Ok(Some(repaired_code))
        } else {
            Ok(None)
        }
    }
    
    fn extract_keyword_from_message(message: &str) -> Option<String> {
        // Pattern: "expected identifier, found keyword `mod`"
        let pattern = regex::Regex::new(r"found keyword `(\w+)`").unwrap();
        pattern.captures(message)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
    }
    
    fn extract_variable_from_message(message: &str) -> Option<String> {
        // Pattern: "unused variable: `result`"
        let pattern = regex::Regex::new(r"variable:?\s+`(\w+)`").unwrap();
        pattern.captures(message)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
    }
}

/// Simple metrics collector
#[derive(Debug, Default)]
pub struct TranslationMetrics {
    pub compilation_success: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub iterations_needed: usize,
}

impl TranslationMetrics {
    pub fn from_result(result: &CompilationResult, iterations: usize) -> Self {
        Self {
            compilation_success: result.success,
            error_count: result.errors.len(),
            warning_count: result.warnings.len(),
            iterations_needed: iterations,
        }
    }
    
    pub fn to_report(&self) -> String {
        format!(
            "Translation Metrics:\n\
             - Compilation: {}\n\
             - Errors: {}\n\
             - Warnings: {}\n\
             - Iterations: {}",
            if self.compilation_success { "SUCCESS" } else { "FAILED" },
            self.error_count,
            self.warning_count,
            self.iterations_needed
        )
    }
}