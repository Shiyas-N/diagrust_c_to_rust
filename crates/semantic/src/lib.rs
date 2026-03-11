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

impl FunctionSemantics {
    /// Create an empty FunctionSemantics for contexts where no analysis is available
    /// (e.g. evaluating global variable initialisers).
    pub fn empty() -> Self {
        use ast_input::CType;
        Self {
            name: String::new(),
            return_type: CType::Void,
            parameters: vec![],
            symbol_table: crate::symbol_table::SymbolTable::default(),
            def_use_chains: Default::default(),
            alias_graph: crate::alias::AliasGraph::default(),
            memory_patterns: vec![],
            ownership_map: Default::default(),
            lifetime_map: Default::default(),
            rust_type_map: Default::default(),
            cfg: crate::cfg::ControlFlowGraph::default(),
            mutable_vars: Default::default(),
        }
    }
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
        // Pass def_use_chains so the inferrer can distinguish read-only pointer
        // params (&T) from write-through pointer params (&mut T).
        let ownership_map = OwnershipInferrer::infer(func, &alias_graph, &memory_patterns, &def_use_chains);
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
            CStatement::FieldAssignment { object, .. } => {
                // Only the root object variable needs mut (e.g. "p1" from "p1.x = …")
                let root = object.split('.').next().unwrap_or(object.as_str());
                set.insert(root.to_string());
            }
            CStatement::If { then_body, else_body, .. } => {
                for s in then_body { Self::from_statement(s, set); }
                if let Some(eb) = else_body { for s in eb { Self::from_statement(s, set); } }
            }
            CStatement::While { body, .. } => {
                for s in body { Self::from_statement(s, set); }
            }
            // ── do-while: body variables are mutated ──────────────────────
            CStatement::DoWhile { body, .. } => {
                for s in body { Self::from_statement(s, set); }
            }
            CStatement::For { init, increment, body, .. } => {
                Self::from_statement(init, set);
                // The increment expression (e.g. `i = i + 1`) mutates the loop variable.
                // Extract any assignment target from it so the binding is marked `mut`.
                Self::from_expr_assignments(increment, set);
                for s in body { Self::from_statement(s, set); }
            }
            CStatement::Switch { cases, default, .. } => {
                for (_, body) in cases { for s in body { Self::from_statement(s, set); } }
                if let Some(d) = default { for s in d { Self::from_statement(s, set); } }
            }
            // Expression statements: recurse to catch address-of and other mutations
            CStatement::Expression(e) => { Self::from_expr_assignments(e, set); }
            _ => {}
        }
    }

    /// Walk an expression and collect mutated variable names:
    ///  - LHS of assignment (`x = …`)
    ///  - Address-of (`&x`) — callee may write through the pointer
    ///  - Call arguments (recurse to catch nested &var)
    fn from_expr_assignments(expr: &CExpression, set: &mut HashSet<String>) {
        match expr {
            CExpression::BinaryOp { left, op: BinOp::Assign, .. } => {
                if let CExpression::Variable(name) = left.as_ref() {
                    set.insert(name.clone());
                }
            }
            CExpression::BinaryOp { left, right, .. } => {
                Self::from_expr_assignments(left, set);
                Self::from_expr_assignments(right, set);
            }
            // &var  — address-of means the callee may write through this pointer
            CExpression::UnaryOp { op: UnaryOp::AddrOf, operand } => {
                if let CExpression::Variable(name) = operand.as_ref() {
                    set.insert(name.clone());
                }
            }
            // Recurse into call arguments — they may contain &var
            CExpression::Call { args, .. } => {
                for a in args { Self::from_expr_assignments(a, set); }
            }
            _ => {}
        }
    }
}
