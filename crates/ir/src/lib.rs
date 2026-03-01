//! # Intermediate Representation (IR)
//!
//! A typed, language-neutral IR that sits between the C AST + semantic model
//! and the Rust code generator.  This layer ensures:
//!  - Clear separation between analysis and code generation
//!  - Safe transformation decisions driven by semantic findings
//!  - Improved maintainability and extensibility
//!
//! Each `IrFunction` mirrors a C function but uses Rust-typed nodes,
//! ownership annotations, and resolved lifetimes.

use ast_input::{BinOp, CType, UnaryOp};
use semantic::{OwnershipKind, RustType, SemanticModel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// IR types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IrProgram {
    pub functions: Vec<IrFunction>,
    /// Set of `use` declarations needed at the top of the output file.
    pub required_uses: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct IrFunction {
    pub name: String,
    pub rust_return_type: String,
    pub parameters: Vec<IrParameter>,
    pub body: Vec<IrStatement>,
    pub requires_unsafe: bool,
}

#[derive(Debug, Clone)]
pub struct IrParameter {
    pub name: String,
    pub rust_type: String,
    pub is_mutable: bool,
    pub ownership: OwnershipKind,
}

#[derive(Debug, Clone)]
pub enum IrStatement {
    /// `let [mut] name: rust_type = value;`
    LetBinding {
        name: String,
        rust_type: String,
        init: Option<IrExpr>,
        is_mutable: bool,
    },
    /// `name = value;`
    Assign {
        target: String,
        value: IrExpr,
    },
    /// `array[index] = value;`
    IndexAssign {
        array: String,
        index: IrExpr,
        value: IrExpr,
    },
    /// `return expr;` or `return;`
    Return(Option<IrExpr>),
    /// `if cond { then } [else { else }]`
    If {
        condition: IrExpr,
        then_block: Vec<IrStatement>,
        else_block: Option<Vec<IrStatement>>,
    },
    /// `while cond { body }`
    While {
        condition: IrExpr,
        body: Vec<IrStatement>,
    },
    /// Desugared for loop → while
    While2 {
        init: Box<IrStatement>,
        condition: IrExpr,
        increment: IrExpr,
        body: Vec<IrStatement>,
    },
    /// `break;`
    Break,
    /// `continue;`
    Continue,
    /// Standalone expression statement
    Expr(IrExpr),
    /// Block of unsafe code
    UnsafeBlock(Vec<IrStatement>),
}

#[derive(Debug, Clone)]
pub enum IrExpr {
    IntLit(i64),
    FloatLit(f64),
    BoolLit(bool),
    StringLit(String),
    CharLit(char),
    Null,
    Var(String),
    BinOp { left: Box<IrExpr>, op: BinOp, right: Box<IrExpr> },
    UnaryOp { op: UnaryOp, operand: Box<IrExpr> },
    Call { function: String, args: Vec<IrExpr> },
    ArrayAccess { array: Box<IrExpr>, index: Box<IrExpr> },
    ArrayLiteral(Vec<IrExpr>),
    Cast { to_type: String, expr: Box<IrExpr> },
    /// `expr as usize` for array indexing
    AsUsize(Box<IrExpr>),
    /// `Box::new(expr)`
    BoxNew(Box<IrExpr>),
    /// `Rc::new(expr)`
    RcNew(Box<IrExpr>),
    /// `RefCell::new(expr)`
    RefCellNew(Box<IrExpr>),
    /// `std::ptr::null_mut()`
    NullMut,
    /// `None`
    OptionNone,
    /// `Some(expr)`
    OptionSome(Box<IrExpr>),
}

// ─────────────────────────────────────────────────────────────────────────────
// IR builder – lowers C AST + semantic model into IR
// ─────────────────────────────────────────────────────────────────────────────

pub struct IrBuilder;

impl IrBuilder {
    pub fn build(
        program: &ast_input::CProgram,
        semantic: &SemanticModel,
    ) -> anyhow::Result<IrProgram> {
        let mut functions = Vec::new();
        let mut required_uses: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (func, sem) in program.functions.iter().zip(semantic.functions.iter()) {
            let ir_func = Self::build_function(func, sem, &mut required_uses)?;
            functions.push(ir_func);
        }

        Ok(IrProgram {
            functions,
            required_uses: required_uses.into_iter().collect(),
        })
    }

    fn build_function(
        func: &ast_input::CFunction,
        sem: &semantic::FunctionSemantics,
        uses: &mut std::collections::HashSet<String>,
    ) -> anyhow::Result<IrFunction> {
        // Parameters
        let parameters: Vec<IrParameter> = func
            .parameters
            .iter()
            .map(|p| {
                let rt = sem.rust_type_map.get(&p.name)
                    .map(|r| r.rust_repr.clone())
                    .unwrap_or_else(|| Self::fallback_type(&p.param_type));
                let ownership = sem.ownership_map.get(&p.name)
                    .cloned()
                    .unwrap_or(OwnershipKind::Value);
                let is_mutable = sem.mutable_vars.contains(&p.name);
                IrParameter { name: p.name.clone(), rust_type: rt, is_mutable, ownership }
            })
            .collect();

        // Determine if any unsafe needed
        let requires_unsafe = sem.rust_type_map.values().any(|rt| rt.requires_unsafe);

        if requires_unsafe {
            uses.insert("std::ptr".to_string());
        }

        // Check if Rc needed
        let needs_rc = sem.ownership_map.values().any(|o| *o == OwnershipKind::SharedOwned);
        if needs_rc {
            uses.insert("std::rc::Rc".to_string());
        }

        // Check if RefCell needed
        let needs_refcell = sem.ownership_map.values().any(|o| *o == OwnershipKind::InteriorMutable);
        if needs_refcell {
            uses.insert("std::cell::RefCell".to_string());
        }

        let rust_return_type = Self::fallback_type(&func.return_type);

        // Build body
        let body = Self::build_stmts(&func.body, sem)?;

        Ok(IrFunction {
            name: func.name.clone(),
            rust_return_type,
            parameters,
            body,
            requires_unsafe,
        })
    }

    fn build_stmts(
        stmts: &[ast_input::CStatement],
        sem: &semantic::FunctionSemantics,
    ) -> anyhow::Result<Vec<IrStatement>> {
        let mut result = Vec::new();
        for stmt in stmts {
            result.push(Self::build_stmt(stmt, sem)?);
        }
        Ok(result)
    }

    fn build_stmt(
        stmt: &ast_input::CStatement,
        sem: &semantic::FunctionSemantics,
    ) -> anyhow::Result<IrStatement> {
        use ast_input::CStatement as CS;
        Ok(match stmt {
            CS::VarDecl { name, var_type, init } => {
                let rust_type = sem.rust_type_map.get(name)
                    .map(|r| r.rust_repr.clone())
                    .unwrap_or_else(|| Self::fallback_type(var_type));
                let is_mutable = sem.mutable_vars.contains(name);
                let init_ir = if let Some(e) = init {
                    Some(Self::build_expr(e, sem)?)
                } else {
                    None
                };
                IrStatement::LetBinding { name: name.clone(), rust_type, init: init_ir, is_mutable }
            }
            CS::Assignment { target, value } => {
                let value_ir = Self::build_expr(value, sem)?;
                IrStatement::Assign { target: target.clone(), value: value_ir }
            }
            CS::IndexedAssignment { array, index, value } => {
                let index_ir = Self::build_index_expr(index, sem)?;
                let value_ir = Self::build_expr(value, sem)?;
                IrStatement::IndexAssign { array: array.clone(), index: index_ir, value: value_ir }
            }
            CS::Return(Some(e)) => IrStatement::Return(Some(Self::build_expr(e, sem)?)),
            CS::Return(None) => IrStatement::Return(None),
            CS::Break => IrStatement::Break,
            CS::Continue => IrStatement::Continue,
            CS::Expression(e) => IrStatement::Expr(Self::build_expr(e, sem)?),
            CS::If { condition, then_body, else_body } => {
                let cond = Self::build_expr(condition, sem)?;
                let then_b = Self::build_stmts(then_body, sem)?;
                let else_b = if let Some(eb) = else_body {
                    Some(Self::build_stmts(eb, sem)?)
                } else {
                    None
                };
                IrStatement::If { condition: cond, then_block: then_b, else_block: else_b }
            }
            CS::While { condition, body } => {
                let cond = Self::build_expr(condition, sem)?;
                let body_ir = Self::build_stmts(body, sem)?;
                IrStatement::While { condition: cond, body: body_ir }
            }
            CS::For { init, condition, increment, body } => {
                let init_ir = Box::new(Self::build_stmt(init, sem)?);
                let cond = Self::build_expr(condition, sem)?;
                let incr = Self::build_expr(increment, sem)?;
                let body_ir = Self::build_stmts(body, sem)?;
                IrStatement::While2 { init: init_ir, condition: cond, increment: incr, body: body_ir }
            }
        })
    }

    fn build_expr(
        expr: &ast_input::CExpression,
        sem: &semantic::FunctionSemantics,
    ) -> anyhow::Result<IrExpr> {
        use ast_input::CExpression as CE;
        use ast_input::CLiteral;
        Ok(match expr {
            CE::Literal(CLiteral::Int(n))    => IrExpr::IntLit(*n),
            CE::Literal(CLiteral::Float(f))  => IrExpr::FloatLit(*f),
            CE::Literal(CLiteral::String(s)) => IrExpr::StringLit(s.clone()),
            CE::Literal(CLiteral::Char(c))   => IrExpr::CharLit(*c),
            CE::Null                          => IrExpr::NullMut,
            CE::Variable(name)               => IrExpr::Var(name.clone()),
            CE::BinaryOp { left, op, right } => IrExpr::BinOp {
                left: Box::new(Self::build_expr(left, sem)?),
                op: *op,
                right: Box::new(Self::build_expr(right, sem)?),
            },
            CE::UnaryOp { op, operand } => IrExpr::UnaryOp {
                op: *op,
                operand: Box::new(Self::build_expr(operand, sem)?),
            },
            CE::Call { function, args } => IrExpr::Call {
                function: function.clone(),
                args: args.iter().map(|a| Self::build_expr(a, sem)).collect::<anyhow::Result<_>>()?,
            },
            CE::ArrayAccess { array, index } => IrExpr::ArrayAccess {
                array: Box::new(Self::build_expr(array, sem)?),
                index: Box::new(Self::build_index_expr(index, sem)?),
            },
            CE::ArrayLiteral(elems) => IrExpr::ArrayLiteral(
                elems.iter().map(|e| Self::build_expr(e, sem)).collect::<anyhow::Result<_>>()?
            ),
            CE::Cast { to_type, expr: inner } => {
                let rust_type = Self::fallback_type(to_type);
                IrExpr::Cast { to_type: rust_type, expr: Box::new(Self::build_expr(inner, sem)?) }
            }
        })
    }

    fn build_index_expr(
        expr: &ast_input::CExpression,
        sem: &semantic::FunctionSemantics,
    ) -> anyhow::Result<IrExpr> {
        use ast_input::CExpression as CE;
        use ast_input::CLiteral;
        // Plain integer literal → no cast needed
        if let CE::Literal(CLiteral::Int(_)) = expr {
            return Self::build_expr(expr, sem);
        }
        Ok(IrExpr::AsUsize(Box::new(Self::build_expr(expr, sem)?)))
    }

    fn fallback_type(ctype: &CType) -> String {
        match ctype {
            CType::Void => "()".to_string(),
            CType::Int => "i32".to_string(),
            CType::Char => "u8".to_string(),
            CType::Float => "f32".to_string(),
            CType::Double => "f64".to_string(),
            CType::Pointer(inner) => format!("*mut {}", Self::fallback_type(inner)),
            CType::Array(inner, Some(n)) => format!("[{}; {}]", Self::fallback_type(inner), n),
            CType::Array(inner, None) => format!("Vec<{}>", Self::fallback_type(inner)),
            CType::Unknown(s) => s.clone(),
        }
    }
}
