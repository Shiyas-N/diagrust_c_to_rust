//! # Semantic Analysis Module (Process 0.3)
//!
//! Implements all sub-processes of DFD Level 2, Figure 5.4:
//!   0.3.1 Data Dependency Analyser  → builds Def-Use chains
//!   0.3.2 Memory Patterns & Alias Detection
//!   0.3.3 Ownership & Lifetime Inference
//!   0.3.4 Type Mapping to Rust
//!
//! The module operates on the `CProgram` AST produced by `ast_input`
//! and emits a `SemanticModel` consumed by the `ir` and `codegen` crates.

use ast_input::*;
use std::collections::{HashMap, HashSet};

pub mod def_use;
pub mod alias;
pub mod ownership;
pub mod type_map;
pub mod symbol_table;
pub mod cfg;

pub use def_use::*;
pub use alias::*;
pub use ownership::*;
pub use type_map::*;
pub use symbol_table::*;
pub use cfg::*;

// ─────────────────────────────────────────────────────────────────────────────
// Top-level result: SemanticModel
// ─────────────────────────────────────────────────────────────────────────────

/// Complete semantic findings for an entire `CProgram`.
/// This is the data type passed from Process 0.3 → Process 0.4.
#[derive(Debug, Clone)]
pub struct SemanticModel {
    /// Per-function semantic analyses
    pub functions: Vec<FunctionSemantics>,
}

/// Semantic information for a single C function.
#[derive(Debug, Clone)]
pub struct FunctionSemantics {
    pub name: String,
    pub return_type: CType,
    pub parameters: Vec<CParameter>,
    /// Symbol table (variables in scope)
    pub symbol_table: SymbolTable,
    /// Def-use chains per variable
    pub def_use_chains: HashMap<String, DefUseChain>,
    /// Pointer alias graph
    pub alias_graph: AliasGraph,
    /// Memory access patterns
    pub memory_patterns: Vec<MemoryPattern>,
    /// Ownership assignments per variable
    pub ownership_map: HashMap<String, OwnershipKind>,
    /// Lifetime annotations per variable
    pub lifetime_map: HashMap<String, LifetimeScope>,
    /// Rust type mappings (C type → Rust type string)
    pub rust_type_map: HashMap<String, RustType>,
    /// Control flow graph (basic)
    pub cfg: ControlFlowGraph,
    /// Variables that are mutated (written after initial declaration)
    pub mutable_vars: HashSet<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// SemanticAnalyser – orchestrates all sub-analysers
// ─────────────────────────────────────────────────────────────────────────────

pub struct SemanticAnalyser;

impl SemanticAnalyser {
    /// Analyse an entire C program and return the complete semantic model.
    pub fn analyse(program: &CProgram) -> anyhow::Result<SemanticModel> {
        let mut functions = Vec::new();

        for func in &program.functions {
            let fs = Self::analyse_function(func)?;
            functions.push(fs);
        }

        Ok(SemanticModel { functions })
    }

    fn analyse_function(func: &CFunction) -> anyhow::Result<FunctionSemantics> {
        // 1. Build symbol table
        let symbol_table = SymbolTableBuilder::build(func);

        // 2. Def-Use chains (0.3.1)
        let def_use_chains = DefUseAnalyser::analyse(func, &symbol_table);

        // 3. Memory patterns & alias detection (0.3.2)
        let alias_graph = AliasAnalyser::analyse(func, &symbol_table);
        let memory_patterns = MemoryPatternAnalyser::analyse(func, &symbol_table);

        // 4. Ownership & lifetime inference (0.3.3)
        let ownership_map = OwnershipInferrer::infer(func, &alias_graph, &memory_patterns);
        let lifetime_map = LifetimeInferrer::infer(func, &def_use_chains, &symbol_table);

        // 5. Rust type mapping (0.3.4)
        let rust_type_map = TypeMapper::map_function(func, &ownership_map);

        // 6. Basic CFG
        let cfg = CfgBuilder::build(func);

        // 7. Mutability analysis
        let mutable_vars = MutabilityAnalyser::collect(func);

        Ok(FunctionSemantics {
            name: func.name.clone(),
            return_type: func.return_type.clone(),
            parameters: func.parameters.clone(),
            symbol_table,
            def_use_chains,
            alias_graph,
            memory_patterns,
            ownership_map,
            lifetime_map,
            rust_type_map,
            cfg,
            mutable_vars,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mutability Analyser (helper used by both semantic and codegen layers)
// ─────────────────────────────────────────────────────────────────────────────

pub struct MutabilityAnalyser;

impl MutabilityAnalyser {
    pub fn collect(func: &CFunction) -> HashSet<String> {
        let mut set = HashSet::new();
        for stmt in &func.body {
            Self::from_statement(stmt, &mut set);
        }
        set
    }

    fn from_statement(stmt: &CStatement, set: &mut HashSet<String>) {
        match stmt {
            CStatement::Assignment { target, .. } => { set.insert(target.clone()); }
            CStatement::IndexedAssignment { array, .. } => { set.insert(array.clone()); }
            CStatement::If { then_body, else_body, .. } => {
                for s in then_body { Self::from_statement(s, set); }
                if let Some(eb) = else_body { for s in eb { Self::from_statement(s, set); } }
            }
            CStatement::While { body, .. } => { for s in body { Self::from_statement(s, set); } }
            CStatement::For { init, body, .. } => {
                Self::from_statement(init, set);
                for s in body { Self::from_statement(s, set); }
            }
            _ => {}
        }
    }
}
