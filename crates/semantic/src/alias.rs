//! # Memory Patterns & Alias Detection (Process 0.3.2)
//!
//! Identifies heap/stack allocation patterns, detects pointer aliasing
//! relationships, and classifies memory access patterns.

use ast_input::*;
use std::collections::{HashMap, HashSet};
use crate::symbol_table::SymbolTable;

// ─────────────────────────────────────────────────────────────────────────────
// Memory patterns
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum StorageClass {
    Stack,
    Heap,
    Static,
    Parameter,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct MemoryPattern {
    pub variable: String,
    pub storage: StorageClass,
    pub is_pointer: bool,
    pub is_array: bool,
    pub pointer_depth: usize,    // number of stars: int** → 2
    pub is_const: bool,
    pub is_mutable: bool,
    pub size_known: bool,
    pub array_size: Option<usize>,
}

pub struct MemoryPatternAnalyser;

impl MemoryPatternAnalyser {
    pub fn analyse(func: &CFunction, _table: &SymbolTable) -> Vec<MemoryPattern> {
        let mut patterns = Vec::new();

        // Parameters
        for param in &func.parameters {
            patterns.push(Self::classify_type(
                &param.name,
                &param.param_type,
                StorageClass::Parameter,
            ));
        }

        // Body declarations
        Self::walk_stmts(&func.body, &mut patterns);

        patterns
    }

    fn walk_stmts(stmts: &[CStatement], patterns: &mut Vec<MemoryPattern>) {
        for stmt in stmts {
            match stmt {
                CStatement::VarDecl { name, var_type, init } => {
                    let storage = if matches!(var_type, CType::Array(_, _)) {
                        StorageClass::Stack // arrays are stack-allocated
                    } else if let Some(CExpression::Call { function, .. }) = init {
                        if function.contains("malloc") || function.contains("calloc")
                            || function.contains("realloc")
                        {
                            StorageClass::Heap
                        } else {
                            StorageClass::Stack
                        }
                    } else {
                        StorageClass::Stack
                    };
                    patterns.push(Self::classify_type(name, var_type, storage));
                }
                CStatement::If { then_body, else_body, .. } => {
                    Self::walk_stmts(then_body, patterns);
                    if let Some(eb) = else_body { Self::walk_stmts(eb, patterns); }
                }
                CStatement::While { body, .. } | CStatement::For { body, .. } => {
                    Self::walk_stmts(body, patterns);
                }
                _ => {}
            }
        }
    }

    fn classify_type(name: &str, ctype: &CType, storage: StorageClass) -> MemoryPattern {
        let (is_pointer, pointer_depth) = Self::pointer_depth(ctype);
        let (is_array, array_size) = match ctype {
            CType::Array(_, sz) => (true, *sz),
            _ => (false, None),
        };
        MemoryPattern {
            variable: name.to_string(),
            storage,
            is_pointer,
            is_array,
            pointer_depth,
            is_const: false, // C const qualification tracking would require more parsing
            is_mutable: true,
            size_known: array_size.is_some(),
            array_size,
        }
    }

    fn pointer_depth(ctype: &CType) -> (bool, usize) {
        match ctype {
            CType::Pointer(inner) => {
                let (_, d) = Self::pointer_depth(inner);
                (true, d + 1)
            }
            _ => (false, 0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Alias graph
// ─────────────────────────────────────────────────────────────────────────────

/// Records which variables may alias each other.
#[derive(Debug, Clone, Default)]
pub struct AliasGraph {
    /// Each entry: variable name → set of variables it may alias
    pub edges: HashMap<String, HashSet<String>>,
    /// Variables whose address has been taken
    pub address_taken: HashSet<String>,
    /// Variables assigned from another pointer
    pub pointer_aliases: HashMap<String, String>,
}

impl AliasGraph {
    pub fn may_alias(&self, a: &str, b: &str) -> bool {
        self.edges
            .get(a)
            .map_or(false, |s| s.contains(b))
    }

    pub fn has_aliases(&self, name: &str) -> bool {
        self.edges.get(name).map_or(false, |s| !s.is_empty())
    }
}

pub struct AliasAnalyser;

impl AliasAnalyser {
    pub fn analyse(func: &CFunction, _table: &SymbolTable) -> AliasGraph {
        let mut graph = AliasGraph::default();

        // Walk all statements, looking for pointer assignments
        Self::walk_stmts(&func.body, &mut graph);

        graph
    }

    fn walk_stmts(stmts: &[CStatement], graph: &mut AliasGraph) {
        for stmt in stmts {
            match stmt {
                CStatement::VarDecl { name, var_type, init: Some(init_expr) } => {
                    if matches!(var_type, CType::Pointer(_)) {
                        Self::record_pointer_init(name, init_expr, graph);
                    }
                }
                CStatement::Assignment { target, value } => {
                    Self::record_pointer_init(target, value, graph);
                }
                CStatement::If { then_body, else_body, .. } => {
                    Self::walk_stmts(then_body, graph);
                    if let Some(eb) = else_body { Self::walk_stmts(eb, graph); }
                }
                CStatement::While { body, .. } | CStatement::For { body, .. } => {
                    Self::walk_stmts(body, graph);
                }
                _ => {}
            }
        }
    }

    fn record_pointer_init(target: &str, expr: &CExpression, graph: &mut AliasGraph) {
        match expr {
            CExpression::UnaryOp { op: UnaryOp::AddrOf, operand } => {
                if let CExpression::Variable(src) = operand.as_ref() {
                    graph.address_taken.insert(src.clone());
                    graph
                        .edges
                        .entry(target.to_string())
                        .or_default()
                        .insert(src.clone());
                    graph
                        .edges
                        .entry(src.clone())
                        .or_default()
                        .insert(target.to_string());
                    graph.pointer_aliases.insert(target.to_string(), src.clone());
                }
            }
            CExpression::Variable(src) => {
                // p = q (pointer copy)
                graph
                    .edges
                    .entry(target.to_string())
                    .or_default()
                    .insert(src.clone());
                graph.pointer_aliases.insert(target.to_string(), src.clone());
            }
            _ => {}
        }
    }
}
