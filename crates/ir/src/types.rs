// ir/src/types.rs
//
// Intermediate Representation type definitions.

use ast_input::{BinOp, UnaryOp};
use semantic::OwnershipKind;

// ─────────────────────────────────────────────────────────────────────────────
// IR types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IrStructField { pub name: String, pub rust_type: String }

#[derive(Debug, Clone)]
pub struct IrStructDef { pub name: String, pub fields: Vec<IrStructField> }

#[derive(Debug, Clone)]
pub struct IrGlobalVar {
    pub name: String,
    pub rust_type: String,
    pub init: Option<IrExpr>,
    pub is_const: bool,
}

#[derive(Debug, Clone)]
pub struct IrProgram {
    pub functions: Vec<IrFunction>,
    pub structs: Vec<IrStructDef>,
    pub global_vars: Vec<IrGlobalVar>,
    pub required_uses: Vec<String>,
    pub is_executable: bool,
}

#[derive(Debug, Clone)]
pub struct IrFunction {
    pub name: String,
    pub rust_return_type: String,
    pub parameters: Vec<IrParameter>,
    pub body: Vec<IrStatement>,
    pub requires_unsafe: bool,
    pub is_main: bool,
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
    LetBinding  { name: String, rust_type: String, init: Option<IrExpr>, is_mutable: bool },
    Assign      { target: String, value: IrExpr },
    IndexAssign { array: String, index: IrExpr, value: IrExpr },
    FieldAssign { object: String, field: String, value: IrExpr },
    Return(Option<IrExpr>),
    If      { condition: IrExpr, then_block: Vec<IrStatement>, else_block: Option<Vec<IrStatement>> },
    While   { condition: IrExpr, body: Vec<IrStatement> },
    While2  { init: Box<IrStatement>, condition: IrExpr, increment: IrExpr, body: Vec<IrStatement> },
    /// Rust `match` (from C switch/case).
    Match   { expr: IrExpr, arms: Vec<(Vec<IrExpr>, Vec<IrStatement>)>, default: Option<Vec<IrStatement>> },
    /// C `do { ... } while(cond);` → `loop { body; if !cond { break; } }`
    Loop    { body: Vec<IrStatement>, condition: IrExpr },
    Break, Continue,
    Expr(IrExpr),
    UnsafeBlock(Vec<IrStatement>),
    Drop(String),
    /// Swap two array elements in-place: arr.swap(i, j)
    SwapElements { array: String, i: IrExpr, j: IrExpr },
    /// Read one or more values from stdin (from C scanf)
    Scanf { vars: Vec<String> },
}

#[derive(Debug, Clone)]
pub enum IrExpr {
    IntLit(i64), FloatLit(f64), BoolLit(bool), StringLit(String), CharLit(char),
    Null, Var(String),
    BinOp    { left: Box<IrExpr>, op: BinOp, right: Box<IrExpr> },
    UnaryOp  { op: UnaryOp, operand: Box<IrExpr> },
    Call     { function: String, args: Vec<IrExpr> },
    ArrayAccess  { array: Box<IrExpr>, index: Box<IrExpr> },
    ArrayLiteral(Vec<IrExpr>),
    Cast     { to_type: String, expr: Box<IrExpr> },
    AsUsize(Box<IrExpr>),
    BoxNew(Box<IrExpr>), RcNew(Box<IrExpr>), RefCellNew(Box<IrExpr>),
    NullMut, OptionNone, OptionSome(Box<IrExpr>),
    FieldAccess  { object: Box<IrExpr>, field: String },
    StructLiteral { struct_type: String, fields: Vec<(String, IrExpr)> },
    StructDefault(String),
    VecNew { elem_type: String, len: Box<IrExpr> },
    Deref(Box<IrExpr>),
    Ref(Box<IrExpr>),       // &expr
    RefMut(Box<IrExpr>),    // &mut expr
    Clone(Box<IrExpr>),     // expr.clone()
    PrintLn { fmt: String, args: Vec<IrExpr> },
    Print   { fmt: String, args: Vec<IrExpr> },
    EPrintLn { fmt: String, args: Vec<IrExpr> },
    ReadLine { var: String },
    Exit(Box<IrExpr>),
}

/// Simple structural equality for IR expressions (used in swap detection).
pub fn expr_eq(a: &IrExpr, b: &IrExpr) -> bool {
    match (a, b) {
        (IrExpr::IntLit(x), IrExpr::IntLit(y)) => x == y,
        (IrExpr::Var(x), IrExpr::Var(y)) => x == y,
        (IrExpr::AsUsize(x), IrExpr::AsUsize(y)) => expr_eq(x, y),
        (IrExpr::BinOp { left: l1, op: o1, right: r1 }, IrExpr::BinOp { left: l2, op: o2, right: r2 }) => {
            format!("{:?}", o1) == format!("{:?}", o2) && expr_eq(l1, l2) && expr_eq(r1, r2)
        }
        _ => false,
    }
}
