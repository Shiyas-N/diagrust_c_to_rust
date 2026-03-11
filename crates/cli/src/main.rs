//! # DIAGRUST CLI
//!
//! Orchestrates the full six-stage pipeline:
//!   1. Input Validation
//!   2. AST Processing (libclang)
//!   3. Semantic Analysis
//!   4. IR Construction
//!   5. Rust Code Generation
//!   6. Compiler Loop & Repair
//!
//! Supports both single-file and batch translation.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ast_input::SimpleParser;
use codegen::CodeAssembler;
use compiler_loop::{CompilerLoop, LoopOutcome};
use input_validation::InputValidator;
use ir::IrBuilder;
use reporting::{SemanticSummary, TranslationReport};
use semantic::SemanticAnalyser;

// ─────────────────────────────────────────────────────────────────────────────
// CLI arguments
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "diagrust",
    version = "0.1.0",
    about = "DIAGRUST – Compiler-in-the-Loop C to Rust Transpiler",
    long_about = "\
DIAGRUST translates C source code to idiomatic, memory-safe Rust.\n\
\n\
SINGLE FILE:\n\
  diagrust input.c\n\
  diagrust input.c -o output.rs\n\
\n\
BATCH – explicit files:\n\
  diagrust --batch file1.c file2.c file3.c\n\
\n\
BATCH – directory (all .c files recursively):\n\
  diagrust --batch-dir ./src/\n\
\n\
BATCH – glob pattern:\n\
  diagrust --batch-glob \"src/**/*.c\"\n\
\n\
BATCH with output directory:\n\
  diagrust --batch-dir ./src/ --output-dir ./translated/"
)]
struct Args {
    /// Input C source file (.c)  [single-file mode]
    #[arg(value_name = "INPUT.c", conflicts_with_all = ["batch", "batch_dir", "batch_glob"])]
    input: Option<PathBuf>,

    /// Output Rust file (default: same name with .rs extension)  [single-file mode]
    #[arg(short, long, value_name = "OUTPUT.rs")]
    output: Option<PathBuf>,

    // ── Batch modes ──────────────────────────────────────────────────────────

    /// Batch: translate an explicit list of .c files
    #[arg(long, num_args = 1.., value_name = "FILES")]
    batch: Option<Vec<PathBuf>>,

    /// Batch: recursively translate all .c files in a directory
    #[arg(long, value_name = "DIR")]
    batch_dir: Option<PathBuf>,

    /// Batch: translate all .c files matching a glob pattern  (quote the pattern!)
    #[arg(long, value_name = "PATTERN")]
    batch_glob: Option<String>,

    /// Output directory for batch translation (default: alongside each input file)
    #[arg(long, value_name = "DIR")]
    output_dir: Option<PathBuf>,

    // ── Shared options ───────────────────────────────────────────────────────

    /// Maximum compiler-repair iterations (default: 10)
    #[arg(short = 'n', long, default_value = "10")]
    max_iterations: usize,

    /// Skip input validation (not recommended)
    #[arg(long)]
    no_validate: bool,

    /// Skip compilation check (generate code only)
    #[arg(long)]
    no_compile: bool,

    /// Permissive mode: warn on unsupported features instead of aborting
    #[arg(long)]
    permissive: bool,

    /// Save a .report.txt alongside each output file
    #[arg(long)]
    report: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Keep going after errors in batch mode (default: true)
    #[arg(long, default_value = "true")]
    keep_going: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = Args::parse();
    print_banner();

    // ── Batch mode detection ─────────────────────────────────────────────────
    let batch_files = collect_batch_files(&args)?;

    if !batch_files.is_empty() {
        run_batch(batch_files, &args)
    } else if let Some(ref input) = args.input {
        // Single file
        let report = translate_file(input, args.output.as_deref(), &args)?;
        println!("{}", report.render());
        Ok(())
    } else {
        anyhow::bail!("No input file specified. Run `diagrust --help` for usage.")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Batch file collection
// ─────────────────────────────────────────────────────────────────────────────

/// Collect all C files to translate from whichever batch mode was requested.
/// Returns an empty Vec if no batch mode was active.
fn collect_batch_files(args: &Args) -> Result<Vec<PathBuf>> {
    if let Some(ref files) = args.batch {
        // Explicit file list — validate each exists
        for f in files {
            if !f.exists() {
                anyhow::bail!("Batch file not found: {}", f.display());
            }
        }
        return Ok(files.clone());
    }

    if let Some(ref dir) = args.batch_dir {
        if !dir.is_dir() {
            anyhow::bail!("--batch-dir: '{}' is not a directory", dir.display());
        }
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path().to_path_buf();
            if p.extension().and_then(|e| e.to_str()) == Some("c") && p.is_file() {
                files.push(p);
            }
        }
        files.sort();
        if files.is_empty() {
            anyhow::bail!("--batch-dir: no .c files found under '{}'", dir.display());
        }
        return Ok(files);
    }

    if let Some(ref pattern) = args.batch_glob {
        let mut files: Vec<PathBuf> = glob::glob(pattern)
            .with_context(|| format!("Invalid glob pattern: {}", pattern))?
            .filter_map(|r| r.ok())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("c") && p.is_file())
            .collect();
        files.sort();
        if files.is_empty() {
            anyhow::bail!("--batch-glob: no .c files matched '{}'", pattern);
        }
        return Ok(files);
    }

    Ok(vec![])
}

// ─────────────────────────────────────────────────────────────────────────────
// Batch runner
// ─────────────────────────────────────────────────────────────────────────────

/// Per-file result used for the batch summary table.
struct BatchResult {
    input:    PathBuf,
    output:   PathBuf,
    success:  bool,
    errors:   usize,
    warnings: usize,
    lines:    usize,
    elapsed:  std::time::Duration,
    mode:     String,   // "executable" or "library"
    error_msg: Option<String>,
}

fn run_batch(files: Vec<PathBuf>, args: &Args) -> Result<()> {
    let total = files.len();
    println!("  📦 Batch mode: {} file(s) to translate\n", total);

    // Create output directory if specified
    if let Some(ref out_dir) = args.output_dir {
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("Cannot create output directory: {}", out_dir.display()))?;
    }

    let mut results: Vec<BatchResult> = Vec::new();
    let batch_start = Instant::now();

    for (idx, input) in files.iter().enumerate() {
        println!("  [{}/{}] {}", idx + 1, total, input.display());

        let output_path = resolve_output_path(input, args);
        let t0 = Instant::now();

        match translate_file(input, Some(&output_path), args) {
            Ok(report) => {
                let elapsed = t0.elapsed();
                let ok = report.metrics.compilation_success;
                let sym = if ok { "✓" } else { "✗" };
                println!("         {} → {} ({:.1}s)", sym, output_path.display(), elapsed.as_secs_f32());
                results.push(BatchResult {
                    input: input.clone(),
                    output: output_path,
                    success: ok,
                    errors: report.metrics.error_count,
                    warnings: report.metrics.warning_count,
                    lines: report.metrics.lines_generated,
                    elapsed,
                    mode: if report.metrics.lines_generated > 0 { "ok".to_string() } else { "empty".to_string() },
                    error_msg: None,
                });
                if args.verbose {
                    println!("{}", report.render());
                }
            }
            Err(e) => {
                let elapsed = t0.elapsed();
                println!("         ✗ FAILED: {}", e);
                results.push(BatchResult {
                    input: input.clone(),
                    output: output_path,
                    success: false,
                    errors: 1,
                    warnings: 0,
                    lines: 0,
                    elapsed,
                    mode: "error".to_string(),
                    error_msg: Some(e.to_string()),
                });
                if !args.keep_going {
                    eprintln!("\n  Stopped due to error (use --keep-going to continue).");
                    break;
                }
            }
        }
    }

    // ── Summary table ─────────────────────────────────────────────────────────
    println!("\n{}", "═".repeat(72));
    println!("  DIAGRUST — Batch Translation Summary");
    println!("{}", "═".repeat(72));

    let name_width = results.iter()
        .map(|r| r.input.file_name().and_then(|n| n.to_str()).unwrap_or("?").len())
        .max().unwrap_or(20).max(20).min(40);

    println!("  {:<w$}  {:>7}  {:>6}  {:>5}  {:>5}  {}", 
        "File", "Status", "Lines", "Err", "Warn", "Time",
        w = name_width);
    println!("  {}", "─".repeat(name_width + 40));

    for r in &results {
        let fname = r.input.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let status = if r.success { "✓ OK" } else { "✗ FAIL" };
        println!("  {:<w$}  {:>7}  {:>6}  {:>5}  {:>5}  {:.1}s",
            &fname[..fname.len().min(name_width)],
            status, r.lines, r.errors, r.warnings,
            r.elapsed.as_secs_f32(),
            w = name_width);
    }

    let succeeded = results.iter().filter(|r| r.success).count();
    let failed    = results.iter().filter(|r| !r.success).count();
    let total_lines: usize = results.iter().map(|r| r.lines).sum();
    let total_elapsed = batch_start.elapsed();

    println!("  {}", "─".repeat(name_width + 40));
    println!("  {:<w$}  {:>7}  {:>6}", 
        format!("{} succeeded, {} failed", succeeded, failed),
        format!("{}/{}", succeeded, results.len()),
        total_lines,
        w = name_width);
    println!("{}", "═".repeat(72));
    println!("  Total time: {:.1}s", total_elapsed.as_secs_f32());

    if failed > 0 {
        println!("\n  Failed files:");
        for r in results.iter().filter(|r| !r.success) {
            let msg = r.error_msg.as_deref().unwrap_or("compilation errors");
            println!("    ✗ {}  — {}", r.input.display(), &msg[..msg.len().min(80)]);
        }
    }

    println!();

    // Exit with non-zero if any failed
    if failed > 0 && !args.keep_going {
        std::process::exit(1);
    }

    Ok(())
}

/// Determine output path for a file in batch mode.
fn resolve_output_path(input: &Path, args: &Args) -> PathBuf {
    if let Some(ref out_dir) = args.output_dir {
        // Preserve the filename, change extension, place in out_dir
        let mut name = input.file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("output.rs"));
        name.set_extension("rs");
        out_dir.join(name)
    } else {
        // Same directory as input, .rs extension
        let mut p = input.to_path_buf();
        p.set_extension("rs");
        p
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core translation pipeline  (single file)
// ─────────────────────────────────────────────────────────────────────────────

fn translate_file(
    input: &Path,
    output_override: Option<&Path>,
    args: &Args,
) -> Result<TranslationReport> {

    let output_path: PathBuf = output_override
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| { let mut o = input.to_path_buf(); o.set_extension("rs"); o });

    if !args.batch.is_some() && args.batch_dir.is_none() && args.batch_glob.is_none() {
        println!("  📂 Input  : {}", input.display());
        println!("  📄 Output : {}", output_path.display());
    }

    // ── Stage 1: Input Validation ─────────────────────────────────────────────
    let single = output_override.is_none() || args.verbose;
    if single { println!("\n  ── Stage 1: Input Validation"); }

    let validator = InputValidator::new(!args.permissive);
    let validated = if args.no_validate {
        if single { println!("     ⚠  Validation skipped (--no-validate)"); }
        let raw = std::fs::read_to_string(input).context("Failed to read input file")?;
        input_validation::ValidatedSource {
            source: raw,
            file_path: input.to_string_lossy().to_string(),
            line_count: 0,
            features: vec![],
            diagnostics: vec![],
            complexity: 0,
            defines: std::collections::HashMap::new(),
            warnings: vec![],
        }
    } else {
        match validator.validate_file(input) {
            Ok(v) => {
                if single {
                    let errors_n   = v.diagnostics.iter().filter(|d| d.severity == input_validation::Severity::Error).count();
                    let warnings_n = v.diagnostics.iter().filter(|d| d.severity == input_validation::Severity::Warning).count();
                    println!("     ✓ Validation passed ({} lines, {} features, complexity {}%)",
                        v.line_count, v.features.len(), v.complexity);
                    if errors_n > 0 || warnings_n > 0 {
                        println!("       {} error(s), {} warning(s):", errors_n, warnings_n);
                    }
                    for d in &v.diagnostics {
                        if d.severity >= input_validation::Severity::Warning {
                            let prefix = if d.severity == input_validation::Severity::Error { "✗" } else { "⚠" };
                            let loc = if d.line > 0 { format!(" L{}", d.line) } else { String::new() };
                            println!("       {} {}{}: {}", prefix, d.severity, loc, d.message);
                            if !d.hint.is_empty() { println!("          → {}", d.hint); }
                        }
                    }
                }
                v
            }
            Err(e) => return Err(anyhow::anyhow!("Validation failed: {}", e)),
        }
    };

    let validation_warnings = validated.warnings.clone();
    let c_source = validated.source.clone();

    // ── Stage 2: AST Processing ───────────────────────────────────────────────
    if single { println!("\n  ── Stage 2: AST Processing (Parsing)"); }
    let c_program = SimpleParser::parse(&c_source).context("C parsing failed")?;

    let mode = if c_program.has_main { "executable" } else { "library" };
    if single {
        println!("     ✓ Parsed {} function(s), {} struct(s), {} global(s)  [{}]",
            c_program.functions.len(), c_program.structs.len(),
            c_program.global_vars.len(), mode);
        if args.verbose {
            for f in &c_program.functions {
                println!("       · {}({} params){}", f.name, f.parameters.len(),
                    if f.name == "main" { " ← entry point" } else { "" });
            }
            for gv in &c_program.global_vars {
                println!("       · global: {} ({})", gv.name,
                    if gv.is_const { "const" } else { "var" });
            }
        }
    }

    if c_program.functions.is_empty() && c_program.global_vars.is_empty() {
        return Err(anyhow::anyhow!("No functions or global variables found in the C source"));
    }

    // ── Stage 3: Semantic Analysis ────────────────────────────────────────────
    if single { println!("\n  ── Stage 3: Semantic Analysis"); }
    let semantic_model = SemanticAnalyser::analyse(&c_program)
        .context("Semantic analysis failed")?;

    let mut sem_summary = SemanticSummary { functions_analysed: semantic_model.functions.len(), ..Default::default() };
    for fs in &semantic_model.functions {
        sem_summary.pointer_variables += fs.symbol_table.symbols.values().filter(|s| s.is_pointer).count();
        sem_summary.array_variables   += fs.symbol_table.symbols.values().filter(|s| s.is_array).count();
        sem_summary.def_use_pairs     += fs.def_use_chains.values()
            .map(|c| c.def_count().min(1) * c.use_count()).sum::<usize>();
        sem_summary.has_loops         = sem_summary.has_loops    || fs.cfg.has_loops;
        sem_summary.has_branches      = sem_summary.has_branches || fs.cfg.has_branches;
        sem_summary.has_pointers      = sem_summary.has_pointers || sem_summary.pointer_variables > 0;
        for (name, kind) in &fs.ownership_map {
            sem_summary.inferred_ownership.push((name.clone(), format!("{:?}", kind)));
        }
    }

    if single {
        println!("     ✓ {} function(s) analysed", sem_summary.functions_analysed);
        println!("     · {} pointer variable(s)", sem_summary.pointer_variables);
        println!("     · {} array variable(s)", sem_summary.array_variables);
        println!("     · {} def-use pair(s)", sem_summary.def_use_pairs);
        println!("     · Has loops: {}", sem_summary.has_loops);
        if args.verbose {
            for (var, kind) in &sem_summary.inferred_ownership {
                println!("       ownership({}) = {}", var, kind);
            }
        }
    }

    // ── Stage 4: IR Construction ──────────────────────────────────────────────
    if single { println!("\n  ── Stage 4: IR Construction"); }
    let ir_program = IrBuilder::build(&c_program, &semantic_model)
        .context("IR construction failed")?;
    if single {
        println!("     ✓ IR built: {} function(s), {} global(s)  [{}]",
            ir_program.functions.len(), ir_program.global_vars.len(),
            if ir_program.is_executable { "executable" } else { "library" });
    }

    // ── Stage 5: Code Generation ──────────────────────────────────────────────
    if single { println!("\n  ── Stage 5: Rust Code Generation"); }
    let initial_rust = CodeAssembler::generate(&ir_program)
        .context("Code generation failed")?;
    if single {
        println!("     ✓ Generated {} lines of Rust", initial_rust.lines().count());
    }

    std::fs::write(&output_path, &initial_rust).context("Failed to write output file")?;
    if single { println!("     ✓ Written to {}", output_path.display()); }

    // ── Stage 6: Compiler Loop & Repair ──────────────────────────────────────
    if args.no_compile {
        if single { println!("\n  ── Stage 6: Compiler Loop (SKIPPED – --no-compile)"); }
        let report = reporting::ReportBuilder::build(
            &input.to_string_lossy(), &output_path.to_string_lossy(),
            &c_source, &initial_rust, 0, args.max_iterations,
            false, 0, 0, vec![], sem_summary, validation_warnings,
        );
        return Ok(report);
    }

    if single {
        println!("\n  ── Stage 6: Compiler Loop & Repair (max {} iterations)", args.max_iterations);
    }

    let compiler = CompilerLoop::new(args.max_iterations);
    let (outcome, report) = compiler.run(
        &initial_rust,
        &input.to_string_lossy(),
        &output_path.to_string_lossy(),
        &c_source,
        sem_summary,
        validation_warnings,
    )?;

    let final_code = match &outcome {
        LoopOutcome::Success { final_code, .. }             => final_code.clone(),
        LoopOutcome::MaxIterationsReached { best_code, .. } => best_code.clone(),
        LoopOutcome::ConvergenceFailure { code, .. }        => code.clone(),
        LoopOutcome::NoCompiler { code }                    => code.clone(),
    };

    if final_code != initial_rust {
        std::fs::write(&output_path, &final_code)
            .context("Failed to write repaired output")?;
        if single { println!("     ✓ Repaired code written to {}", output_path.display()); }
    }

    if args.report {
        let report_path = output_path.with_extension("report.txt");
        std::fs::write(&report_path, report.render())
            .context("Failed to write report file")?;
        if single { println!("     📋 Report saved to {}", report_path.display()); }
    }

    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  🦀 DIAGRUST  –  Compiler-in-the-Loop Rust Transpiler        ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
}
