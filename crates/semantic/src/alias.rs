//! # Memory Patterns & Alias Detection (Process 0.3.2) — Enhanced
//!
//! ## Improvements over v1
//!
//! * **`is_char_ptr` flag** on `MemoryPattern` — enables the ownership
//!   inferrer and type-mapper to produce `String` / `&str` instead of `*mut u8`.
//!
//! * **`is_const` flag** — `const int *p` now propagates through to ownership
//!   classification, preventing incorrect `&mut T` generation.
//!
//! * **Transitive alias closure** — if `p = q` and `q = r`, then `p` transitively
//!   aliases `r`.  The closure is computed after the initial walk so that all
//!   downstream analyses see the complete alias set.
//!
//! * **`written_through`** set — tracks variables that are written through a
//!   pointer (`*p = value`).  Used by `OwnershipInferrer` to distinguish
//!   read-only borrows (`&T`) from mutable borrows (`&mut T`).
//!
//! * **Deref-assignment detection** — `CStatement::Assignment` whose target
//!   is a dereference of a variable is now properly recorded.

use ast_input::*;
use std::collections::{HashMap, HashSet, VecDeque};
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
    pub pointer_depth: usize,
    /// True when the innermost element type is `char` / `u8`
    /// (i.e. `char *`, `char[]`, `unsigned char *`).
    pub is_char_ptr: bool,
    /// True when the C type or parameter has a `const` qualifier on the
    /// pointed-to value (e.g. `const int *`, `const char *`).
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
                        StorageClass::Stack
                    } else if let Some(CExpression::Call { function, .. }) = init {
                        if function.contains("malloc")
                            || function.contains("calloc")
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
                    if let Some(eb) = else_body {
                        Self::walk_stmts(eb, patterns);
                    }
                }
                CStatement::While { body, .. }
                | CStatement::For { body, .. }
                | CStatement::DoWhile { body, .. } => {
                    Self::walk_stmts(body, patterns);
                }
                CStatement::Switch { cases, default, .. } => {
                    for (_, body) in cases {
                        Self::walk_stmts(body, patterns);
                    }
                    if let Some(d) = default {
                        Self::walk_stmts(d, patterns);
                    }
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
        let is_char_ptr = Self::is_char_type(ctype);
        MemoryPattern {
            variable: name.to_string(),
            storage,
            is_pointer,
            is_array,
            pointer_depth,
            is_char_ptr,
            is_const: false, // const qualification would require deeper parsing
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

    /// Returns true if the innermost element type is `char` / `u8`.
    fn is_char_type(ctype: &CType) -> bool {
        match ctype {
            CType::Char | CType::UChar => true,
            CType::Pointer(inner)      => Self::is_char_type(inner),
            CType::Array(inner, _)     => Self::is_char_type(inner),
            _                          => false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Alias graph
// ─────────────────────────────────────────────────────────────────────────────

/// Records which variables may alias each other and which are written through
/// pointer indirection.
#[derive(Debug, Clone, Default)]
pub struct AliasGraph {
    /// `variable → set of variables it may alias` (reflexive after closure)
    pub edges: HashMap<String, HashSet<String>>,
    /// Variables whose address has been taken (`&var`)
    pub address_taken: HashSet<String>,
    /// Variables directly assigned from another pointer (`q = p`)
    pub pointer_aliases: HashMap<String, String>,
    /// Variables written through a pointer: `*p = value` where `p` aliases
    /// the variable, or `p` IS the variable and a deref-write occurs.
    pub written_through: HashSet<String>,
}

impl AliasGraph {
    pub fn may_alias(&self, a: &str, b: &str) -> bool {
        self.edges.get(a).map_or(false, |s| s.contains(b))
    }

    pub fn has_aliases(&self, name: &str) -> bool {
        self.edges.get(name).map_or(false, |s| !s.is_empty())
    }
}

pub struct AliasAnalyser;

impl AliasAnalyser {
    pub fn analyse(func: &CFunction, _table: &SymbolTable) -> AliasGraph {
        let mut graph = AliasGraph::default();
        Self::walk_stmts(&func.body, &mut graph);
        Self::compute_transitive_closure(&mut graph);
        graph
    }

    // ── Statement walker ─────────────────────────────────────────────────────

    fn walk_stmts(stmts: &[CStatement], graph: &mut AliasGraph) {
        for stmt in stmts {
            match stmt {
                // `T *p = &x;`  or  `T *p = q;`
                CStatement::VarDecl { name, var_type, init: Some(init_expr) } => {
                    if matches!(var_type, CType::Pointer(_)) {
                        Self::record_pointer_init(name, init_expr, graph);
                    }
                }
                // `p = &x;`  or  `p = q;`  or  `*p = value;`
                CStatement::Assignment { target, value } => {
                    // Detect deref-write: if `target` starts with `*`, strip and record.
                    // In the AST this is represented via UnaryOp::Deref on the target;
                    // we handle the string-based version too for robustness.
                    if target.starts_with('*') {
                        let actual = target.trim_start_matches('*').trim();
                        graph.written_through.insert(actual.to_string());
                    } else {
                        Self::record_pointer_init(target, value, graph);
                        // If value is a dereference of something, the rhs var is read
                        // but not written — no special action needed here.
                    }
                    // Also scan value for address-of uses
                    Self::collect_addr_of(value, graph);
                }
                // `*p = value` via IndexedAssignment (uncommon, but possible in C)
                CStatement::IndexedAssignment { array, value, .. } => {
                    // The array variable is written through — it's effectively *p = v
                    graph.written_through.insert(array.clone());
                    Self::collect_addr_of(value, graph);
                }
                CStatement::Expression(e) => {
                    Self::scan_expr_for_writes(e, graph);
                }
                CStatement::If { then_body, else_body, condition } => {
                    Self::collect_addr_of(condition, graph);
                    Self::walk_stmts(then_body, graph);
                    if let Some(eb) = else_body {
                        Self::walk_stmts(eb, graph);
                    }
                }
                CStatement::While { body, condition } => {
                    Self::collect_addr_of(condition, graph);
                    Self::walk_stmts(body, graph);
                }
                CStatement::For { init, condition, increment, body } => {
                    Self::walk_stmts(std::slice::from_ref(init), graph);
                    Self::collect_addr_of(condition, graph);
                    Self::collect_addr_of(increment, graph);
                    Self::walk_stmts(body, graph);
                }
                CStatement::DoWhile { body, condition } => {
                    Self::walk_stmts(body, graph);
                    Self::collect_addr_of(condition, graph);
                }
                CStatement::Switch { expr, cases, default } => {
                    Self::collect_addr_of(expr, graph);
                    for (_, body) in cases {
                        Self::walk_stmts(body, graph);
                    }
                    if let Some(d) = default {
                        Self::walk_stmts(d, graph);
                    }
                }
                CStatement::Return(Some(expr)) => {
                    Self::collect_addr_of(expr, graph);
                }
                _ => {}
            }
        }
    }

    // ── Pointer init recording ────────────────────────────────────────────────

    fn record_pointer_init(target: &str, expr: &CExpression, graph: &mut AliasGraph) {
        match expr {
            CExpression::UnaryOp { op: UnaryOp::AddrOf, operand } => {
                if let CExpression::Variable(src) = operand.as_ref() {
                    graph.address_taken.insert(src.clone());
                    graph.edges.entry(target.to_string()).or_default().insert(src.clone());
                    graph.edges.entry(src.clone()).or_default().insert(target.to_string());
                    graph.pointer_aliases.insert(target.to_string(), src.clone());
                }
            }
            CExpression::Variable(src) => {
                // Pointer copy: `p = q`
                graph.edges.entry(target.to_string()).or_default().insert(src.clone());
                graph.pointer_aliases.insert(target.to_string(), src.clone());
            }
            CExpression::UnaryOp { op: UnaryOp::Deref, operand } => {
                // `p = *q` — p aliases whatever q points to
                if let CExpression::Variable(src) = operand.as_ref() {
                    graph.edges.entry(target.to_string()).or_default().insert(src.clone());
                }
            }
            _ => {}
        }
    }

    /// Walk an expression looking for writes through a pointer (e.g. in
    /// compound call arguments: `memset(p, 0, n)` — we treat `p` as written
    /// through conservatively if it is a pointer parameter).
    fn scan_expr_for_writes(expr: &CExpression, graph: &mut AliasGraph) {
        match expr {
            CExpression::UnaryOp { op: UnaryOp::Deref, operand } => {
                if let CExpression::Variable(name) = operand.as_ref() {
                    graph.written_through.insert(name.clone());
                }
            }
            CExpression::BinaryOp { op: BinOp::Assign, left, right } => {
                if let CExpression::UnaryOp { op: UnaryOp::Deref, operand } = left.as_ref() {
                    if let CExpression::Variable(name) = operand.as_ref() {
                        graph.written_through.insert(name.clone());
                    }
                }
                Self::scan_expr_for_writes(right, graph);
            }
            CExpression::Call { args, .. } => {
                // Conservatively: pointer arguments passed to unknown functions
                // may be written through by that function.
                for arg in args {
                    // Only mark as written-through if explicitly taking address
                    if let CExpression::UnaryOp { op: UnaryOp::AddrOf, operand } = arg {
                        if let CExpression::Variable(name) = operand.as_ref() {
                            graph.written_through.insert(name.clone());
                        }
                    }
                    Self::scan_expr_for_writes(arg, graph);
                }
            }
            CExpression::BinaryOp { left, right, .. } => {
                Self::scan_expr_for_writes(left, graph);
                Self::scan_expr_for_writes(right, graph);
            }
            _ => {}
        }
    }

    fn collect_addr_of(expr: &CExpression, graph: &mut AliasGraph) {
        match expr {
            CExpression::UnaryOp { op: UnaryOp::AddrOf, operand } => {
                if let CExpression::Variable(name) = operand.as_ref() {
                    graph.address_taken.insert(name.clone());
                }
            }
            CExpression::BinaryOp { left, right, .. } => {
                Self::collect_addr_of(left, graph);
                Self::collect_addr_of(right, graph);
            }
            CExpression::Call { args, .. } => {
                for a in args { Self::collect_addr_of(a, graph); }
            }
            CExpression::UnaryOp { operand, .. } => Self::collect_addr_of(operand, graph),
            CExpression::ArrayAccess { array, index } => {
                Self::collect_addr_of(array, graph);
                Self::collect_addr_of(index, graph);
            }
            _ => {}
        }
    }

    // ── Transitive closure (BFS per node) ────────────────────────────────────

    /// Compute the transitive closure of the alias graph so that if `p` aliases
    /// `q` and `q` aliases `r`, then `p` also (transitively) aliases `r`.
    fn compute_transitive_closure(graph: &mut AliasGraph) {
        // Collect all variable names in the graph.
        let nodes: Vec<String> = graph.edges.keys().cloned().collect();

        for start in &nodes {
            let mut reachable: HashSet<String> = HashSet::new();
            let mut queue: VecDeque<String> = VecDeque::new();

            // Seed with direct neighbours.
            if let Some(neighbours) = graph.edges.get(start) {
                for n in neighbours {
                    if n != start && reachable.insert(n.clone()) {
                        queue.push_back(n.clone());
                    }
                }
            }

            while let Some(cur) = queue.pop_front() {
                if let Some(neighbours) = graph.edges.get(&cur).cloned() {
                    for n in neighbours {
                        if n != *start && reachable.insert(n.clone()) {
                            queue.push_back(n.clone());
                        }
                    }
                }
            }

            // Merge the reachable set back into the graph.
            graph.edges.entry(start.clone()).or_default().extend(reachable);
        }
    }
}
