//! # Ownership & Lifetime Inference (Process 0.3.3) — Enhanced
//!
//! Classifies each variable into an ownership category and assigns
//! a lifetime scope, as specified in SRS §4.6.4.
//!
//! ## Improvements over v1
//!
//! * **Write-through analysis** — pointer parameters are only `&mut T` when
//!   there is at least one write through the pointer (`*p = …`).  Read-only
//!   pointer parameters are now correctly classified as `ImmutableBorrow`.
//!
//! * **`char *` idiom** — `char *` / `char []` variables map to `StringOwned`
//!   (owned `String`) or `StringBorrow` (`&str`) instead of `*mut u8`, giving
//!   idiomatic Rust string handling.
//!
//! * **`RawPointer` fallback** — multi-level or unresolvable pointers are
//!   tagged `RawPointer` so that `type_map` emits `*mut T` inside `unsafe`
//!   instead of generating incorrect safe Rust.
//!
//! * **Multiple lifetime labels** — the `LifetimeInferrer` assigns distinct
//!   labels (`'a`, `'b`, …) to different reference parameters and ties the
//!   return-value lifetime to the correct source parameter.
//!
//! * **Lifetime elision awareness** — a function with exactly one reference
//!   parameter whose return is also a reference uses Rust's elision rules and
//!   does not need explicit annotations.

use ast_input::*;
use std::collections::{HashMap, HashSet};
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
    /// `*mut T` / `*const T` — raw pointer (unsafe fallback for multi-level
    /// pointers or ambiguous cases where safe Rust is not possible)
    RawPointer,
    /// `String` — owned heap string (from `char *` / `char []`)
    StringOwned,
    /// `&str` — borrowed string slice (from `const char *`)
    StringBorrow,
}

impl OwnershipKind {
    pub fn rust_qualifier(&self) -> &'static str {
        match self {
            OwnershipKind::UniqueOwned     => "Box",
            OwnershipKind::SharedOwned     => "Rc",
            OwnershipKind::InteriorMutable => "RefCell",
            OwnershipKind::MutableBorrow   => "&mut",
            OwnershipKind::ImmutableBorrow => "&",
            OwnershipKind::Value           => "",
            OwnershipKind::StackArray      => "[T; N]",
            OwnershipKind::HeapVec         => "Vec",
            OwnershipKind::RawPointer      => "*mut",
            OwnershipKind::StringOwned     => "String",
            OwnershipKind::StringBorrow    => "&str",
        }
    }

    /// Whether this ownership kind forces an `unsafe` context.
    pub fn requires_unsafe(&self) -> bool {
        matches!(self, OwnershipKind::RawPointer)
    }

    /// Whether the resulting type is idiomatic Rust (vs a raw/unsafe fallback).
    pub fn is_idiomatic(&self) -> bool {
        !matches!(self, OwnershipKind::RawPointer)
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
    /// Whether a lifetime annotation is needed in generated Rust.
    pub needs_annotation: bool,
    /// The Rust lifetime label (e.g. `'a`), if needed.
    pub label: Option<String>,
    /// Index into the function's parameter list that this variable's lifetime
    /// is tied to (used by codegen for return-type lifetime matching).
    pub tied_to_param: Option<usize>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Ownership inferrer
// ─────────────────────────────────────────────────────────────────────────────

pub struct OwnershipInferrer;

impl OwnershipInferrer {
    /// Infer ownership for all variables in `func`.
    ///
    /// `def_use_chains` is used to determine whether a pointer parameter is
    /// ever written through (→ `&mut T`) or is read-only (→ `&T`).
    pub fn infer(
        _func: &CFunction,
        alias_graph: &AliasGraph,
        memory_patterns: &[MemoryPattern],
        def_use_chains: &HashMap<String, DefUseChain>,
    ) -> HashMap<String, OwnershipKind> {
        let mut map: HashMap<String, OwnershipKind> = HashMap::new();

        for pattern in memory_patterns {
            let kind = Self::classify(pattern, alias_graph, def_use_chains);
            map.insert(pattern.variable.clone(), kind);
        }

        map
    }

    fn classify(
        pattern: &MemoryPattern,
        alias_graph: &AliasGraph,
        def_use_chains: &HashMap<String, DefUseChain>,
    ) -> OwnershipKind {
        let has_aliases = alias_graph.has_aliases(&pattern.variable);
        let addr_taken  = alias_graph.address_taken.contains(&pattern.variable);

        // ── Array types ──────────────────────────────────────────────────────
        if pattern.is_array {
            return if pattern.size_known {
                OwnershipKind::StackArray
            } else {
                OwnershipKind::HeapVec
            };
        }

        // ── Non-pointer values ───────────────────────────────────────────────
        if !pattern.is_pointer {
            return OwnershipKind::Value;
        }

        // ── char * / char [] — translate to String / &str ───────────────────
        if pattern.is_char_ptr {
            return match &pattern.storage {
                StorageClass::Parameter => {
                    // Read-only char* parameter → &str (idiomatic Rust).
                    // Written-through char* (e.g. strcpy target) → String.
                    let written = def_use_chains
                        .get(&pattern.variable)
                        .map(|c| c.has_pointer_write)
                        .unwrap_or(false);
                    if pattern.is_const || !written {
                        OwnershipKind::StringBorrow
                    } else {
                        OwnershipKind::StringOwned
                    }
                }
                // Local stack / heap char* → owned String
                _ => OwnershipKind::StringOwned,
            };
        }

        // ── Multi-level pointers (int**, char**, …) ──────────────────────────
        // Cannot be safely expressed as borrows in general.
        // Emit *mut *mut T with an unsafe block.
        if pattern.pointer_depth > 1 {
            return OwnershipKind::RawPointer;
        }

        // ── Single-level pointers ────────────────────────────────────────────
        match &pattern.storage {
            StorageClass::Heap => {
                if has_aliases {
                    OwnershipKind::SharedOwned
                } else {
                    OwnershipKind::UniqueOwned
                }
            }

            StorageClass::Parameter => {
                // Determine from write-through analysis whether the pointer
                // parameter is mutably used.
                let written_through =
                    def_use_chains.get(&pattern.variable).map(|c| c.has_pointer_write).unwrap_or(false)
                    || alias_graph.written_through.contains(&pattern.variable);

                if pattern.is_const {
                    OwnershipKind::ImmutableBorrow
                } else if written_through {
                    OwnershipKind::MutableBorrow
                } else {
                    // Read-only pointer parameter → idiomatic &T
                    OwnershipKind::ImmutableBorrow
                }
            }

            StorageClass::Stack => {
                if addr_taken && has_aliases {
                    OwnershipKind::InteriorMutable
                } else if addr_taken || has_aliases {
                    let written = def_use_chains
                        .get(&pattern.variable)
                        .map(|c| c.has_pointer_write || c.is_reassigned)
                        .unwrap_or(false);
                    if written {
                        OwnershipKind::MutableBorrow
                    } else {
                        OwnershipKind::ImmutableBorrow
                    }
                } else {
                    OwnershipKind::Value
                }
            }

            // Static / global pointers — raw pointer, requires unsafe
            StorageClass::Static => OwnershipKind::RawPointer,

            StorageClass::Unknown => OwnershipKind::Value,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lifetime label generator
// ─────────────────────────────────────────────────────────────────────────────

struct LifetimeLabelGen { next: usize }

impl LifetimeLabelGen {
    fn new() -> Self { Self { next: 0 } }

    fn next_label(&mut self) -> String {
        let label = match self.next {
            0 => "'a".to_string(),
            1 => "'b".to_string(),
            2 => "'c".to_string(),
            n => format!("'lt{}", n),
        };
        self.next += 1;
        label
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

        // Variables returned from this function.
        let returned_vars = Self::find_returned_vars(&func.body);

        // Reference-type parameters (pointers / arrays).
        let ref_params: Vec<&CParameter> = func.parameters.iter()
            .filter(|p| Self::is_reference_type(&p.param_type))
            .collect();

        let ret_is_ref = Self::is_reference_type(&func.return_type);

        // Rust lifetime elision rules:
        //   0 ref params → no annotation ever needed
        //   1 ref param + ref return → elision applies, no explicit annotation
        //   2+ ref params + ref return → explicit annotations required
        let needs_multi_lifetime = ret_is_ref && ref_params.len() > 1;
        let elision_applies      = ret_is_ref && ref_params.len() == 1;

        // Assign a distinct lifetime label to each reference parameter.
        let mut gen = LifetimeLabelGen::new();
        let mut param_labels: HashMap<String, String> = HashMap::new();
        if needs_multi_lifetime {
            for p in &ref_params {
                let label = gen.next_label();
                param_labels.insert(p.name.clone(), label);
            }
        }

        // Which parameter does the returned value borrow from?
        // We check whether any returned variable name matches a param name, or
        // whether its def-use chain records it was sourced from a param.
        let return_source_param: Option<&str> = returned_vars.iter().find_map(|rv| {
            if ref_params.iter().any(|p| p.name == *rv) {
                return Some(*rv);
            }
            if let Some(chain) = def_use_chains.get(*rv) {
                for p in &ref_params {
                    if chain.sourced_from_params.contains(&p.name) {
                        return Some(p.name.as_str());
                    }
                }
            }
            None
        });

        for (name, chain) in def_use_chains {
            let live_from = chain.definitions.iter().map(|d| d.stmt_index).min().unwrap_or(0);
            let live_to   = chain.uses.iter().map(|u| u.stmt_index).max().unwrap_or(live_from);
            let escapes   = returned_vars.contains(name.as_str());

            let is_ref_param = ref_params.iter().any(|p| &p.name == name);

            // Decide whether an explicit annotation is needed.
            let needs_annotation = if elision_applies {
                false
            } else if needs_multi_lifetime {
                is_ref_param || (escapes && ret_is_ref)
            } else {
                // No return borrow: only annotate address-taken multi-ref cases.
                chain.address_taken && ref_params.len() > 1
            };

            // Choose the lifetime label.
            let label: Option<String> = if needs_annotation {
                if is_ref_param {
                    Some(param_labels.get(name).cloned().unwrap_or_else(|| "'a".to_string()))
                } else if escapes && ret_is_ref {
                    let src = return_source_param
                        .and_then(|sp| param_labels.get(sp))
                        .or_else(|| ref_params.first().and_then(|p| param_labels.get(&p.name)));
                    Some(src.cloned().unwrap_or_else(|| "'a".to_string()))
                } else {
                    None
                }
            } else {
                None
            };

            // Which parameter index is this variable's lifetime tied to?
            let tied_to_param = if escapes && ret_is_ref {
                return_source_param.and_then(|sp| {
                    func.parameters.iter().position(|p| p.name == sp)
                })
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
                    tied_to_param,
                },
            );
        }

        map
    }

    fn is_reference_type(ctype: &CType) -> bool {
        matches!(ctype, CType::Pointer(_) | CType::Array(_, _))
    }

    fn find_returned_vars<'a>(stmts: &'a [CStatement]) -> HashSet<&'a str> {
        let mut set = HashSet::new();
        for stmt in stmts {
            match stmt {
                CStatement::Return(Some(expr)) => {
                    Self::collect_expr_vars(expr, &mut set);
                }
                CStatement::If { then_body, else_body, .. } => {
                    set.extend(Self::find_returned_vars(then_body));
                    if let Some(eb) = else_body {
                        set.extend(Self::find_returned_vars(eb));
                    }
                }
                CStatement::While { body, .. }
                | CStatement::For { body, .. }
                | CStatement::DoWhile { body, .. } => {
                    set.extend(Self::find_returned_vars(body));
                }
                CStatement::Switch { cases, default, .. } => {
                    for (_, body) in cases {
                        set.extend(Self::find_returned_vars(body));
                    }
                    if let Some(d) = default {
                        set.extend(Self::find_returned_vars(d));
                    }
                }
                _ => {}
            }
        }
        set
    }

    fn collect_expr_vars<'a>(expr: &'a CExpression, set: &mut HashSet<&'a str>) {
        match expr {
            CExpression::Variable(name)         => { set.insert(name.as_str()); }
            CExpression::BinaryOp { left, right, .. } => {
                Self::collect_expr_vars(left, set);
                Self::collect_expr_vars(right, set);
            }
            CExpression::UnaryOp { operand, .. } => Self::collect_expr_vars(operand, set),
            CExpression::Call { args, .. }       => {
                for a in args { Self::collect_expr_vars(a, set); }
            }
            CExpression::ArrayAccess { array, index } => {
                Self::collect_expr_vars(array, set);
                Self::collect_expr_vars(index, set);
            }
            CExpression::FieldAccess { object, .. } => Self::collect_expr_vars(object, set),
            _ => {}
        }
    }
}
