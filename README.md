# DIAGRUST - C to Rust Transpiler (MVP)

## Quick Start

1. Build: `cargo build --release`
2. Run: `./target/release/diagrust tests/c_samples/01_simple_add.c`
3. Check output: `cat tests/c_samples/01_simple_add.rs`

## Usage

```bash
# Basic translation
./target/release/diagrust input.c

# Verbose mode
./target/release/diagrust input.c -v

# Custom output
./target/release/diagrust input.c -o output.rs
```

See full documentation in docs/README.md
