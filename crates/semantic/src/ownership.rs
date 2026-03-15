//! # Ownership & Lifetime Inference (Process 0.3.3)
//!
//! Classifies each variable into an ownership category and assigns
//! a lifetime scope, as specified in SRS §4.6.4.

use ast_input::*;
use std::collections::HashMap;
use std::collections::HashSet;
use crate::alias::AliasGraph;
use crate::alias::MemoryPattern;
use crate::alias::StorageClass;
use crate::def_use::DefUseChain;
use crate::symbol_table::SymbolTable;

// ─────────────────────────────────────────────────────────────────────────────
// Ownership classification (SRS §4.6.4)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum OwnershipKind {
    /// `Box<T>` — unique heap ownership
    UniqueOwned,
    /// `Rc<T>` / `Arc<T>` — reference-counted shared ownership
    SharedOwned,
    /// `RefCell<T>` — interior mutability for aliased mutable access
    InteriorMutable,
    /// `&mut T` — mutable borrow (temporary)
    MutableBorrow,
    /// `&T` — immutable borrow (temporary)
    ImmutableBorrow,
    /// Plain value (Copy type, stack allocated, no pointer semantics)
    Value,
    /// Fixed-size stack array `[T; N]`
    StackArray,
    /// Heap-allocated vector `Vec<T>`
    HeapVec,
}

impl OwnershipKind {
    pub fn rust_qualifier(&self) -> &'static str {
        match self {
            OwnershipKind::UniqueOwned => "Box",
            OwnershipKind::SharedOwned => "Rc",
            OwnershipKind::InteriorMutable => "RefCell",
            OwnershipKind::MutableBorrow => "&mut",
            OwnershipKind::ImmutableBorrow => "&",
            OwnershipKind::Value => "",
            OwnershipKind::StackArray => "[T; N]",
            OwnershipKind::HeapVec => "Vec",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lifetime scope
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LifetimeScope {
    pub variable: String,
    /// Earliest statement index at which the variable is live.
    pub live_from: usize,
    /// Latest statement index at which the variable is live.
    pub live_to: usize,
    /// Whether the variable outlives the current block (e.g. returned).
    pub escapes: bool,
    /// Whether the lifetime annotation is needed in the generated Rust.
    pub needs_annotation: bool,
    /// The Rust lifetime label (e.g. `'a`), if needed.
    pub label: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Ownership inferrer
// ─────────────────────────────────────────────────────────────────────────────

pub struct OwnershipInferrer;

impl OwnershipInferrer {
    pub fn infer(
        func: &CFunction,
        alias_graph: &AliasGraph,
        memory_patterns: &[MemoryPattern],
    ) -> HashMap<String, OwnershipKind> {
        let mut map: HashMap<String, OwnershipKind> = HashMap::new();

        // §4.6.4: pointer parameters are &mut T only if they are actually written
        // through in the function body.  Otherwise, classify them as &T (read-only borrow).
        let mutated_ptr_params: HashSet<String> = func
            .parameters
            .iter()
            .filter(|p| matches!(&p.param_type, CType::Pointer(_)))
            .filter(|p| Self::is_ptr_param_mutated(&p.name, &func.body))
            .map(|p| p.name.clone())
            .collect();

        for pattern in memory_patterns {
            let kind = Self::classify(pattern, alias_graph, &mutated_ptr_params);
            map.insert(pattern.variable.clone(), kind);
        }

        map
    }

    // ── Mutation detection ───────────────────────────────────────────────────
    //
    // Returns true when the pointer parameter `name` is written *through* inside
    // `stmts`.  We detect:
    //   • arr[i]  = v   →  IndexedAssignment { array: name }
    //   • s->fld  = v   →  FieldAssignment   { object: starts with name }
    //   • name    = v   →  Assignment        { target: name }  (pointer reassign)
    //   • *name   = v   →  Expression containing a deref-assign (conservative)
    //   • name passed to a call (may be mutated by callee — conservative)
    //
    // Everything else leaves the pointer immutable → &T.
    fn is_ptr_param_mutated(name: &str, stmts: &[CStatement]) -> bool {
        for stmt in stmts {
            match stmt {
                CStatement::IndexedAssignment { array, .. } if array == name => return true,

                CStatement::FieldAssignment { object, .. }
                    if object == name || object.starts_with(&format!("{}.", name)) =>
                {
                    return true;
                }

                CStatement::Assignment { target, value } => {
                    if target == name { return true; }
                    // Deref-assign: `*name = v` shows up as an Expression in the
                    // raw parse but also as an assign-through in some paths.
                    if Self::expr_writes_through(name, value) { return true; }
                }

                // A bare expression statement may contain `*name = v`
                CStatement::Expression(e) => {
                    if Self::expr_writes_through(name, e) { return true; }
                }

                // Recurse into compound statements
                CStatement::If { then_body, else_body, .. } => {
                    if Self::is_ptr_param_mutated(name, then_body) { return true; }
                    if let Some(eb) = else_body {
                        if Self::is_ptr_param_mutated(name, eb) { return true; }
                    }
                }
                CStatement::While { body, .. } | CStatement::DoWhile { body, .. } => {
                    if Self::is_ptr_param_mutated(name, body) { return true; }
                }
                CStatement::For { init, body, .. } => {
                    if Self::is_ptr_param_mutated(name, std::slice::from_ref(init.as_ref())) {
                        return true;
                    }
                    if Self::is_ptr_param_mutated(name, body) { return true; }
                }
                CStatement::Switch { cases, default, .. } => {
                    for (_, b) in cases {
                        if Self::is_ptr_param_mutated(name, b) { return true; }
                    }
                    if let Some(d) = default {
                        if Self::is_ptr_param_mutated(name, d) { return true; }
                    }
                }
                CStatement::Return(Some(e)) => {
                    // Returning the pointer itself isn't a mutation, but returning
                    // a deref-assign expression would be.
                    if Self::expr_writes_through(name, e) { return true; }
                }
                _ => {}
            }
        }
        false
    }

    /// True if expression `e` contains an assignment *through* a pointer named `name`
    /// (e.g. `*name = v`, `name[i] = v` expressed as a BinaryOp{Assign}).
    fn expr_writes_through(name: &str, e: &CExpression) -> bool {
        match e {
            CExpression::BinaryOp { left, op: BinOp::Assign, .. } => {
                match left.as_ref() {
                    // *name = v
                    CExpression::UnaryOp { op: UnaryOp::Deref, operand } => {
                        matches!(operand.as_ref(), CExpression::Variable(n) if n == name)
                    }
                    // name[i] = v  (array-style pointer write)
                    CExpression::ArrayAccess { array, .. } => {
                        matches!(array.as_ref(), CExpression::Variable(n) if n == name)
                    }
                    // name->field = v
                    CExpression::FieldAccess { object, .. } => {
                        matches!(object.as_ref(), CExpression::Variable(n) if n == name)
                    }
                    _ => false,
                }
            }
            // Recurse into nested expressions
            CExpression::BinaryOp { left, right, .. } => {
                Self::expr_writes_through(name, left) || Self::expr_writes_through(name, right)
            }
            _ => false,
        }
    }

    fn classify(
        pattern: &MemoryPattern,
        alias_graph: &AliasGraph,
        mutated_ptr_params: &HashSet<String>,
    ) -> OwnershipKind {
        let has_aliases = alias_graph.has_aliases(&pattern.variable);
        let addr_taken = alias_graph.address_taken.contains(&pattern.variable);

        if pattern.is_array {
            if pattern.size_known {
                return OwnershipKind::StackArray;
            } else {
                return OwnershipKind::HeapVec;
            }
        }

        if !pattern.is_pointer {
            return OwnershipKind::Value;
        }

        match &pattern.storage {
            StorageClass::Heap => {
                if has_aliases {
                    OwnershipKind::SharedOwned
                } else {
                    OwnershipKind::UniqueOwned
                }
            }
            StorageClass::Parameter => {
                // §4.6.4: use &mut T only when we can prove the callee actually
                // writes through the pointer; otherwise use &T.
                if mutated_ptr_params.contains(&pattern.variable) {
                    OwnershipKind::MutableBorrow
                } else {
                    OwnershipKind::ImmutableBorrow
                }
            }
            StorageClass::Stack => {
                if addr_taken && has_aliases {
                    OwnershipKind::InteriorMutable
                } else if addr_taken || has_aliases {
                    OwnershipKind::MutableBorrow
                } else {
                    OwnershipKind::Value
                }
            }
            _ => OwnershipKind::Value,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lifetime inferrer
// ─────────────────────────────────────────────────────────────────────────────

pub struct LifetimeInferrer;

impl LifetimeInferrer {
    pub fn infer(
        func: &CFunction,
        def_use_chains: &HashMap<String, DefUseChain>,
        _table: &SymbolTable,
    ) -> HashMap<String, LifetimeScope> {
        let mut map = HashMap::new();

        // Detect returned variables
        let returned_vars = Self::find_returned_vars(&func.body);

        for (name, chain) in def_use_chains {
            let live_from = chain.definitions.iter().map(|d| d.stmt_index).min().unwrap_or(0);
            let live_to = chain.uses.iter().map(|u| u.stmt_index).max()
                .unwrap_or(live_from);
            let escapes = returned_vars.contains(name.as_str());
            let needs_annotation = chain.address_taken;
            let label = if needs_annotation {
                Some(format!("'a"))
            } else {
                None
            };
            map.insert(
                name.clone(),
                LifetimeScope {
                    variable: name.clone(),
                    live_from,
                    live_to,
                    escapes,
                    needs_annotation,
                    label,
                },
            );
        }

        map
    }

    fn find_returned_vars<'a>(stmts: &'a [CStatement]) -> std::collections::HashSet<&'a str> {
        let mut set = std::collections::HashSet::new();
        for stmt in stmts {
            match stmt {
                CStatement::Return(Some(expr)) => {
                    if let CExpression::Variable(name) = expr {
                        set.insert(name.as_str());
                    }
                }
                CStatement::If { then_body, else_body, .. } => {
                    set.extend(Self::find_returned_vars(then_body));
                    if let Some(eb) = else_body {
                        set.extend(Self::find_returned_vars(eb));
                    }
                }
                CStatement::While { body, .. } | CStatement::For { body, .. } => {
                    set.extend(Self::find_returned_vars(body));
                }
                _ => {}
            }
        }
        set
    }
}
