//! # Compiler Loop & Repair (v2.0)
//! Iterative compile → diagnose → repair cycle.

use anyhow::{Context, Result};
use repair::{DiagnosticParser, RepairEngine, RustcDiagnostic};
use reporting::{RepairLogEntry, SemanticSummary, TranslationReport, ReportBuilder};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ─────────────────────────────────────────────────────────────────────────────
// Windows: suppress the flash of a console window for every rustc/gcc call.
// On non-Windows this is a no-op.
// ─────────────────────────────────────────────────────────────────────────────
trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

#[derive(Debug)]
pub struct CompilationResult {
    pub success: bool,
    pub errors: Vec<RustcDiagnostic>,
    pub warnings: Vec<RustcDiagnostic>,
    pub raw_stderr: String,
}

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

    pub fn run(
        &self,
        initial_code: &str,
        input_file: &str,
        output_file: &str,
        original_c: &str,
        semantic_summary: SemanticSummary,
        validation_warnings: Vec<String>,
    ) -> Result<(LoopOutcome, TranslationReport)> {
        std::fs::create_dir_all(&self.temp_dir).context("Failed to create temp dir")?;

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
        let mut best_code = initial_code.to_string();
        let mut best_error_count = usize::MAX;
        let mut no_progress_streak = 0usize;
        // Track which error codes we've already tried to repair in a row
        let mut stale_error_codes: HashSet<String> = HashSet::new();

        for iteration in 1..=self.max_iterations {
            println!("  🔄 Iteration {}/{}", iteration, self.max_iterations);

            let result = self.compile(&current_code)?;

            if result.success {
                println!("  ✅ Compilation succeeded on iteration {}!", iteration);
                // Apply idiomatic Rust transformations (x = x + 1 → x += 1, etc.)
                let idiomatic_code = Self::apply_idiom_pass(&current_code);
                // Re-verify the idiomatized code compiles cleanly
                let final_code = if idiomatic_code != current_code {
                    let check = self.compile(&idiomatic_code)?;
                    if check.success {
                        println!("  ✨ Idiom pass applied successfully.");
                        idiomatic_code
                    } else {
                        current_code.clone()
                    }
                } else {
                    current_code.clone()
                };
                let outcome = LoopOutcome::Success {
                    final_code: final_code.clone(),
                    iterations: iteration,
                    repair_log: repair_log.clone(),
                };
                let report = ReportBuilder::build(
                    input_file, output_file, original_c, &final_code,
                    iteration, self.max_iterations, true,
                    0, result.warnings.len(),
                    repair_log, semantic_summary, validation_warnings,
                );
                self.cleanup();
                return Ok((outcome, report));
            }

            let error_count = result.errors.len();
            println!("  ⚠  {} error(s) found.", error_count);

            // Track best code
            if error_count < best_error_count {
                best_error_count = error_count;
                best_code = current_code.clone();
                no_progress_streak = 0;
                stale_error_codes.clear();
            } else {
                no_progress_streak += 1;
            }

            // Check current error codes
            let current_codes: HashSet<String> = result.errors.iter()
                .filter_map(|e| e.error_code().map(String::from))
                .collect();

            // Convergence failure: no progress AND same error codes for 3 rounds
            if no_progress_streak >= 3 && current_codes == stale_error_codes {
                println!("  ✗ Convergence failure – same errors for 3 iterations.");
                let outcome = LoopOutcome::ConvergenceFailure {
                    code: best_code.clone(),
                    message: format!("Stuck on: {:?}", current_codes),
                    repair_log: repair_log.clone(),
                };
                let report = ReportBuilder::build(
                    input_file, output_file, original_c, &best_code,
                    iteration, self.max_iterations, false,
                    best_error_count, result.warnings.len(),
                    repair_log, semantic_summary, validation_warnings,
                );
                self.cleanup();
                return Ok((outcome, report));
            }
            stale_error_codes = current_codes;

            // Print top errors
            for (i, err) in result.errors.iter().take(4).enumerate() {
                let code = err.error_code().unwrap_or("?");
                let msg_short = &err.message[..err.message.len().min(90)];
                if let Some(line) = err.primary_line() {
                    println!("     {}. [{}] line {}: {}", i+1, code, line, msg_short);
                } else {
                    println!("     {}. [{}] {}", i+1, code, msg_short);
                }
            }

            println!("  🔧 Applying repair strategies...");
            let all_diags: Vec<_> = result.errors.iter()
                .chain(result.warnings.iter())
                .cloned()
                .collect();

            let (repaired, actions) = RepairEngine::repair(&current_code, &all_diags)?;

            if actions.is_empty() {
                // Try adding #[allow(unused)] and see if that helps move forward
                if error_count == best_error_count && !current_code.contains("#![allow(") {
                    let suppressed = format!("#![allow(unused_variables, unused_mut, dead_code)]\n{}", current_code);
                    let test_result = self.compile(&suppressed)?;
                    if test_result.errors.len() < error_count {
                        println!("  🔧 Added #![allow(...)] to suppress non-critical warnings.");
                        current_code = suppressed;
                        repair_log.push(RepairLogEntry {
                            iteration,
                            rule_name: "AllowUnused".to_string(),
                            description: "Added #![allow(...)] to suppress distracting warnings".to_string(),
                        });
                        continue;
                    }
                }
                println!("  ✗ No applicable repairs found – stopping.");
                let outcome = LoopOutcome::ConvergenceFailure {
                    code: best_code.clone(),
                    message: "No repair rules matched remaining diagnostics".into(),
                    repair_log: repair_log.clone(),
                };
                let report = ReportBuilder::build(
                    input_file, output_file, original_c, &best_code,
                    iteration, self.max_iterations, false,
                    best_error_count, result.warnings.len(),
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

        println!("  ✗ Maximum iterations ({}) reached.", self.max_iterations);
        let final_result = self.compile(&best_code)?;
        let outcome = LoopOutcome::MaxIterationsReached {
            best_code: best_code.clone(),
            last_error_count: final_result.errors.len(),
            iterations: self.max_iterations,
            repair_log: repair_log.clone(),
        };
        let report = ReportBuilder::build(
            input_file, output_file, original_c, &best_code,
            self.max_iterations, self.max_iterations,
            final_result.success,
            final_result.errors.len(), final_result.warnings.len(),
            repair_log, semantic_summary, validation_warnings,
        );
        self.cleanup();
        Ok((outcome, report))
    }

    fn compile(&self, code: &str) -> Result<CompilationResult> {
        let rust_file = self.temp_dir.join("diagrust_temp.rs");
        std::fs::write(&rust_file, code).context("Failed to write temp Rust file")?;

        // Auto-detect whether the code is a binary (has fn main) or a library.
        // Library code is compiled with --crate-type=lib so rustc doesn't
        // complain about a missing `main` function.
        let (crate_type, extra_args): (&str, &[&str]) = if code.contains("fn main()") {
            ("bin", &[])
        } else {
            ("lib", &[])
        };

        let mut cmd = Command::new("rustc");
        cmd.no_window()
           .arg(format!("--crate-type={}", crate_type))
           .arg("--error-format=json")
           .arg("--edition=2021")
           .arg(&rust_file)
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        if crate_type == "bin" {
            cmd.arg("-o").arg(self.temp_dir.join("diagrust_temp"));
        }
        for arg in extra_args { cmd.arg(arg); }

        let output = cmd.output().context("Failed to invoke rustc")?;
        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let (errors, warnings) = DiagnosticParser::parse(&raw_stderr);
        Ok(CompilationResult { success: output.status.success(), errors, warnings, raw_stderr })
    }

    /// Run the compiled binary and return its stdout output.
    pub fn run_binary(&self, stdin_input: Option<&str>) -> Result<String> {
        let binary = self.temp_dir.join("diagrust_temp");
        if !binary.exists() {
            return Err(anyhow::anyhow!("Binary not found — compile first"));
        }
        let mut cmd = Command::new(&binary);
        cmd.no_window().stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin_input.is_some() { cmd.stdin(Stdio::piped()); }

        let mut child = cmd.spawn().context("Failed to run binary")?;
        if let Some(input) = stdin_input {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(input.as_bytes())?;
            }
        }
        let out = child.wait_with_output().context("Failed to collect binary output")?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !stderr.is_empty() {
            Ok(format!("{}{}", stdout, stderr))
        } else {
            Ok(stdout)
        }
    }

    pub fn rustc_available(&self) -> bool {
        Command::new("rustc").no_window().arg("--version")
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false)
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Idiom pass: convert verbose C-style patterns to idiomatic Rust
    // ─────────────────────────────────────────────────────────────────────────

    pub fn apply_idiom_pass(code: &str) -> String {
        idiom::apply_all(code)
    }

    pub fn compile_rust_code(&self, code: &str, temp_dir: &Path) -> Result<CompilationResult> {
        let rust_file = temp_dir.join("diagrust_temp.rs");
        std::fs::write(&rust_file, code)?;
        // Auto-detect bin vs lib
        let crate_type = if code.contains("fn main()") { "bin" } else { "lib" };
        let mut cmd = Command::new("rustc");
        cmd.no_window()
           .arg(format!("--crate-type={}", crate_type))
           .arg("--error-format=json")
           .arg("--edition=2021")
           .arg(&rust_file)
           .stdout(Stdio::piped()).stderr(Stdio::piped());
        if crate_type == "bin" {
            cmd.arg("-o").arg(temp_dir.join("diagrust_temp"));
        }
        let output = cmd.output().context("Failed to invoke rustc")?;
        let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let (errors, warnings) = DiagnosticParser::parse(&raw_stderr);
        Ok(CompilationResult { success: output.status.success(), errors, warnings, raw_stderr })
    }
}
