//! # Ownership & Lifetime Inference (Process 0.3.3)
//!
//! Classifies each variable into an ownership category and assigns
//! a lifetime scope, as specified in SRS §4.6.4.

use ast_input::*;
use std::collections::HashMap;
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

        for pattern in memory_patterns {
            let kind = Self::classify(pattern, alias_graph);
            map.insert(pattern.variable.clone(), kind);
        }

        map
    }

    fn classify(pattern: &MemoryPattern, alias_graph: &AliasGraph) -> OwnershipKind {
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
                if has_aliases || addr_taken {
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
