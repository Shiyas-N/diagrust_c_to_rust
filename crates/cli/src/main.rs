//! # DIAGRUST CLI
//!
//! Command-line entry point that orchestrates the full six-stage pipeline:
//!
//!   Stage 1 – Input Validation   (input_validation crate)
//!   Stage 2 – AST Processing     (ast_input crate)
//!   Stage 3 – Semantic Analysis  (semantic crate)
//!   Stage 4 – IR Construction    (ir crate)
//!   Stage 5 – Code Generation    (codegen crate)
//!   Stage 6 – Compiler Loop & Repair (compiler_loop + repair crates)
//!
//! Final output: Rust source file + Translation Report.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

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
DIAGRUST translates C source code to idiomatic, memory-safe Rust by:\n\
  1. Validating and preprocessing the C source\n\
  2. Parsing it into an internal AST\n\
  3. Running semantic analysis (def-use chains, alias analysis, ownership inference)\n\
  4. Building a typed Intermediate Representation\n\
  5. Generating idiomatic Rust code\n\
  6. Iteratively compiling and repairing via rustc diagnostics"
)]
struct Args {
    /// Input C source file (.c)
    #[arg(value_name = "INPUT.c")]
    input: PathBuf,

    /// Output Rust file (default: same name with .rs extension)
    #[arg(short, long, value_name = "OUTPUT.rs")]
    output: Option<PathBuf>,

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

    /// Save a JSON translation report alongside the output
    #[arg(long)]
    report: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Batch: translate multiple .c files (space-separated)
    #[arg(long, num_args = 1.., value_name = "FILES")]
    batch: Option<Vec<PathBuf>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    // Initialise logger (RUST_LOG=info diagrust ... for more output)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();

    print_banner();

    // Batch mode
    if let Some(ref files) = args.batch {
        let all_files = files.clone();
        let mut success = 0usize;
        let mut total = all_files.len();
        for file in &all_files {
            println!("\n─── Translating: {} ───", file.display());
            match translate_file(file, None, &args) {
                Ok(report) => {
                    if report.metrics.compilation_success { success += 1; }
                    println!("{}", report.render());
                }
                Err(e) => {
                    eprintln!("  ERROR: {}", e);
                }
            }
        }
        println!("\n📊 Batch complete: {}/{} succeeded.", success, total);
        return Ok(());
    }

    // Single file
    let report = translate_file(&args.input, args.output.as_ref().map(|p| p.as_path()), &args)?;
    println!("{}", report.render());

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Core translation pipeline
// ─────────────────────────────────────────────────────────────────────────────

fn translate_file(
    input: &std::path::Path,
    output_override: Option<&std::path::Path>,
    args: &Args,
) -> Result<TranslationReport> {

    let output_path: PathBuf = output_override
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| {
            let mut o = input.to_path_buf();
            o.set_extension("rs");
            o
        });

    println!("  📂 Input  : {}", input.display());
    println!("  📄 Output : {}", output_path.display());

    // ── Stage 1: Input Validation ─────────────────────────────────────────────
    println!("\n  ── Stage 1: Input Validation");
    let validator = InputValidator::new(!args.permissive);

    let validated = if args.no_validate {
        println!("     ⚠  Validation skipped (--no-validate)");
        let raw = std::fs::read_to_string(input)
            .context("Failed to read input file")?;
        // Minimal validated source
        input_validation::ValidatedSource {
            source: raw,
            file_path: input.to_string_lossy().to_string(),
            line_count: 0,
            features: vec![],
            warnings: vec![],
        }
    } else {
        match validator.validate_file(input) {
            Ok(v) => {
                println!("     ✓ Validation passed ({} lines, {} features detected)",
                    v.line_count, v.features.len());
                if args.verbose {
                    for w in &v.warnings { println!("       ⚠  {}", w); }
                }
                v
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Validation failed: {}", e));
            }
        }
    };

    let validation_warnings = validated.warnings.clone();
    let c_source = validated.source.clone();

    // ── Stage 2: AST Processing ───────────────────────────────────────────────
    println!("\n  ── Stage 2: AST Processing (Parsing)");
    let c_program = SimpleParser::parse(&c_source)
        .context("C parsing failed")?;

    println!("     ✓ Parsed {} function(s)", c_program.functions.len());
    if args.verbose {
        for f in &c_program.functions {
            println!("       · {}({} params)", f.name, f.parameters.len());
        }
    }

    if c_program.functions.is_empty() {
        return Err(anyhow::anyhow!("No functions found in the C source"));
    }

    // ── Stage 3: Semantic Analysis ────────────────────────────────────────────
    println!("\n  ── Stage 3: Semantic Analysis");
    let semantic_model = SemanticAnalyser::analyse(&c_program)
        .context("Semantic analysis failed")?;

    // Build summary for reporting
    let mut sem_summary = SemanticSummary {
        functions_analysed: semantic_model.functions.len(),
        ..Default::default()
    };
    for fs in &semantic_model.functions {
        sem_summary.pointer_variables += fs.symbol_table.symbols.values()
            .filter(|s| s.is_pointer).count();
        sem_summary.array_variables += fs.symbol_table.symbols.values()
            .filter(|s| s.is_array).count();
        sem_summary.def_use_pairs += fs.def_use_chains.values()
            .map(|c| c.def_count().min(1) * c.use_count()).sum::<usize>();
        sem_summary.has_loops = sem_summary.has_loops || fs.cfg.has_loops;
        sem_summary.has_branches = sem_summary.has_branches || fs.cfg.has_branches;
        sem_summary.has_pointers = sem_summary.has_pointers || sem_summary.pointer_variables > 0;
        for (name, kind) in &fs.ownership_map {
            sem_summary.inferred_ownership.push((
                name.clone(),
                format!("{:?}", kind),
            ));
        }
    }

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

    // ── Stage 4: IR Construction ──────────────────────────────────────────────
    println!("\n  ── Stage 4: IR Construction");
    let ir_program = IrBuilder::build(&c_program, &semantic_model)
        .context("IR construction failed")?;

    println!("     ✓ IR built: {} function(s)", ir_program.functions.len());
    if args.verbose && !ir_program.required_uses.is_empty() {
        println!("     · Required uses: {}", ir_program.required_uses.join(", "));
    }

    // ── Stage 5: Code Generation ──────────────────────────────────────────────
    println!("\n  ── Stage 5: Rust Code Generation");
    let initial_rust = CodeAssembler::generate(&ir_program)
        .context("Code generation failed")?;

    println!("     ✓ Generated {} lines of Rust", initial_rust.lines().count());

    // Write initial output
    std::fs::write(&output_path, &initial_rust)
        .context("Failed to write output file")?;
    println!("     ✓ Written to {}", output_path.display());

    // ── Stage 6: Compiler Loop & Repair ──────────────────────────────────────
    if args.no_compile {
        println!("\n  ── Stage 6: Compiler Loop (SKIPPED – --no-compile)");
        // Return a synthetic report
        let report = reporting::ReportBuilder::build(
            &input.to_string_lossy(),
            &output_path.to_string_lossy(),
            &c_source,
            &initial_rust,
            0, args.max_iterations, false, 0, 0,
            vec![], sem_summary, validation_warnings,
        );
        return Ok(report);
    }

    println!("\n  ── Stage 6: Compiler Loop & Repair (max {} iterations)", args.max_iterations);

    let compiler = CompilerLoop::new(args.max_iterations);
    let (outcome, report) = compiler.run(
        &initial_rust,
        &input.to_string_lossy(),
        &output_path.to_string_lossy(),
        &c_source,
        sem_summary,
        validation_warnings,
    )?;

    // Write final (possibly repaired) code
    let final_code = match &outcome {
        LoopOutcome::Success { final_code, .. } => final_code.clone(),
        LoopOutcome::MaxIterationsReached { best_code, .. } => best_code.clone(),
        LoopOutcome::ConvergenceFailure { code, .. } => code.clone(),
        LoopOutcome::NoCompiler { code } => code.clone(),
    };

    if final_code != initial_rust {
        std::fs::write(&output_path, &final_code)
            .context("Failed to write repaired output")?;
        println!("     ✓ Repaired code written to {}", output_path.display());
    }

    // Optional JSON report
    if args.report {
        let report_path = output_path.with_extension("report.txt");
        std::fs::write(&report_path, report.render())
            .context("Failed to write report file")?;
        println!("     📋 Report saved to {}", report_path.display());
    }

    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  🦀 DIAGRUST  –  Compiler-in-the-Loop Rust Transpiler        ║");
    // println!("║     Department of Computer Applications (MCA), GEC Thrissur  ║");
    // println!("║     Muhammed Shiyas N (TCR24MCA-2043) · March 2026           ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
}
