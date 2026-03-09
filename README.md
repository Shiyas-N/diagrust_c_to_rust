# DIAGRUST — C to Rust Transpiler

A compiler-in-the-loop diagnostic-driven transpiler that translates C source code into idiomatic, memory-safe Rust. It parses C via libclang, performs semantic analysis (ownership inference, alias detection, def-use chains), generates Rust, then iteratively repairs compilation errors using `rustc` diagnostics.

---

## Prerequisites

| Dependency                        | Purpose                | Install                                                           |
| --------------------------------- | ---------------------- | ----------------------------------------------------------------- |
| [Rust + Cargo](https://rustup.rs) | Build toolchain        | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| libclang                          | C parsing              | See below                                                         |
| GCC _(optional)_                  | Run C code from the UI | `sudo apt install gcc`                                            |

**Install libclang:**

```bash
# Ubuntu / Debian / WSL
sudo apt install -y libclang-dev clang

# macOS
brew install llvm
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"

# Windows — download LLVM installer from https://github.com/llvm/llvm-project/releases
# Tick "Add LLVM to PATH", then:
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
```

**Linux UI dependencies** (only needed for `diagrust-ui`):

```bash
sudo apt install -y libxcb-render0-dev libxcb-shape0-dev \
    libxcb-xfixes0-dev libxkbcommon-dev libssl-dev pkg-config
```

---

## Quick Start

```bash
# 1. Build everything
cargo build --release

# 2a. Launch the desktop GUI
cargo run --release -p diagrust-ui
#   Windows:  .\target\release\diagrust-ui.exe
#   Linux:    ./target/release/diagrust-ui

# 2b. Or use the CLI
./target/release/diagrust tests/c_samples/01_simple_add.c

# 3. Check the output
cat tests/c_samples/01_simple_add.rs
```

---

## GUI (`diagrust-ui`)

```bash
cargo run --release -p diagrust-ui
```

| Button                   | Action                                                              |
| ------------------------ | ------------------------------------------------------------------- |
| **⚙ Translate C → Rust** | Run the full pipeline: parse → analyse → IR → codegen → repair loop |
| **▶ Run C**              | Compile and run the C code with `gcc`, show output                  |
| **▶ Run Rust**           | Compile and run the generated Rust code with `rustc`, show output   |
| **💾 Save Rust**         | Save the `.rs` file to the path shown in the text box               |
| **📂 Open .c**           | Open a `.c` file from disk into the editor                          |
| **📋 Copy**              | Copy generated Rust to clipboard                                    |
| **Max iterations**       | How many compile-repair cycles to attempt (default 10)              |

**Bottom tabs:**

- **📺 Run Output** — stdout/stderr from C and Rust side by side
- **📋 Translation Report** — metrics, repair log, ownership analysis

---

## CLI (`diagrust`)

### Single file

```bash
# Basic translation
./target/release/diagrust input.c

# Specify output file
./target/release/diagrust input.c -o output.rs

# Verbose output
./target/release/diagrust input.c -v

# Save a .report.txt alongside the output
./target/release/diagrust input.c --report

# More repair iterations (default: 10)
./target/release/diagrust input.c --max-iterations 20

# Skip the compiler loop, emit raw generated code
./target/release/diagrust input.c --no-repair
```

### Batch mode

```bash
# Translate all .c files in a directory
./target/release/diagrust --batch-dir ./my_c_code/ --output-dir ./rust_output/

# Specific files
./target/release/diagrust --batch file1.c file2.c file3.c

# Glob pattern
./target/release/diagrust --batch-glob "src/**/*.c" --output-dir ./out/
```

---

## How It Works

```
C source
   │
   ▼
[1] ast_input     — libclang parses C into a typed AST
                    (filters out #include'd system headers automatically)
   │
   ▼
[2] semantic       — ownership inference, alias detection, def-use chains,
                    type mapping, mutability analysis
   │
   ▼
[3] ir             — builds an intermediate representation:
                    • array params typed as &mut [T] slices
                    • swap patterns rewritten to .swap(i, j)
                    • global accesses wrapped in unsafe {}
                    • variable declarations hoisted out of unsafe blocks
   │
   ▼
[4] codegen        — emits Rust source from the IR
   │
   ▼
[5] compiler_loop  — iterative rustc compile → diagnose → repair cycle:
                    • move/borrow errors   (E0382, E0502, E0505, E0507)
                    • mutability errors    (E0594, E0596)
                    • type mismatches      (E0308)
                    • lifetime errors      (E0106, E0515)
                    • keyword collisions, unused imports, format strings, …
   │
   ▼
[6] reporting      — translation report: metrics, repair log, idiomaticity score
```

---

## Project Structure

```
diagrust/
├── Cargo.toml                  ← workspace root
├── crates/
│   ├── ast_input/              ← C parser (libclang bindings)
│   ├── semantic/               ← ownership / type analysis
│   │   ├── src/alias.rs        ← pointer alias graph
│   │   ├── src/def_use.rs      ← def-use chains
│   │   ├── src/ownership.rs    ← ownership inference
│   │   └── src/type_map.rs     ← C → Rust type mapping
│   ├── ir/                     ← intermediate representation builder
│   ├── codegen/                ← Rust code emitter
│   ├── repair/                 ← automated error repair engine
│   ├── compiler_loop/          ← iterative compile-repair loop
│   ├── reporting/              ← translation report builder
│   ├── cli/                    ← `diagrust` command-line tool
│   └── ui/                     ← `diagrust-ui` desktop GUI (eframe/egui)
└── tests/
    ├── c_samples/              ← basic translation tests
    ├── math_samples/           ← arithmetic / float tests
    ├── control_flow/           ← if/while/for tests
    └── validation/             ← valid and invalid input tests
```

---

## Environment Variables

| Variable        | Purpose                                     | Example                |
| --------------- | ------------------------------------------- | ---------------------- |
| `LIBCLANG_PATH` | Directory containing `libclang.so` / `.dll` | `/usr/lib/llvm-18/lib` |
| `RUST_LOG`      | Log verbosity for the CLI                   | `debug`, `info`        |

If libclang is installed but not found at build time:

```bash
find /usr -name "libclang*.so*" 2>/dev/null   # locate it
export LIBCLANG_PATH=/usr/lib/llvm-18/lib      # point to it
```

---

## Common Issues

**`could not find native library 'clang'`**

```bash
sudo apt install libclang-dev
export LIBCLANG_PATH=/usr/lib/llvm-$(llvm-config --version | cut -d. -f1)/lib
```

**UI doesn't open on Linux**

```bash
sudo apt install libxcb-render0-dev libxcb-shape0-dev libxkbcommon-dev
```

**`gcc not found` when clicking ▶ Run C**
Install gcc (`sudo apt install gcc`) or use **▶ Run Rust** instead — that only needs `rustc`.

**Build takes a long time on first run**
Normal — Cargo is downloading and compiling ~200 crates including `eframe`. Subsequent builds take seconds.
