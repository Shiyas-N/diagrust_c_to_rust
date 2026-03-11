//! # Data Dependency Analyser (Process 0.3.1) — Enhanced
//!
//! Extracts Def-Use chains from the C AST.
//!
//! ## Improvements over v1
//!
//! * **`has_pointer_write`** — set to `true` whenever a write *through* a pointer
//!   is detected (`*p = value`, deref-assign in expression form).  Used by
//!   `OwnershipInferrer` to distinguish `&T` from `&mut T` for pointer parameters.
//!
//! * **`sourced_from_params`** — records which parameter names contributed to
//!   this variable's value (via direct assignment or aliasing).  Consumed by
//!   `LifetimeInferrer` to tie return-value lifetimes back to the correct
//!   input parameter.
//!
//! * **Deref-write collection** — `UnaryOp::Deref` on the left-hand side of an
//!   assignment is now correctly identified and the pointed-to variable is
//!   marked as written-through.
//!
//! * **Address-of in expressions** — `&x` inside a call argument or assignment
//!   rhs now propagates `address_taken = true` to `x`.

use ast_input::*;
use std::collections::{HashMap, HashSet};
use crate::symbol_table::SymbolTable;

// ─────────────────────────────────────────────────────────────────────────────
// Data structures
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Definition {
    pub variable: String,
    pub stmt_index: usize,
    pub is_parameter: bool,
    pub is_reassignment: bool,
}

#[derive(Debug, Clone)]
pub struct Use {
    pub variable: String,
    pub stmt_index: usize,
}

/// All definitions and uses for a single variable.
#[derive(Debug, Clone, Default)]
pub struct DefUseChain {
    pub variable: String,
    pub definitions: Vec<Definition>,
    pub uses: Vec<Use>,
    /// Whether the variable is assigned more than once (excluding initial decl).
    pub is_reassigned: bool,
    /// Whether the variable's address is ever taken (`&var`).
    pub address_taken: bool,
    /// Whether a write *through* this pointer occurs (`*var = …`).
    /// Set to `true` for the pointed-to variable when a deref-write is seen.
    pub has_pointer_write: bool,
    /// Names of parameters whose values flow into this variable
    /// (direct assignment or aliasing).  Used by `LifetimeInferrer` to
    /// associate return lifetimes with the correct input parameter.
    pub sourced_from_params: HashSet<String>,
}

impl DefUseChain {
    pub fn def_count(&self) -> usize { self.definitions.len() }
    pub fn use_count(&self) -> usize { self.uses.len() }

    pub fn is_live_at(&self, stmt: usize) -> bool {
        self.definitions.iter().any(|d| d.stmt_index <= stmt)
            && self.uses.iter().any(|u| u.stmt_index >= stmt)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Analyser
// ─────────────────────────────────────────────────────────────────────────────

pub struct DefUseAnalyser;

impl DefUseAnalyser {
    pub fn analyse(func: &CFunction, _table: &SymbolTable) -> HashMap<String, DefUseChain> {
        let mut chains: HashMap<String, DefUseChain> = HashMap::new();
        let mut counter = 0usize;

        // Collect parameter names for `sourced_from_params` tracking.
        let param_names: HashSet<String> = func.parameters.iter()
            .map(|p| p.name.clone())
            .collect();

        // Parameters are definitions at index 0.
        for param in &func.parameters {
            let entry = chains.entry(param.name.clone()).or_insert_with(|| DefUseChain {
                variable: param.name.clone(),
                ..Default::default()
            });
            entry.definitions.push(Definition {
                variable: param.name.clone(),
                stmt_index: 0,
                is_parameter: true,
                is_reassignment: false,
            });
        }

        Self::process_stmts(&func.body, &mut chains, &mut counter, &param_names);

        // Post-pass: mark reassigned variables.
        for chain in chains.values_mut() {
            let non_param_defs = chain.definitions.iter().filter(|d| !d.is_parameter).count();
            chain.is_reassigned = non_param_defs > 1
                || (non_param_defs == 1 && chain.definitions.iter().any(|d| d.is_reassignment));
        }

        chains
    }

    // ── Statement processing ─────────────────────────────────────────────────

    fn process_stmts(
        stmts: &[CStatement],
        chains: &mut HashMap<String, DefUseChain>,
        counter: &mut usize,
        params: &HashSet<String>,
    ) {
        for stmt in stmts {
            *counter += 1;
            let idx = *counter;
            Self::process_stmt(stmt, chains, idx, params);
        }
    }

    fn process_stmt(
        stmt: &CStatement,
        chains: &mut HashMap<String, DefUseChain>,
        idx: usize,
        params: &HashSet<String>,
    ) {
        match stmt {
            CStatement::VarDecl { name, init, .. } => {
                let entry = chains.entry(name.clone()).or_insert_with(|| DefUseChain {
                    variable: name.clone(),
                    ..Default::default()
                });
                entry.definitions.push(Definition {
                    variable: name.clone(),
                    stmt_index: idx,
                    is_parameter: false,
                    is_reassignment: false,
                });
                if let Some(init_expr) = init {
                    // Track whether the init value comes from a parameter.
                    let sourced = Self::params_in_expr(init_expr, params);
                    chains.entry(name.clone()).or_default().sourced_from_params.extend(sourced);
                    Self::collect_uses(init_expr, chains, idx, params);
                }
            }

            CStatement::Assignment { target, value } => {
                // Check for deref-write: `*p = value`
                // The AST represents this as target = "*varname" (string) OR as
                // the Assignment variant's target being a deref expression.
                // In the current AST, target is a plain String — we detect the
                // leading `*` convention used by some parsers.
                if target.starts_with('*') {
                    let actual = target.trim_start_matches('*').trim();
                    let entry = chains.entry(actual.to_string()).or_insert_with(|| DefUseChain {
                        variable: actual.to_string(),
                        ..Default::default()
                    });
                    entry.has_pointer_write = true;
                    entry.uses.push(Use { variable: actual.to_string(), stmt_index: idx });
                } else {
                    let entry = chains.entry(target.clone()).or_insert_with(|| DefUseChain {
                        variable: target.clone(),
                        ..Default::default()
                    });
                    entry.definitions.push(Definition {
                        variable: target.clone(),
                        stmt_index: idx,
                        is_parameter: false,
                        is_reassignment: !entry.definitions.is_empty(),
                    });
                    // Track parameter sources for the assigned variable.
                    let sourced = Self::params_in_expr(value, params);
                    chains.entry(target.clone()).or_default().sourced_from_params.extend(sourced);
                }
                // Also detect deref-writes via UnaryOp in the value expression.
                Self::collect_deref_writes(value, chains, idx);
                Self::collect_uses(value, chains, idx, params);
            }

            CStatement::IndexedAssignment { array, index, value } => {
                let entry = chains.entry(array.clone()).or_insert_with(|| DefUseChain {
                    variable: array.clone(),
                    ..Default::default()
                });
                entry.definitions.push(Definition {
                    variable: array.clone(),
                    stmt_index: idx,
                    is_parameter: false,
                    is_reassignment: true,
                });
                Self::collect_uses(index, chains, idx, params);
                Self::collect_uses(value, chains, idx, params);
            }

            CStatement::Return(Some(expr)) => {
                Self::collect_uses(expr, chains, idx, params);
            }

            CStatement::Expression(expr) => {
                Self::collect_uses(expr, chains, idx, params);
                Self::collect_deref_writes(expr, chains, idx);
            }

            CStatement::If { condition, then_body, else_body } => {
                Self::collect_uses(condition, chains, idx, params);
                let mut sub = idx;
                Self::process_stmts(then_body, chains, &mut sub, params);
                if let Some(eb) = else_body {
                    Self::process_stmts(eb, chains, &mut sub, params);
                }
            }

            CStatement::While { condition, body } => {
                Self::collect_uses(condition, chains, idx, params);
                let mut sub = idx;
                Self::process_stmts(body, chains, &mut sub, params);
            }

            CStatement::DoWhile { body, condition } => {
                let mut sub = idx;
                Self::process_stmts(body, chains, &mut sub, params);
                Self::collect_uses(condition, chains, sub, params);
            }

            CStatement::For { init, condition, increment, body } => {
                Self::process_stmt(init, chains, idx, params);
                Self::collect_uses(condition, chains, idx, params);
                Self::collect_uses(increment, chains, idx, params);
                let mut sub = idx;
                Self::process_stmts(body, chains, &mut sub, params);
            }

            CStatement::Switch { expr, cases, default } => {
                Self::collect_uses(expr, chains, idx, params);
                let mut sub = idx;
                for (_, body) in cases {
                    Self::process_stmts(body, chains, &mut sub, params);
                }
                if let Some(d) = default {
                    Self::process_stmts(d, chains, &mut sub, params);
                }
            }

            _ => {}
        }
    }

    // ── Use collection ───────────────────────────────────────────────────────

    fn collect_uses(
        expr: &CExpression,
        chains: &mut HashMap<String, DefUseChain>,
        idx: usize,
        params: &HashSet<String>,
    ) {
        match expr {
            CExpression::Variable(name) => {
                let entry = chains.entry(name.clone()).or_insert_with(|| DefUseChain {
                    variable: name.clone(),
                    ..Default::default()
                });
                entry.uses.push(Use { variable: name.clone(), stmt_index: idx });
            }

            CExpression::UnaryOp { op: UnaryOp::AddrOf, operand } => {
                if let CExpression::Variable(name) = operand.as_ref() {
                    let entry = chains.entry(name.clone()).or_insert_with(|| DefUseChain {
                        variable: name.clone(),
                        ..Default::default()
                    });
                    entry.address_taken = true;
                    entry.uses.push(Use { variable: name.clone(), stmt_index: idx });
                } else {
                    Self::collect_uses(operand, chains, idx, params);
                }
            }

            // Deref reads (`*p` on the rhs) — just record the use of `p`.
            CExpression::UnaryOp { op: UnaryOp::Deref, operand } => {
                Self::collect_uses(operand, chains, idx, params);
            }

            CExpression::BinaryOp { left, op: BinOp::Assign, right } => {
                // Deref-write on the lhs: `*p = value`
                if let CExpression::UnaryOp { op: UnaryOp::Deref, operand } = left.as_ref() {
                    if let CExpression::Variable(name) = operand.as_ref() {
                        let entry = chains.entry(name.clone()).or_insert_with(|| DefUseChain {
                            variable: name.clone(),
                            ..Default::default()
                        });
                        entry.has_pointer_write = true;
                        entry.uses.push(Use { variable: name.clone(), stmt_index: idx });
                    }
                } else {
                    Self::collect_uses(left, chains, idx, params);
                }
                Self::collect_uses(right, chains, idx, params);
            }

            CExpression::BinaryOp { left, right, .. } => {
                Self::collect_uses(left, chains, idx, params);
                Self::collect_uses(right, chains, idx, params);
            }

            CExpression::UnaryOp { operand, .. } => {
                Self::collect_uses(operand, chains, idx, params);
            }

            CExpression::Call { args, .. } => {
                for a in args {
                    Self::collect_uses(a, chains, idx, params);
                    // Conservatively: &x passed to a function means x might be
                    // written through by the callee.
                    if let CExpression::UnaryOp { op: UnaryOp::AddrOf, operand } = a {
                        if let CExpression::Variable(name) = operand.as_ref() {
                            let entry = chains.entry(name.clone()).or_insert_with(|| DefUseChain {
                                variable: name.clone(),
                                ..Default::default()
                            });
                            entry.has_pointer_write = true;
                            entry.address_taken = true;
                        }
                    }
                }
            }

            CExpression::ArrayAccess { array, index } => {
                Self::collect_uses(array, chains, idx, params);
                Self::collect_uses(index, chains, idx, params);
            }

            CExpression::Cast { expr, .. } => {
                Self::collect_uses(expr, chains, idx, params);
            }

            CExpression::FieldAccess { object, .. } => {
                Self::collect_uses(object, chains, idx, params);
            }

            _ => {}
        }
    }

    /// Collect deref-write markers from an expression (e.g. `*p = v` when the
    /// assignment is embedded in an expression statement).
    fn collect_deref_writes(
        expr: &CExpression,
        chains: &mut HashMap<String, DefUseChain>,
        idx: usize,
    ) {
        match expr {
            CExpression::BinaryOp { op: BinOp::Assign, left, right } => {
                if let CExpression::UnaryOp { op: UnaryOp::Deref, operand } = left.as_ref() {
                    if let CExpression::Variable(name) = operand.as_ref() {
                        let entry = chains.entry(name.clone()).or_insert_with(|| DefUseChain {
                            variable: name.clone(),
                            ..Default::default()
                        });
                        entry.has_pointer_write = true;
                        entry.uses.push(Use { variable: name.clone(), stmt_index: idx });
                    }
                }
                Self::collect_deref_writes(right, chains, idx);
            }
            CExpression::Call { args, .. } => {
                for a in args { Self::collect_deref_writes(a, chains, idx); }
            }
            CExpression::BinaryOp { left, right, .. } => {
                Self::collect_deref_writes(left, chains, idx);
                Self::collect_deref_writes(right, chains, idx);
            }
            _ => {}
        }
    }

    // ── Parameter-source tracking ─────────────────────────────────────────────

    /// Return the set of parameter names that appear in `expr`.
    fn params_in_expr(expr: &CExpression, params: &HashSet<String>) -> HashSet<String> {
        let mut found = HashSet::new();
        Self::collect_param_uses(expr, params, &mut found);
        found
    }

    fn collect_param_uses(
        expr: &CExpression,
        params: &HashSet<String>,
        found: &mut HashSet<String>,
    ) {
        match expr {
            CExpression::Variable(name) => {
                if params.contains(name) { found.insert(name.clone()); }
            }
            CExpression::BinaryOp { left, right, .. } => {
                Self::collect_param_uses(left, params, found);
                Self::collect_param_uses(right, params, found);
            }
            CExpression::UnaryOp { operand, .. } => {
                Self::collect_param_uses(operand, params, found);
            }
            CExpression::Call { args, .. } => {
                for a in args { Self::collect_param_uses(a, params, found); }
            }
            CExpression::ArrayAccess { array, index } => {
                Self::collect_param_uses(array, params, found);
                Self::collect_param_uses(index, params, found);
            }
            CExpression::Cast { expr, .. } => {
                Self::collect_param_uses(expr, params, found);
            }
            CExpression::FieldAccess { object, .. } => {
                Self::collect_param_uses(object, params, found);
            }
            _ => {}
        }
    }
}
