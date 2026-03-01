//! # Compiler Loop & Repair (Process 0.5)
//!
//! Implements the iterative compile → diagnose → repair cycle described in
//! SRS §4.6.6–4.6.8 and DFD Figure 5.6.
//!
//! Sub-processes:
//!   0.5.1 Compile Code (rustc invocation)
//!   0.5.2 Repair Strategy Application → delegates to `repair` crate
//!   0.5.3 Iteration Control & Reporting → returns result to caller

use anyhow::{Context, Result};
use repair::{DiagnosticParser, RepairAction, RepairEngine, RustcDiagnostic};
use reporting::{RepairLogEntry, SemanticSummary, TranslationReport, ReportBuilder};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ─────────────────────────────────────────────────────────────────────────────
// Compilation result
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CompilationResult {
    pub success: bool,
    pub errors: Vec<RustcDiagnostic>,
    pub warnings: Vec<RustcDiagnostic>,
    pub raw_stderr: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Loop outcome
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LoopOutcome {
    Success {
        final_code: String,
        iterations: usize,
        repair_log: Vec<RepairLogEntry>,
    },
    MaxIterationsReached {
        best_code: String,
        last_error_count: usize,
        iterations: usize,
        repair_log: Vec<RepairLogEntry>,
    },
    ConvergenceFailure {
        code: String,
        message: String,
        repair_log: Vec<RepairLogEntry>,
    },
    NoCompiler {
        code: String,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Compiler loop
// ─────────────────────────────────────────────────────────────────────────────

pub struct CompilerLoop {
    pub max_iterations: usize,
    pub temp_dir: PathBuf,
}

impl CompilerLoop {
    pub fn new(max_iterations: usize) -> Self {
        let temp_dir = std::env::temp_dir().join(format!("diagrust_{}", std::process::id()));
        Self { max_iterations, temp_dir }
    }

    pub fn with_temp_dir(max_iterations: usize, temp_dir: PathBuf) -> Self {
        Self { max_iterations, temp_dir }
    }

    /// Run the full compile-repair loop (Process 0.5).
    ///
    /// Returns the loop outcome along with the full translation report.
    pub fn run(
        &self,
        initial_code: &str,
        input_file: &str,
        output_file: &str,
        original_c: &str,
        semantic_summary: SemanticSummary,
        validation_warnings: Vec<String>,
    ) -> Result<(LoopOutcome, TranslationReport)> {
        std::fs::create_dir_all(&self.temp_dir)
            .context("Failed to create temp directory")?;

        // Check if rustc is available
        if !self.rustc_available() {
            println!("  ⚠  rustc not found – skipping compilation check.");
            let outcome = LoopOutcome::NoCompiler { code: initial_code.to_string() };
            let report = ReportBuilder::build(
                input_file, output_file, original_c, initial_code,
                0, self.max_iterations, false, 0, 0,
                vec![], semantic_summary, validation_warnings,
            );
            return Ok((outcome, report));
        }

        let mut current_code = initial_code.to_string();
        let mut repair_log: Vec<RepairLogEntry> = Vec::new();
        let mut last_error_count = usize::MAX;
        let mut no_progress_count = 0usize;

        for iteration in 1..=self.max_iterations {
            println!("  🔄 Iteration {}/{}", iteration, self.max_iterations);

            // 0.5.1: Compile
            let result = self.compile(&current_code)?;

            if result.success {
                println!("  ✅ Compilation succeeded on iteration {}!", iteration);
                let outcome = LoopOutcome::Success {
                    final_code: current_code.clone(),
                    iterations: iteration,
                    repair_log: repair_log.clone(),
                };
                let report = ReportBuilder::build(
                    input_file, output_file, original_c, &current_code,
                    iteration, self.max_iterations, true,
                    result.errors.len(), result.warnings.len(),
                    repair_log, semantic_summary, validation_warnings,
                );
                self.cleanup();
                return Ok((outcome, report));
            }

            let error_count = result.errors.len();
            println!("  ⚠  {} error(s) found.", error_count);

            // Check for convergence failure (no improvement)
            if error_count >= last_error_count {
                no_progress_count += 1;
            } else {
                no_progress_count = 0;
            }
            last_error_count = error_count;

            if no_progress_count >= 3 {
                println!("  ✗ Convergence failure – repairs are not reducing error count.");
                let outcome = LoopOutcome::ConvergenceFailure {
                    code: current_code.clone(),
                    message: "Error count stopped decreasing after 3 consecutive iterations".into(),
                    repair_log: repair_log.clone(),
                };
                let report = ReportBuilder::build(
                    input_file, output_file, original_c, &current_code,
                    iteration, self.max_iterations, false,
                    error_count, result.warnings.len(),
                    repair_log, semantic_summary, validation_warnings,
                );
                self.cleanup();
                return Ok((outcome, report));
            }

            // Print top errors (verbose)
            for (i, err) in result.errors.iter().take(3).enumerate() {
                let code = err.error_code().unwrap_or("?");
                println!("     {}. [{}] {}", i + 1, code, &err.message[..err.message.len().min(80)]);
            }

            // 0.5.2: Repair
            println!("  🔧 Applying repair strategies...");
            let all_diags: Vec<_> = result.errors.iter()
                .chain(result.warnings.iter())
                .cloned()
                .collect();

            let (repaired, actions) = RepairEngine::repair(&current_code, &all_diags)?;

            if actions.is_empty() {
                println!("  ✗ No applicable repairs found – stopping.");
                let outcome = LoopOutcome::ConvergenceFailure {
                    code: current_code.clone(),
                    message: "No repair rules matched the remaining diagnostics".into(),
                    repair_log: repair_log.clone(),
                };
                let report = ReportBuilder::build(
                    input_file, output_file, original_c, &current_code,
                    iteration, self.max_iterations, false,
                    error_count, result.warnings.len(),
                    repair_log, semantic_summary, validation_warnings,
                );
                self.cleanup();
                return Ok((outcome, report));
            }

            for action in &actions {
                println!("     ✓ {}: {}", action.rule_name, action.description);
                repair_log.push(RepairLogEntry {
                    iteration,
                    rule_name: action.rule_name.clone(),
                    description: action.description.clone(),
                });
            }

            current_code = repaired;
        }

        // Max iterations reached
        println!("  ✗ Maximum iterations ({}) reached.", self.max_iterations);
        let final_result = self.compile(&current_code)?;
        let outcome = LoopOutcome::MaxIterationsReached {
            best_code: current_code.clone(),
            last_error_count: final_result.errors.len(),
            iterations: self.max_iterations,
            repair_log: repair_log.clone(),
        };
        let report = ReportBuilder::build(
            input_file, output_file, original_c, &current_code,
            self.max_iterations, self.max_iterations,
            final_result.success,
            final_result.errors.len(), final_result.warnings.len(),
            repair_log, semantic_summary, validation_warnings,
        );
        self.cleanup();
        Ok((outcome, report))
    }

    // ── 0.5.1: Compile ────────────────────────────────────────────────────────

    fn compile(&self, code: &str) -> Result<CompilationResult> {
        let rust_file = self.temp_dir.join("diagrust_temp.rs");
        std::fs::write(&rust_file, code)
            .context("Failed to write temporary Rust file")?;

        let output = Command::new("rustc")
            .arg("--crate-type=lib")
            .arg("--error-format=json")
            .arg("--edition=2021")
            .arg(&rust_file)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to invoke rustc")?;

        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let (errors, warnings) = DiagnosticParser::parse(&raw_stderr);

        Ok(CompilationResult {
            success: output.status.success(),
            errors,
            warnings,
            raw_stderr,
        })
    }

    fn rustc_available(&self) -> bool {
        Command::new("rustc")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }

    /// Compatibility method – compile a code string directly (used by CLI for single-shot check).
    pub fn compile_rust_code(&self, code: &str, temp_dir: &Path) -> Result<CompilationResult> {
        let rust_file = temp_dir.join("diagrust_temp.rs");
        std::fs::write(&rust_file, code)?;

        let output = Command::new("rustc")
            .arg("--crate-type=lib")
            .arg("--error-format=json")
            .arg("--edition=2021")
            .arg(&rust_file)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to invoke rustc")?;

        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let (errors, warnings) = DiagnosticParser::parse(&raw_stderr);

        Ok(CompilationResult {
            success: output.status.success(),
            errors,
            warnings,
            raw_stderr,
        })
    }
}
