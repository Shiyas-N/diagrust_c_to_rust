//! Symbol table – tracks variable declarations and their types across scopes.

use ast_input::*;
use std::collections::HashMap;

/// A single symbol (variable / parameter) in the table.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub c_type: CType,
    pub scope_depth: usize,
    pub is_parameter: bool,
    pub is_pointer: bool,
    pub is_array: bool,
    pub declaration_line: usize,
}

/// Flat symbol table for a single function.
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    pub symbols: HashMap<String, Symbol>,
}

impl SymbolTable {
    pub fn get(&self, name: &str) -> Option<&Symbol> {
        self.symbols.get(name)
    }

    pub fn is_pointer(&self, name: &str) -> bool {
        self.symbols.get(name).map_or(false, |s| s.is_pointer)
    }

    pub fn is_array(&self, name: &str) -> bool {
        self.symbols.get(name).map_or(false, |s| s.is_array)
    }

    pub fn c_type(&self, name: &str) -> Option<&CType> {
        self.symbols.get(name).map(|s| &s.c_type)
    }
}

// ─────────────────────────────────────────────────────────────────────────────

pub struct SymbolTableBuilder;

impl SymbolTableBuilder {
    pub fn build(func: &CFunction) -> SymbolTable {
        let mut table = SymbolTable::default();

        // Add parameters
        for param in &func.parameters {
            let is_pointer = matches!(param.param_type, CType::Pointer(_));
            let is_array = matches!(param.param_type, CType::Array(_, _));
            table.symbols.insert(
                param.name.clone(),
                Symbol {
                    name: param.name.clone(),
                    c_type: param.param_type.clone(),
                    scope_depth: 0,
                    is_parameter: true,
                    is_pointer,
                    is_array,
                    declaration_line: 0,
                },
            );
        }

        // Walk body and collect declarations
        Self::collect_from_stmts(&func.body, 1, &mut table);
        table
    }

    fn collect_from_stmts(stmts: &[CStatement], depth: usize, table: &mut SymbolTable) {
        for stmt in stmts {
            match stmt {
                CStatement::VarDecl { name, var_type, .. } => {
                    let is_pointer = matches!(var_type, CType::Pointer(_));
                    let is_array = matches!(var_type, CType::Array(_, _));
                    table.symbols.insert(
                        name.clone(),
                        Symbol {
                            name: name.clone(),
                            c_type: var_type.clone(),
                            scope_depth: depth,
                            is_parameter: false,
                            is_pointer,
                            is_array,
                            declaration_line: 0,
                        },
                    );
                }
                CStatement::If { then_body, else_body, .. } => {
                    Self::collect_from_stmts(then_body, depth + 1, table);
                    if let Some(eb) = else_body {
                        Self::collect_from_stmts(eb, depth + 1, table);
                    }
                }
                CStatement::While { body, .. } => {
                    Self::collect_from_stmts(body, depth + 1, table);
                }
                CStatement::For { init, body, .. } => {
                    Self::collect_from_stmts(std::slice::from_ref(init), depth + 1, table);
                    Self::collect_from_stmts(body, depth + 1, table);
                }
                CStatement::StructDecl { name, struct_type, .. } => {
                    table.symbols.insert(
                        name.clone(),
                        Symbol {
                            name: name.clone(),
                            c_type: ast_input::CType::Struct(struct_type.clone()),
                            scope_depth: depth,
                            is_parameter: false,
                            is_pointer: false,
                            is_array: false,
                            declaration_line: 0,
                        },
                    );
                }
                _ => {}
            }
        }
    }
}
