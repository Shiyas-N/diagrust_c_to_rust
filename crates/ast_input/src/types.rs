// ast_input/src/types.rs
//
// Core AST type definitions for the C representation.

use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
// Validation error types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(String),
    #[error("Syntax error at line {line}: {message}")]
    SyntaxError { line: usize, message: String },
    #[error("Empty or invalid C source")]
    EmptySource,
}

// ─────────────────────────────────────────────────────────────────────────────
// AST types
// ─────────────────────────────────────────────────────────────────────────────

/// A top-level (global) variable declaration.
#[derive(Debug, Clone)]
pub struct CGlobalVar {
    pub name: String,
    pub var_type: CType,
    pub init: Option<CExpression>,
    pub is_const: bool,
}

#[derive(Debug, Clone)]
pub struct CProgram {
    pub functions: Vec<CFunction>,
    /// Top-level struct type definitions.
    pub structs: Vec<CStructDef>,
    /// Top-level variable declarations (globals / consts).
    pub global_vars: Vec<CGlobalVar>,
    /// True if a `main` function was found — executable program.
    pub has_main: bool,
}

#[derive(Debug, Clone)]
pub struct CFunction {
    pub name: String,
    pub return_type: CType,
    pub parameters: Vec<CParameter>,
    pub body: Vec<CStatement>,
}

#[derive(Debug, Clone)]
pub struct CParameter {
    pub name: String,
    pub param_type: CType,
}

/// A top-level struct definition.
#[derive(Debug, Clone)]
pub struct CStructDef {
    pub name: String,
    pub fields: Vec<CStructField>,
}

#[derive(Debug, Clone)]
pub struct CStructField {
    pub name: String,
    pub field_type: CType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CType {
    Void,
    Int,
    Char,
    Float,
    Double,
    Long,
    UInt,
    UChar,
    ULong,
    Short,
    Bool,
    Pointer(Box<CType>),
    Array(Box<CType>, Option<usize>),
    /// A named struct type.
    Struct(String),
    Unknown(String),
}

#[derive(Debug, Clone)]
pub enum CStatement {
    Return(Option<CExpression>),
    VarDecl { name: String, var_type: CType, init: Option<CExpression> },
    Assignment { target: String, value: CExpression },
    IndexedAssignment { array: String, index: CExpression, value: CExpression },
    FieldAssignment { object: String, field: String, value: CExpression },
    Expression(CExpression),
    If {
        condition: CExpression,
        then_body: Vec<CStatement>,
        else_body: Option<Vec<CStatement>>,
    },
    While { condition: CExpression, body: Vec<CStatement> },
    For {
        init: Box<CStatement>,
        condition: CExpression,
        increment: CExpression,
        body: Vec<CStatement>,
    },
    /// C `switch(expr) { case ...: ... }` statement.
    Switch {
        expr: CExpression,
        cases: Vec<(Vec<CExpression>, Vec<CStatement>)>,
        default: Option<Vec<CStatement>>,
    },
    /// C `do { ... } while(cond);` loop.
    DoWhile {
        body: Vec<CStatement>,
        condition: CExpression,
    },
    Break,
    Continue,
    /// A local struct variable declaration / initialisation.
    StructDecl {
        name: String,
        struct_type: String,
        fields: Vec<(String, CExpression)>,
    },
}

#[derive(Debug, Clone)]
pub enum CExpression {
    Literal(CLiteral),
    Variable(String),
    BinaryOp { left: Box<CExpression>, op: BinOp, right: Box<CExpression> },
    UnaryOp { op: UnaryOp, operand: Box<CExpression> },
    Call { function: String, args: Vec<CExpression> },
    ArrayAccess { array: Box<CExpression>, index: Box<CExpression> },
    ArrayLiteral(Vec<CExpression>),
    Cast { to_type: CType, expr: Box<CExpression> },
    /// `obj.field` or `ptr->field` (both normalised to this).
    FieldAccess { object: Box<CExpression>, field: String },
    /// Struct / array initialiser literal.
    StructLiteral { struct_type: String, fields: Vec<(String, CExpression)> },
    Null,
}

#[derive(Debug, Clone)]
pub enum CLiteral {
    Int(i64),
    Float(f64),
    String(String),
    Char(char),
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Equal, NotEqual, Less, LessEq, Greater, GreaterEq,
    And, Or,
    Assign,
    BitAnd, BitOr, BitXor, Shl, Shr,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg, Not, Deref, AddrOf, BitNot,
}
