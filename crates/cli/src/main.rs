// crates/cli/Cargo.toml
/*
[package]
name = "diagrust-cli"
version.workspace = true
edition.workspace = true

[[bin]]
name = "diagrust"
path = "src/main.rs"

[dependencies]
ast_input = { path = "../ast_input" }
translator = { path = "../translator" }
compiler_loop = { path = "../compiler_loop" }
anyhow.workspace = true
clap = { version = "4.4", features = ["derive"] }
*/

// crates/cli/src/main.rs
use anyhow::{Context, Result};
use ast_input::{InputValidator, SimpleParser, read_c_file};
use clap::Parser;
use compiler_loop::{CompilerLoop, TranslationMetrics};
use std::path::PathBuf;
use translator::RustTranslator;

#[derive(Parser, Debug)]
#[command(
    name = "diagrust",
    version = "0.1.0",
    about = "DIAGRUST - Compiler-in-the-Loop C to Rust Transpiler (MVP)",
    long_about = "Translates C source code to Rust with validation and compilation feedback"
)]
struct Args {
    /// Input C source file
    #[arg(value_name = "INPUT_FILE")]
    input: PathBuf,
    
    /// Output Rust file (default: input.rs)
    #[arg(short, long, value_name = "OUTPUT_FILE")]
    output: Option<PathBuf>,
    
    /// Skip input validation (allow unsupported features)
    #[arg(long)]
    no_validate: bool,
    
    /// Use unsafe Rust pointers (default: try safe references)
    #[arg(long)]
    unsafe_mode: bool,
    
    /// Skip compilation check
    #[arg(long)]
    no_compile: bool,
    
    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    println!("🦀 DIAGRUST - C to Rust Transpiler (MVP)");
    println!("Input: {}", args.input.display());
    
    // Determine output file
    let output_path = args.output.clone().unwrap_or_else(|| {
        let mut out = args.input.clone();
        out.set_extension("rs");
        out
    });
    
    println!("Output: {}", output_path.display());
    println!();
    
    // Step 1: Read C source file
    println!("📖 Step 1: Reading C source file...");
    let c_source = read_c_file(&args.input)
        .context("Failed to read input file")?;
    
    if args.verbose {
        println!("   Read {} bytes", c_source.len());
    }
    
    // Step 2: Validate input
    if !args.no_validate {
        println!("✅ Step 2: Validating input...");
        let validator = InputValidator::new(true);
        validator.validate(&c_source)
            .context("Input validation failed")?;
        println!("   ✓ Validation passed");
    } else {
        println!("⚠️  Step 2: Validation skipped (--no-validate)");
    }
    
    // Step 3: Parse C code
    println!("🔍 Step 3: Parsing C code...");
    let c_program = SimpleParser::parse(&c_source)
        .context("Failed to parse C code")?;
    
    if args.verbose {
        println!("   Found {} function(s)", c_program.functions.len());
        for func in &c_program.functions {
            println!("     - {}()", func.name);
        }
    }
    
    // Step 4: Translate to Rust
    println!("🔄 Step 4: Translating to Rust...");
    let translator = RustTranslator::new(args.unsafe_mode);
    let rust_code = translator.translate(&c_program)
        .context("Translation failed")?;
    
    if args.verbose {
        println!("   Generated {} lines of Rust", rust_code.lines().count());
    }
    
    // Step 5: Write output
    println!("💾 Step 5: Writing output...");
    std::fs::write(&output_path, &rust_code)
        .context("Failed to write output file")?;
    println!("   ✓ Written to {}", output_path.display());
    
    // Step 6: Compile and check (optional)
    if !args.no_compile {
        println!("🔨 Step 6: Compiling Rust code...");
        
        let temp_dir = std::env::temp_dir().join("diagrust_temp");
        std::fs::create_dir_all(&temp_dir)?;
        
        let compiler = CompilerLoop::new(5);
        let result = compiler.compile_rust_code(&rust_code, &temp_dir)
            .context("Compilation check failed")?;
        
        let metrics = TranslationMetrics::from_result(&result, 1);
        
        if result.success {
            println!("   ✅ Compilation successful!");
        } else {
            println!("   ⚠️  Compilation failed with {} error(s)", result.errors.len());
            
            if args.verbose {
                println!("\n   Errors:");
                for (i, error) in result.errors.iter().take(5).enumerate() {
                    println!("     {}. {}", i + 1, error.message);
                    if let Some(ref code) = error.code {
                        println!("        Error code: {}", code.code);
                    }
                }
                if result.errors.len() > 5 {
                    println!("     ... and {} more", result.errors.len() - 5);
                }
            }
        }
        
        if args.verbose && !result.warnings.is_empty() {
            println!("\n   Warnings: {}", result.warnings.len());
        }
        
        println!("\n{}", metrics.to_report());
        
        // Cleanup temp directory
        let _ = std::fs::remove_dir_all(&temp_dir);
    } else {
        println!("⏭️  Step 6: Compilation check skipped (--no-compile)");
    }
    
    println!("\n✨ Translation complete!");
    
    Ok(())
}