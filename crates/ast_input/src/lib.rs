// ast_input/src/lib.rs
//
#![allow(non_upper_case_globals)]
//
// Clang-AST based C parser.
// Replaces the old regex / hand-rolled parser.  Uses clang-sys (libclang
// bindings) to obtain a proper AST and walks it into our internal CProgram
// representation.  Struct support is included.

use anyhow::{Context, Result};
use std::path::Path;
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────
// Validation error types (unchanged from original)
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

// ─────────────────────────────────────────────────────────────────────────────
// Input validator (lightweight structural checks only)
// ─────────────────────────────────────────────────────────────────────────────

pub struct InputValidator {
    strict_mode: bool,
}

impl InputValidator {
    pub fn new(strict_mode: bool) -> Self {
        Self { strict_mode }
    }

    pub fn validate(&self, source: &str) -> Result<(), ValidationError> {
        if source.trim().is_empty() {
            return Err(ValidationError::EmptySource);
        }
        self.check_unsupported_features(source)?;
        self.check_basic_syntax(source)?;
        Ok(())
    }

    fn check_unsupported_features(&self, source: &str) -> Result<(), ValidationError> {
        let unsupported = [
            ("goto",  "goto statements"),
            ("asm",   "inline assembly"),
            ("__asm", "inline assembly"),
            ("union", "unions"),
        ];
        for (keyword, feature) in unsupported {
            if source.contains(keyword) && self.strict_mode {
                return Err(ValidationError::UnsupportedFeature(feature.to_string()));
            }
        }
        Ok(())
    }

    fn check_basic_syntax(&self, source: &str) -> Result<(), ValidationError> {
        let open  = source.matches('{').count();
        let close = source.matches('}').count();
        if open != close {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: format!("Mismatched braces: {} open, {} close", open, close),
            });
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ClangParser – Clang-AST based parser
// ─────────────────────────────────────────────────────────────────────────────

pub struct ClangParser;

impl ClangParser {
    pub fn parse(source: &str) -> Result<CProgram> {
        use clang_sys::*;
        use std::ffi::CString;
        use std::os::raw::c_int;

        // Dynamically load libclang if it has not been loaded yet on this thread.
        // clang_sys::load() is safe (no `unsafe` needed) and idempotent.
        clang_sys::load().map_err(|e| anyhow::anyhow!(
            "Failed to load libclang: {}. \n\
             On Windows: install LLVM from https://releases.llvm.org/ and ensure \
             libclang.dll is on PATH or set LIBCLANG_PATH. \n\
             On Linux/macOS: install llvm-dev / llvm package.", e
        ))?;

        let tmp_path = std::env::temp_dir().join("diagrust_input.c");
        std::fs::write(&tmp_path, source)
            .context("Failed to write temporary C source file")?;
        let path_cstr = CString::new(tmp_path.to_str().unwrap()).unwrap();

        unsafe {
            let index = clang_createIndex(0, 0);
            anyhow::ensure!(!index.is_null(), "clang_createIndex returned null");

            let args: Vec<CString> = vec![
                CString::new("-std=c99").unwrap(),
                CString::new("-w").unwrap(),
            ];
            let arg_ptrs: Vec<*const i8> = args.iter().map(|s| s.as_ptr()).collect();

            let tu = clang_parseTranslationUnit(
                index,
                path_cstr.as_ptr(),
                arg_ptrs.as_ptr(),
                arg_ptrs.len() as c_int,
                std::ptr::null_mut(),
                0,
                CXTranslationUnit_None,
            );

            if tu.is_null() {
                clang_disposeIndex(index);
                anyhow::bail!("libclang failed to parse the translation unit");
            }

            let mut program = CProgram { functions: Vec::new(), structs: Vec::new(), global_vars: Vec::new(), has_main: false };
            let root = clang_getTranslationUnitCursor(tu);

            extern "C" fn top_visitor(
                cursor: CXCursor,
                _parent: CXCursor,
                data: CXClientData,
            ) -> CXChildVisitResult {
                let prog = unsafe { &mut *(data as *mut CProgram) };
                unsafe { visit_top_level(cursor, prog) };
                CXChildVisit_Continue
            }

            clang_visitChildren(root, top_visitor, &mut program as *mut _ as CXClientData);
            clang_disposeTranslationUnit(tu);
            clang_disposeIndex(index);
            program.has_main = program.functions.iter().any(|f| f.name == "main");
            Ok(program)
        }
    }
}

/// Drop-in alias so `SimpleParser::parse(source)` still compiles.
pub type SimpleParser = ClangParser;

// ─────────────────────────────────────────────────────────────────────────────
// Cursor walking
// ─────────────────────────────────────────────────────────────────────────────

unsafe fn visit_top_level(cursor: clang_sys::CXCursor, prog: &mut CProgram) {
    use clang_sys::*;

    // Skip anything that comes from an #include'd file (stdio.h, stdlib.h, etc.)
    // Only process declarations physically located in the user's own source file.
    let loc = clang_getCursorLocation(cursor);
    if clang_Location_isFromMainFile(loc) == 0 {
        return;
    }

    match clang_getCursorKind(cursor) {
        CXCursor_StructDecl => {
            if let Some(sd) = extract_struct_def(cursor) {
                prog.structs.push(sd);
            }
        }
        CXCursor_FunctionDecl => {
            if clang_isCursorDefinition(cursor) != 0 {
                if let Some(f) = extract_function(cursor) {
                    prog.functions.push(f);
                }
            }
        }
        CXCursor_VarDecl => {
            // Top-level global variable
            let name      = cursor_spelling(cursor);
            let var_type  = cx_type_to_ctype(clang_getCursorType(cursor));
            let type_str  = type_spelling(clang_getCursorType(cursor));
            let is_const  = type_str.contains("const");
            let children  = collect_children(cursor);
            let init = children.iter().find_map(|&c| extract_expr(c));
            prog.global_vars.push(CGlobalVar { name, var_type, init, is_const });
        }
        _ => {}
    }
}

// ── String helpers ────────────────────────────────────────────────────────────

unsafe fn cx_string(s: clang_sys::CXString) -> String {
    use clang_sys::*;
    let ptr = clang_getCString(s);
    let r = if ptr.is_null() {
        String::new()
    } else {
        std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
    };
    clang_disposeString(s);
    r
}

unsafe fn cursor_spelling(c: clang_sys::CXCursor) -> String {
    cx_string(clang_sys::clang_getCursorSpelling(c))
}

unsafe fn type_spelling(t: clang_sys::CXType) -> String {
    cx_string(clang_sys::clang_getTypeSpelling(t))
}

// ── Type conversion ────────────────────────────────────────────────────────────

unsafe fn cx_type_to_ctype(ty: clang_sys::CXType) -> CType {
    use clang_sys::*;
    match ty.kind {
        CXType_Void                                  => CType::Void,
        CXType_Bool                                  => CType::Bool,
        CXType_Char_S | CXType_Char_U | CXType_SChar => CType::Char,
        CXType_UChar                                 => CType::UChar,
        CXType_Short                                 => CType::Short,
        CXType_Int                                   => CType::Int,
        CXType_UInt                                  => CType::UInt,
        CXType_Long | CXType_LongLong                => CType::Long,
        CXType_ULong                                 => CType::ULong,
        CXType_Float                                 => CType::Float,
        CXType_Double | CXType_LongDouble            => CType::Double,
        CXType_Pointer => {
            let inner = clang_getPointeeType(ty);
            CType::Pointer(Box::new(cx_type_to_ctype(inner)))
        }
        CXType_ConstantArray | CXType_IncompleteArray | CXType_VariableArray => {
            let elem = clang_getArrayElementType(ty);
            let size = clang_getArraySize(ty);
            let elem_type = cx_type_to_ctype(elem);
            if size >= 0 {
                CType::Array(Box::new(elem_type), Some(size as usize))
            } else {
                CType::Array(Box::new(elem_type), None)
            }
        }
        CXType_Record => {
            let raw = type_spelling(ty);
            let clean = raw
                .trim_start_matches("struct ")
                .trim_start_matches("union ")
                .trim()
                .to_string();
            CType::Struct(clean)
        }
        CXType_Elaborated => {
            cx_type_to_ctype(clang_getCanonicalType(ty))
        }
        CXType_Typedef => {
            cx_type_to_ctype(clang_getCanonicalType(ty))
        }
        _ => CType::Unknown(type_spelling(ty)),
    }
}

// ── Struct def ─────────────────────────────────────────────────────────────────

unsafe fn extract_struct_def(cursor: clang_sys::CXCursor) -> Option<CStructDef> {
    use clang_sys::*;
    let name = cursor_spelling(cursor);
    if name.is_empty() { return None; }

    let mut fields: Vec<CStructField> = Vec::new();

    extern "C" fn field_vis(
        cursor: CXCursor,
        _parent: CXCursor,
        data: CXClientData,
    ) -> CXChildVisitResult {
        let fields = unsafe { &mut *(data as *mut Vec<CStructField>) };
        unsafe {
            if clang_getCursorKind(cursor) == CXCursor_FieldDecl {
                let fname = cursor_spelling(cursor);
                let ftype = cx_type_to_ctype(clang_getCursorType(cursor));
                fields.push(CStructField { name: fname, field_type: ftype });
            }
        }
        CXChildVisit_Continue
    }

    clang_visitChildren(cursor, field_vis, &mut fields as *mut _ as CXClientData);
    Some(CStructDef { name, fields })
}

// ── Function extraction ────────────────────────────────────────────────────────

unsafe fn extract_function(cursor: clang_sys::CXCursor) -> Option<CFunction> {
    use clang_sys::*;
    let name        = cursor_spelling(cursor);
    let func_type   = clang_getCursorType(cursor);
    let return_type = cx_type_to_ctype(clang_getResultType(func_type));

    let n_args = clang_Cursor_getNumArguments(cursor);
    let parameters: Vec<CParameter> = (0..n_args)
        .map(|i| {
            let arg = clang_Cursor_getArgument(cursor, i as u32);
            CParameter {
                name: cursor_spelling(arg),
                param_type: cx_type_to_ctype(clang_getCursorType(arg)),
            }
        })
        .collect();

    struct BodyCtx { stmts: Option<Vec<CStatement>> }

    extern "C" fn body_vis(
        cursor: CXCursor,
        _parent: CXCursor,
        data: CXClientData,
    ) -> CXChildVisitResult {
        use clang_sys::*;
        let ctx = unsafe { &mut *(data as *mut BodyCtx) };
        if unsafe { clang_getCursorKind(cursor) } == CXCursor_CompoundStmt {
            ctx.stmts = Some(unsafe { extract_compound_stmt(cursor) });
            return CXChildVisit_Break;
        }
        CXChildVisit_Continue
    }

    let mut ctx = BodyCtx { stmts: None };
    clang_visitChildren(cursor, body_vis, &mut ctx as *mut _ as CXClientData);
    let body = ctx.stmts.unwrap_or_default();

    Some(CFunction { name, return_type, parameters, body })
}

// ── Compound statement ─────────────────────────────────────────────────────────

unsafe fn extract_compound_stmt(cursor: clang_sys::CXCursor) -> Vec<CStatement> {
    use clang_sys::*;

    struct Ctx { stmts: Vec<CStatement> }

    extern "C" fn vis(
        cursor: CXCursor,
        _parent: CXCursor,
        data: CXClientData,
    ) -> CXChildVisitResult {
        let ctx = unsafe { &mut *(data as *mut Ctx) };
        if let Some(s) = unsafe { extract_stmt(cursor) } {
            ctx.stmts.push(s);
        }
        CXChildVisit_Continue
    }

    let mut ctx = Ctx { stmts: Vec::new() };
    clang_visitChildren(cursor, vis, &mut ctx as *mut _ as CXClientData);
    ctx.stmts
}

// ── Statement extraction ───────────────────────────────────────────────────────

unsafe fn extract_stmt(cursor: clang_sys::CXCursor) -> Option<CStatement> {
    use clang_sys::*;
    let kind = clang_getCursorKind(cursor);

    match kind {
        CXCursor_CompoundStmt => None, // flatten handled by compound walker

        CXCursor_DeclStmt => {
            let mut decls: Vec<CStatement> = Vec::new();
            extern "C" fn dv(cursor: CXCursor, _p: CXCursor, data: CXClientData) -> CXChildVisitResult {
                let decls = unsafe { &mut *(data as *mut Vec<CStatement>) };
                if unsafe { clang_getCursorKind(cursor) } == clang_sys::CXCursor_VarDecl {
                    if let Some(s) = unsafe { extract_var_decl(cursor) } {
                        decls.push(s);
                    }
                }
                CXChildVisit_Continue
            }
            clang_visitChildren(cursor, dv, &mut decls as *mut _ as CXClientData);
            decls.into_iter().next()
        }

        CXCursor_ReturnStmt => {
            // Walk all children; the first expression child is the return value.
            // We collect children explicitly to handle ImplicitCastExpr wrappers.
            let ch = collect_children(cursor);
            let expr = ch.iter().find_map(|&c| extract_expr(c));
            Some(CStatement::Return(expr))
        }

        CXCursor_IfStmt => {
            let ch = collect_children(cursor);
            let cond  = ch.get(0).and_then(|c| extract_expr(*c)).unwrap_or(CExpression::Null);
            let then  = ch.get(1).map(|c| extract_stmt_as_block(*c)).unwrap_or_default();
            let else_ = ch.get(2).map(|c| extract_stmt_as_block(*c));
            Some(CStatement::If { condition: cond, then_body: then, else_body: else_ })
        }

        CXCursor_WhileStmt => {
            let ch   = collect_children(cursor);
            let cond = ch.get(0).and_then(|c| extract_expr(*c)).unwrap_or(CExpression::Null);
            let body = ch.get(1).map(|c| extract_stmt_as_block(*c)).unwrap_or_default();
            Some(CStatement::While { condition: cond, body })
        }

        CXCursor_ForStmt      => extract_for_stmt(cursor),
        CXCursor_BreakStmt    => Some(CStatement::Break),
        CXCursor_ContinueStmt => Some(CStatement::Continue),

        _ => {
            // Only treat as expression-statement for cursor kinds that are
            // genuine expressions. Statement-level cursors that we don't handle
            // explicitly are silently skipped to avoid double-processing.
            let is_expr_kind = matches!(kind,
                CXCursor_BinaryOperator | CXCursor_CompoundAssignOperator |
                CXCursor_UnaryOperator  | CXCursor_CallExpr |
                CXCursor_ArraySubscriptExpr | CXCursor_MemberRefExpr |
                CXCursor_CStyleCastExpr | CXCursor_ParenExpr |
                CXCursor_UnexposedExpr
            );
            if is_expr_kind {
                extract_stmt_expr(cursor)
            } else {
                None
            }
        }
    }
}

unsafe fn extract_var_decl(cursor: clang_sys::CXCursor) -> Option<CStatement> {
    let name     = cursor_spelling(cursor);
    let var_type = cx_type_to_ctype(clang_sys::clang_getCursorType(cursor));

    if let CType::Struct(ref sn) = var_type {
        let sn = sn.clone();
        let children = collect_children(cursor);
        let mut fields: Vec<(String, CExpression)> = Vec::new();
        if let Some(&init_c) = children.last() {
            if unsafe { clang_sys::clang_getCursorKind(init_c) } == clang_sys::CXCursor_InitListExpr {
                let ic = collect_children(init_c);
                for (i, &child) in ic.iter().enumerate() {
                    if let Some(e) = unsafe { extract_expr(child) } {
                        fields.push((format!("_field{}", i), e));
                    }
                }
            } else if let Some(e) = unsafe { extract_expr(init_c) } {
                return Some(CStatement::VarDecl { name, var_type, init: Some(e) });
            }
        }
        return Some(CStatement::StructDecl { name, struct_type: sn, fields });
    }

    // For array types we must NOT use the size-literal child as an initialiser.
    // Only treat a child as an init if it is an InitListExpr or a non-array type.
    let init = unsafe {
        let children = collect_children(cursor);
        match &var_type {
            CType::Array(_, _) => {
                // Only accept an InitListExpr child as the initialiser.
                children.iter().find_map(|&c| {
                    if clang_sys::clang_getCursorKind(c) == clang_sys::CXCursor_InitListExpr {
                        extract_expr(c)
                    } else {
                        None
                    }
                })
            }
            _ => {
                // For scalars/pointers take the first meaningful child expression.
                children.iter().find_map(|&c| extract_expr(c))
            }
        }
    };
    Some(CStatement::VarDecl { name, var_type, init })
}

unsafe fn extract_stmt_as_block(cursor: clang_sys::CXCursor) -> Vec<CStatement> {
    if clang_sys::clang_getCursorKind(cursor) == clang_sys::CXCursor_CompoundStmt {
        extract_compound_stmt(cursor)
    } else {
        extract_stmt(cursor).map(|s| vec![s]).unwrap_or_default()
    }
}

unsafe fn extract_for_stmt(cursor: clang_sys::CXCursor) -> Option<CStatement> {
    let ch = collect_children(cursor);
    if ch.is_empty() { return None; }

    let body = extract_stmt_as_block(*ch.last()?);

    let (init, condition, increment) = match ch.len() {
        1 => (
            CStatement::Expression(CExpression::Null),
            CExpression::Literal(CLiteral::Int(1)),
            CExpression::Null,
        ),
        2 => (
            CStatement::Expression(CExpression::Null),
            extract_expr(ch[0]).unwrap_or(CExpression::Null),
            CExpression::Null,
        ),
        3 => (
            extract_stmt(ch[0]).unwrap_or(CStatement::Expression(CExpression::Null)),
            extract_expr(ch[1]).unwrap_or(CExpression::Null),
            CExpression::Null,
        ),
        _ => (
            extract_stmt(ch[0]).unwrap_or(CStatement::Expression(CExpression::Null)),
            extract_expr(ch[1]).unwrap_or(CExpression::Null),
            extract_expr(ch[2]).unwrap_or(CExpression::Null),
        ),
    };

    Some(CStatement::For { init: Box::new(init), condition, increment, body })
}

unsafe fn extract_stmt_expr(cursor: clang_sys::CXCursor) -> Option<CStatement> {
    let expr = extract_expr(cursor)?;
    if let CExpression::BinaryOp { left, op: BinOp::Assign, right } = expr {
        match *left {
            CExpression::Variable(name) =>
                return Some(CStatement::Assignment { target: name, value: *right }),
            CExpression::ArrayAccess { array, index } => {
                if let CExpression::Variable(arr) = *array {
                    return Some(CStatement::IndexedAssignment { array: arr, index: *index, value: *right });
                }
            }
            CExpression::FieldAccess { object, field } => {
                if let CExpression::Variable(obj) = *object {
                    return Some(CStatement::FieldAssignment { object: obj, field, value: *right });
                }
            }
            _ => {}
        }
    }
    Some(CStatement::Expression(extract_expr(cursor)?))
}

// ── Expression extraction ──────────────────────────────────────────────────────

unsafe fn extract_expr(cursor: clang_sys::CXCursor) -> Option<CExpression> {
    use clang_sys::*;
    let kind = clang_getCursorKind(cursor);

    match kind {
        CXCursor_IntegerLiteral => {
            Some(CExpression::Literal(CLiteral::Int(eval_integer(cursor))))
        }
        CXCursor_FloatingLiteral => {
            Some(CExpression::Literal(CLiteral::Float(eval_float(cursor))))
        }
        CXCursor_StringLiteral => {
            let s = cursor_spelling(cursor).trim_matches('"').to_string();
            Some(CExpression::Literal(CLiteral::String(s)))
        }
        CXCursor_CharacterLiteral => {
            let v = eval_integer(cursor);
            Some(CExpression::Literal(CLiteral::Char(char::from_u32(v as u32).unwrap_or('\0'))))
        }
        CXCursor_CXXNullPtrLiteralExpr => Some(CExpression::Null),
        CXCursor_DeclRefExpr => {
            let name = cursor_spelling(cursor);
            if name == "NULL" { return Some(CExpression::Null); }
            Some(CExpression::Variable(name))
        }
        CXCursor_ParenExpr => first_child_expr(cursor),
        CXCursor_UnaryOperator => extract_unary(cursor),
        CXCursor_BinaryOperator | CXCursor_CompoundAssignOperator => extract_binary(cursor),
        CXCursor_CStyleCastExpr => {
            let to_type = cx_type_to_ctype(clang_getCursorType(cursor));
            let inner   = first_child_expr(cursor)?;
            Some(CExpression::Cast { to_type, expr: Box::new(inner) })
        }
        CXCursor_CallExpr => extract_call(cursor),
        CXCursor_ArraySubscriptExpr => {
            let ch    = collect_children(cursor);
            let array = extract_expr(*ch.get(0)?)?;
            let index = extract_expr(*ch.get(1)?)?;
            Some(CExpression::ArrayAccess { array: Box::new(array), index: Box::new(index) })
        }
        CXCursor_MemberRefExpr => {
            let field  = cursor_spelling(cursor);
            let object = first_child_expr(cursor)?;
            Some(CExpression::FieldAccess { object: Box::new(object), field })
        }
        CXCursor_InitListExpr => {
            let raw = type_spelling(clang_getCursorType(cursor));
            let struct_type = raw.trim_start_matches("struct ").to_string();
            let ch = collect_children(cursor);
            let fields = ch.iter().enumerate()
                .filter_map(|(i, &c)| extract_expr(c).map(|e| (format!("_field{}", i), e)))
                .collect();
            Some(CExpression::StructLiteral { struct_type, fields })
        }
        // CXCursor_ImplicitValueInitExpr (617) is not exported by clang-sys 1.8,
        // so we match on its raw integer value directly.
        // CXCursor_UnexposedExpr (1) is the catch-all for unexposed nodes.
        other => {
            if other == 617 {
                // ImplicitValueInitExpr — zero-initialiser, e.g. in struct {0}
                Some(CExpression::Literal(CLiteral::Int(0)))
            } else if other == CXCursor_UnexposedExpr {
                first_child_expr(cursor)
            } else {
                None
            }
        }
    }
}

unsafe fn extract_binary(cursor: clang_sys::CXCursor) -> Option<CExpression> {
    let ch    = collect_children(cursor);
    let left  = extract_expr(*ch.get(0)?)?;
    let right = extract_expr(*ch.get(1)?)?;
    let op    = binary_op_from_tokens(cursor);
    Some(CExpression::BinaryOp { left: Box::new(left), op, right: Box::new(right) })
}

unsafe fn binary_op_from_tokens(cursor: clang_sys::CXCursor) -> BinOp {
    use clang_sys::*;
    let tu = clang_Cursor_getTranslationUnit(cursor);
    let range = clang_getCursorExtent(cursor);
    let mut toks: *mut CXToken = std::ptr::null_mut();
    let mut n: u32 = 0;
    clang_tokenize(tu, range, &mut toks, &mut n);
    if toks.is_null() || n == 0 { return BinOp::Add; }

    let tokens = std::slice::from_raw_parts(toks, n as usize);
    let mut op = BinOp::Add;
    let mut seen_left = false;
    let mut paren_depth   = 0i32;  // tracks ( )
    let mut bracket_depth = 0i32;  // tracks [ ]

    for tok in tokens {
        let k = clang_getTokenKind(*tok);
        let sp = cx_string(clang_getTokenSpelling(tu, *tok));
        match k {
            CXToken_Punctuation => {
                match sp.as_str() {
                    "(" => { paren_depth   += 1; continue; }
                    ")" => { paren_depth   -= 1; continue; }
                    "[" => { bracket_depth += 1; continue; }
                    "]" => { bracket_depth -= 1; continue; }
                    // "." is a member access — not a binary operator, skip it
                    // "->" is also member access (pointer), skip
                    "." | "->" => { continue; }
                    _ => {}
                }
                if seen_left && paren_depth == 0 && bracket_depth == 0 {
                    op = str_to_binop(&sp);
                    break;
                }
            }
            CXToken_Identifier | CXToken_Literal => {
                if paren_depth == 0 && bracket_depth == 0 {
                    seen_left = true;
                }
            }
            _ => {}
        }
    }

    clang_disposeTokens(tu, toks, n);
    op
}

fn str_to_binop(s: &str) -> BinOp {
    match s {
        "+"  => BinOp::Add,
        "-"  => BinOp::Sub,
        "*"  => BinOp::Mul,
        "/"  => BinOp::Div,
        "%"  => BinOp::Mod,
        "==" => BinOp::Equal,
        "!=" => BinOp::NotEqual,
        "<"  => BinOp::Less,
        "<=" => BinOp::LessEq,
        ">"  => BinOp::Greater,
        ">=" => BinOp::GreaterEq,
        "&&" => BinOp::And,
        "||" => BinOp::Or,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" => BinOp::Assign,
        "&"  => BinOp::BitAnd,
        "|"  => BinOp::BitOr,
        "^"  => BinOp::BitXor,
        "<<" => BinOp::Shl,
        ">>" => BinOp::Shr,
        _    => BinOp::Add,
    }
}

unsafe fn extract_unary(cursor: clang_sys::CXCursor) -> Option<CExpression> {
    use clang_sys::*;
    let tu = clang_Cursor_getTranslationUnit(cursor);
    let range = clang_getCursorExtent(cursor);
    let mut toks: *mut CXToken = std::ptr::null_mut();
    let mut n: u32 = 0;
    clang_tokenize(tu, range, &mut toks, &mut n);

    let mut op_str = String::new();
    if !toks.is_null() && n > 0 {
        let tokens = std::slice::from_raw_parts(toks, n as usize);
        if clang_getTokenKind(tokens[0]) == CXToken_Punctuation {
            op_str = cx_string(clang_getTokenSpelling(tu, tokens[0]));
        }
        clang_disposeTokens(tu, toks, n);
    }

    let operand = first_child_expr(cursor)?;

    match op_str.as_str() {
        "-"  => Some(CExpression::UnaryOp { op: UnaryOp::Neg,    operand: Box::new(operand) }),
        "!"  => Some(CExpression::UnaryOp { op: UnaryOp::Not,    operand: Box::new(operand) }),
        "*"  => Some(CExpression::UnaryOp { op: UnaryOp::Deref,  operand: Box::new(operand) }),
        "&"  => Some(CExpression::UnaryOp { op: UnaryOp::AddrOf, operand: Box::new(operand) }),
        "~"  => Some(CExpression::UnaryOp { op: UnaryOp::BitNot, operand: Box::new(operand) }),
        "++" | "--" => {
            let bin_op = if op_str == "++" { BinOp::Add } else { BinOp::Sub };
            Some(CExpression::BinaryOp {
                left: Box::new(operand.clone()),
                op: BinOp::Assign,
                right: Box::new(CExpression::BinaryOp {
                    left:  Box::new(operand),
                    op: bin_op,
                    right: Box::new(CExpression::Literal(CLiteral::Int(1))),
                }),
            })
        }
        _ => Some(CExpression::UnaryOp { op: UnaryOp::Neg, operand: Box::new(operand) }),
    }
}

unsafe fn extract_call(cursor: clang_sys::CXCursor) -> Option<CExpression> {
    use clang_sys::*;
    let ch = collect_children(cursor);
    if ch.is_empty() { return None; }

    let func_name = match clang_getCursorKind(ch[0]) {
        CXCursor_DeclRefExpr => cursor_spelling(ch[0]),
        _ => cursor_spelling(cursor),
    };

    let args: Vec<CExpression> = ch[1..].iter().filter_map(|&c| extract_expr(c)).collect();
    Some(CExpression::Call { function: func_name, args })
}

// ── Literal evaluation via token spelling ────────────────────────────────────
// clang-sys does not expose clang_Cursor_Evaluate / clang_EvalResult_* in its
// safe bindings, so we extract literal values by reading the source token text
// and parsing it with Rust's built-in parsers.

unsafe fn token_spelling_of_cursor(cursor: clang_sys::CXCursor) -> String {
    use clang_sys::*;
    let tu    = clang_Cursor_getTranslationUnit(cursor);
    let range = clang_getCursorExtent(cursor);
    let mut toks: *mut CXToken = std::ptr::null_mut();
    let mut n: u32 = 0;
    clang_tokenize(tu, range, &mut toks, &mut n);
    let result = if !toks.is_null() && n > 0 {
        let tokens = std::slice::from_raw_parts(toks, n as usize);
        let s = cx_string(clang_getTokenSpelling(tu, tokens[0]));
        clang_disposeTokens(tu, toks, n);
        s
    } else {
        String::new()
    };
    result
}

unsafe fn eval_integer(cursor: clang_sys::CXCursor) -> i64 {
    let raw = token_spelling_of_cursor(cursor);
    // Strip suffixes like u, U, l, L, ul, UL, ull, ULL
    let stripped = raw.trim_end_matches(|c: char| {
        matches!(c, 'u' | 'U' | 'l' | 'L')
    });
    // Hex: 0x... / 0X...
    if stripped.starts_with("0x") || stripped.starts_with("0X") {
        i64::from_str_radix(&stripped[2..], 16).unwrap_or(0)
    }
    // Octal: 0... (more than just "0")
    else if stripped.starts_with('0') && stripped.len() > 1 {
        i64::from_str_radix(&stripped[1..], 8).unwrap_or(0)
    }
    else {
        stripped.parse::<i64>().unwrap_or(0)
    }
}

unsafe fn eval_float(cursor: clang_sys::CXCursor) -> f64 {
    let raw = token_spelling_of_cursor(cursor);
    // Strip float suffixes: f, F, l, L
    let stripped = raw.trim_end_matches(|c: char| {
        matches!(c, 'f' | 'F' | 'l' | 'L')
    });
    stripped.parse::<f64>().unwrap_or(0.0)
}

// ── Cursor child helpers ───────────────────────────────────────────────────────

unsafe fn collect_children(cursor: clang_sys::CXCursor) -> Vec<clang_sys::CXCursor> {
    use clang_sys::*;

    struct Ctx { children: Vec<CXCursor> }

    extern "C" fn vis(cursor: CXCursor, _p: CXCursor, data: CXClientData) -> CXChildVisitResult {
        let ctx = unsafe { &mut *(data as *mut Ctx) };
        ctx.children.push(cursor);
        CXChildVisit_Continue
    }

    let mut ctx = Ctx { children: Vec::new() };
    clang_visitChildren(cursor, vis, &mut ctx as *mut _ as CXClientData);
    ctx.children
}

unsafe fn first_child_expr(cursor: clang_sys::CXCursor) -> Option<CExpression> {
    collect_children(cursor).into_iter().find_map(|c| extract_expr(c))
}

// ─────────────────────────────────────────────────────────────────────────────
// File I/O
// ─────────────────────────────────────────────────────────────────────────────

pub fn read_c_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .context(format!("Failed to read C file: {}", path.display()))
}
