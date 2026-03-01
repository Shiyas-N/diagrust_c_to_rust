//! # Data Dependency Analyser (Process 0.3.1)
//!
//! Extracts Def-Use chains from the C AST.
//! A "def" is any point where a variable is assigned a value.
//! A "use" is any point where a variable's value is read.

use ast_input::*;
use std::collections::HashMap;
use crate::symbol_table::SymbolTable;

// ─────────────────────────────────────────────────────────────────────────────
// Data structures
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Definition {
    pub variable: String,
    pub stmt_index: usize,   // index within linearised statement list
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
    /// Whether the variable's address is ever taken (&var).
    pub address_taken: bool,
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

        // Parameters are definitions at index 0
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

        Self::process_stmts(&func.body, &mut chains, &mut counter);

        // Mark reassigned variables
        for chain in chains.values_mut() {
            let non_param_defs = chain.definitions.iter().filter(|d| !d.is_parameter).count();
            chain.is_reassigned = non_param_defs > 1
                || (non_param_defs == 1 && chain.definitions.iter().any(|d| d.is_reassignment));
        }

        chains
    }

    fn process_stmts(
        stmts: &[CStatement],
        chains: &mut HashMap<String, DefUseChain>,
        counter: &mut usize,
    ) {
        for stmt in stmts {
            *counter += 1;
            let idx = *counter;
            Self::process_stmt(stmt, chains, idx);
        }
    }

    fn process_stmt(
        stmt: &CStatement,
        chains: &mut HashMap<String, DefUseChain>,
        idx: usize,
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
                    Self::collect_uses(init_expr, chains, idx);
                }
            }
            CStatement::Assignment { target, value } => {
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
                Self::collect_uses(value, chains, idx);
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
                Self::collect_uses(index, chains, idx);
                Self::collect_uses(value, chains, idx);
            }
            CStatement::Return(Some(expr)) => {
                Self::collect_uses(expr, chains, idx);
            }
            CStatement::Expression(expr) => {
                Self::collect_uses(expr, chains, idx);
            }
            CStatement::If { condition, then_body, else_body } => {
                Self::collect_uses(condition, chains, idx);
                let mut sub = idx;
                Self::process_stmts_inner(then_body, chains, &mut sub);
                if let Some(eb) = else_body {
                    Self::process_stmts_inner(eb, chains, &mut sub);
                }
            }
            CStatement::While { condition, body } => {
                Self::collect_uses(condition, chains, idx);
                let mut sub = idx;
                Self::process_stmts_inner(body, chains, &mut sub);
            }
            CStatement::For { init, condition, increment, body } => {
                Self::process_stmt(init, chains, idx);
                Self::collect_uses(condition, chains, idx);
                Self::collect_uses(increment, chains, idx);
                let mut sub = idx;
                Self::process_stmts_inner(body, chains, &mut sub);
            }
            _ => {}
        }
    }

    fn process_stmts_inner(
        stmts: &[CStatement],
        chains: &mut HashMap<String, DefUseChain>,
        counter: &mut usize,
    ) {
        for stmt in stmts {
            *counter += 1;
            let idx = *counter;
            Self::process_stmt(stmt, chains, idx);
        }
    }

    fn collect_uses(expr: &CExpression, chains: &mut HashMap<String, DefUseChain>, idx: usize) {
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
                }
            }
            CExpression::BinaryOp { left, right, .. } => {
                Self::collect_uses(left, chains, idx);
                Self::collect_uses(right, chains, idx);
            }
            CExpression::UnaryOp { operand, .. } => {
                Self::collect_uses(operand, chains, idx);
            }
            CExpression::Call { args, .. } => {
                for a in args { Self::collect_uses(a, chains, idx); }
            }
            CExpression::ArrayAccess { array, index } => {
                Self::collect_uses(array, chains, idx);
                Self::collect_uses(index, chains, idx);
            }
            CExpression::Cast { expr, .. } => {
                Self::collect_uses(expr, chains, idx);
            }
            _ => {}
        }
    }
}
