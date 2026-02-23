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
*/

// crates/compiler_loop/src/lib.rs
use anyhow::{Context, Result};
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
            if let Some(ref code_info) = error.code {
                match code_info.code.as_str() {
                    "E0425" => {
                        // Cannot find value - might need to declare variable
                        if error.message.contains("not found in this scope") {
                            // MVP: Just add a comment, don't auto-fix
                            // In full version, this would be more sophisticated
                        }
                    }
                    "E0308" => {
                        // Type mismatch - suggest adding type annotations
                    }
                    _ => {}
                }
            }
        }
        
        if repairs_applied {
            Ok(Some(repaired_code))
        } else {
            Ok(None)
        }
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