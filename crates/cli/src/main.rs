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
        println!("🔨 Step 6: Compiling Rust code with repair loop...");
        
        let temp_dir = std::env::temp_dir().join("diagrust_temp");
        std::fs::create_dir_all(&temp_dir)?;
        
        let compiler = CompilerLoop::new(5);
        let mut current_code = rust_code.clone();
        let mut iteration = 0;
        let max_iterations = 5;
        
        loop {
            iteration += 1;
            println!("   Iteration {}/{}...", iteration, max_iterations);
            
            let result = compiler.compile_rust_code(&current_code, &temp_dir)
                .context("Compilation check failed")?;
            
            if result.success {
                println!("   ✅ Compilation successful!");
                
                // Write the repaired code back if it was fixed
                if iteration > 1 {
                    std::fs::write(&output_path, &current_code)
                        .context("Failed to write repaired output file")?;
                    println!("   💾 Updated output with repairs");
                }
                
                let metrics = TranslationMetrics::from_result(&result, iteration);
                println!("\n{}", metrics.to_report());
                break;
            } else {
                println!("   ⚠️  Compilation failed with {} error(s)", result.errors.len());
                
                if args.verbose {
                    println!("\n   Errors:");
                    for (i, error) in result.errors.iter().take(3).enumerate() {
                        println!("     {}. {}", i + 1, error.message);
                        if let Some(ref code) = error.code {
                            println!("        Error code: {}", code.code);
                        }
                    }
                    if result.errors.len() > 3 {
                        println!("     ... and {} more", result.errors.len() - 3);
                    }
                }
                
                // Try to repair
                if iteration < max_iterations {
                    println!("   🔧 Attempting automatic repairs...");
                    
                    if let Some(repaired) = compiler.suggest_repairs(&current_code, &result.errors)? {
                        current_code = repaired;
                        println!("   ✓ Repairs applied, retrying compilation...\n");
                    } else {
                        println!("   ✗ No automatic repairs available");
                        let metrics = TranslationMetrics::from_result(&result, iteration);
                        println!("\n{}", metrics.to_report());
                        break;
                    }
                } else {
                    println!("   ✗ Max iterations reached");
                    let metrics = TranslationMetrics::from_result(&result, iteration);
                    println!("\n{}", metrics.to_report());
                    break;
                }
            }
        }
        
        if args.verbose && iteration > 1 {
            println!("\n📊 Repair Summary:");
            println!("   - Iterations needed: {}", iteration);
            println!("   - Repairs successful: {}", iteration > 1 && current_code != rust_code);
        }
        
        // Cleanup temp directory
        let _ = std::fs::remove_dir_all(&temp_dir);
    } else {
        println!("⏭️  Step 6: Compilation check skipped (--no-compile)");
    }
    
    println!("\n✨ Translation complete!");
    
    Ok(())
}