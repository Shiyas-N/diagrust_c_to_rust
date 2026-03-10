// ast_input/src/parser.rs
//
// Clang-AST based C parser.
// Uses clang-sys (libclang bindings) to obtain a proper AST and walks it
// into our internal CProgram representation.

#![allow(non_upper_case_globals)]

use anyhow::{Context, Result};
use std::path::Path;

use crate::types::*;

// ─────────────────────────────────────────────────────────────────────────────
// ClangParser – Clang-AST based parser
// ─────────────────────────────────────────────────────────────────────────────

pub struct ClangParser;

impl ClangParser {
    pub fn parse(source: &str) -> Result<CProgram> {
        use clang_sys::*;
        use std::ffi::CString;
        use std::os::raw::c_int;

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

pub(crate) unsafe fn cx_string(s: clang_sys::CXString) -> String {
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

pub(crate) unsafe fn cursor_spelling(c: clang_sys::CXCursor) -> String {
    cx_string(clang_sys::clang_getCursorSpelling(c))
}

pub(crate) unsafe fn type_spelling(t: clang_sys::CXType) -> String {
    cx_string(clang_sys::clang_getTypeSpelling(t))
}

// ── Type conversion ────────────────────────────────────────────────────────────

pub(crate) unsafe fn cx_type_to_ctype(ty: clang_sys::CXType) -> CType {
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

pub(crate) unsafe fn extract_compound_stmt(cursor: clang_sys::CXCursor) -> Vec<CStatement> {
    use clang_sys::*;

    struct Ctx { stmts: Vec<CStatement> }

    extern "C" fn vis(
        cursor: CXCursor,
        _parent: CXCursor,
        data: CXClientData,
    ) -> CXChildVisitResult {
        let ctx = unsafe { &mut *(data as *mut Ctx) };
        if unsafe { clang_getCursorKind(cursor) } == CXCursor_DeclStmt {
            let mut decls: Vec<CStatement> = Vec::new();
            extern "C" fn dv(c: CXCursor, _p: CXCursor, d: CXClientData) -> CXChildVisitResult {
                let decls = unsafe { &mut *(d as *mut Vec<CStatement>) };
                if unsafe { clang_getCursorKind(c) } == CXCursor_VarDecl {
                    if let Some(s) = unsafe { extract_var_decl(c) } {
                        decls.push(s);
                    }
                }
                CXChildVisit_Continue
            }
            unsafe { clang_visitChildren(cursor, dv, &mut decls as *mut _ as CXClientData) };
            ctx.stmts.extend(decls);
            return CXChildVisit_Continue;
        }
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
        CXCursor_CompoundStmt => None,

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

        // ── Switch statement ──────────────────────────────────────────────
        CXCursor_SwitchStmt => extract_switch_stmt(cursor),

        // ── Do-while loop ─────────────────────────────────────────────────
        CXCursor_DoStmt => {
            let ch = collect_children(cursor);
            let body = ch.get(0).map(|c| extract_stmt_as_block(*c)).unwrap_or_default();
            let cond = ch.get(1).and_then(|c| extract_expr(*c)).unwrap_or(CExpression::Null);
            Some(CStatement::DoWhile { body, condition: cond })
        }

        _ => {
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

// ── Switch statement extraction ────────────────────────────────────────────────

unsafe fn extract_switch_stmt(cursor: clang_sys::CXCursor) -> Option<CStatement> {
    use clang_sys::*;

    let ch = collect_children(cursor);
    if ch.is_empty() { return None; }

    // First child is the switch expression
    let switch_expr = extract_expr(ch[0])?;

    // The rest is the compound statement body containing CaseStmt / DefaultStmt
    let body_cursor = ch.get(1)?;
    let body_children = collect_children(*body_cursor);

    let mut cases: Vec<(Vec<CExpression>, Vec<CStatement>)> = Vec::new();
    let mut default: Option<Vec<CStatement>> = None;

    let mut i = 0;
    while i < body_children.len() {
        let child = body_children[i];
        let child_kind = clang_getCursorKind(child);

        if child_kind == CXCursor_CaseStmt {
            // CaseStmt children: [0] = case value expr, [1..] = body statements
            let case_children = collect_children(child);
            let mut values = Vec::new();
            let mut body_stmts = Vec::new();

            if let Some(&val_cursor) = case_children.get(0) {
                if let Some(val) = extract_expr(val_cursor) {
                    values.push(val);
                }
            }

            // Remaining children of this CaseStmt are body statements
            for &sc in case_children.iter().skip(1) {
                let sk = clang_getCursorKind(sc);
                if sk == CXCursor_CaseStmt {
                    // Fall-through to next case — extract its value and merge
                    let nested_ch = collect_children(sc);
                    if let Some(&nv) = nested_ch.get(0) {
                        if let Some(val) = extract_expr(nv) {
                            values.push(val);
                        }
                    }
                    for &nsc in nested_ch.iter().skip(1) {
                        if let Some(s) = extract_stmt(nsc) {
                            body_stmts.push(s);
                        } else if clang_getCursorKind(nsc) == CXCursor_CompoundStmt {
                            body_stmts.extend(extract_compound_stmt(nsc));
                        }
                    }
                } else if sk == CXCursor_DefaultStmt {
                    // Default nested inside a case
                    let def_ch = collect_children(sc);
                    let mut def_stmts = Vec::new();
                    for &dc in &def_ch {
                        if let Some(s) = extract_stmt(dc) {
                            def_stmts.push(s);
                        } else if clang_getCursorKind(dc) == CXCursor_CompoundStmt {
                            def_stmts.extend(extract_compound_stmt(dc));
                        }
                    }
                    default = Some(def_stmts);
                } else if sk == CXCursor_CompoundStmt {
                    body_stmts.extend(extract_compound_stmt(sc));
                } else if let Some(s) = extract_stmt(sc) {
                    body_stmts.push(s);
                }
            }

            // Remove trailing Break (it's implicit in Rust's match)
            if matches!(body_stmts.last(), Some(CStatement::Break)) {
                body_stmts.pop();
            }

            cases.push((values, body_stmts));
        } else if child_kind == CXCursor_DefaultStmt {
            let def_ch = collect_children(child);
            let mut def_stmts = Vec::new();
            for &dc in &def_ch {
                if let Some(s) = extract_stmt(dc) {
                    def_stmts.push(s);
                } else if clang_getCursorKind(dc) == CXCursor_CompoundStmt {
                    def_stmts.extend(extract_compound_stmt(dc));
                }
            }
            // Remove trailing Break
            if matches!(def_stmts.last(), Some(CStatement::Break)) {
                def_stmts.pop();
            }
            default = Some(def_stmts);
        }

        i += 1;
    }

    Some(CStatement::Switch { expr: switch_expr, cases, default })
}

// ── Variable declaration ───────────────────────────────────────────────────────

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

    let init = unsafe {
        let children = collect_children(cursor);
        match &var_type {
            CType::Array(_, _) => {
                children.iter().find_map(|&c| {
                    if clang_sys::clang_getCursorKind(c) == clang_sys::CXCursor_InitListExpr {
                        extract_expr(c)
                    } else {
                        None
                    }
                })
            }
            _ => {
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
                fn field_path(expr: CExpression) -> Option<String> {
                    match expr {
                        CExpression::Variable(v) => Some(v),
                        CExpression::FieldAccess { object, field } => {
                            Some(format!("{}.{}", field_path(*object)?, field))
                        }
                        _ => None,
                    }
                }
                if let Some(obj_path) = field_path(*object) {
                    return Some(CStatement::FieldAssignment {
                        object: obj_path, field, value: *right,
                    });
                }
            }
            _ => {}
        }
    }
    Some(CStatement::Expression(extract_expr(cursor)?))
}

// ── Expression extraction ──────────────────────────────────────────────────────

pub(crate) unsafe fn extract_expr(cursor: clang_sys::CXCursor) -> Option<CExpression> {
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
            let cx_type = clang_getCursorType(cursor);
            let type_kind = cx_type.kind;
            let ch = collect_children(cursor);
            if type_kind == clang_sys::CXType_ConstantArray
                || type_kind == clang_sys::CXType_VariableArray
                || type_kind == clang_sys::CXType_IncompleteArray
            {
                let elems = ch.iter()
                    .filter_map(|&c| extract_expr(c))
                    .collect();
                return Some(CExpression::ArrayLiteral(elems));
            }
            let raw = type_spelling(cx_type);
            let struct_type = raw.trim_start_matches("struct ").to_string();
            let fields = ch.iter().enumerate()
                .filter_map(|(i, &c)| extract_expr(c).map(|e| (format!("_field{}", i), e)))
                .collect();
            Some(CExpression::StructLiteral { struct_type, fields })
        }
        other => {
            if other == 617 {
                Some(CExpression::Literal(CLiteral::Int(0)))
            } else if other == CXCursor_UnexposedExpr {
                first_child_expr(cursor)
            } else {
                None
            }
        }
    }
}

// ── Binary/Unary operators ─────────────────────────────────────────────────────

unsafe fn extract_binary(cursor: clang_sys::CXCursor) -> Option<CExpression> {
    let ch    = collect_children(cursor);
    let left  = extract_expr(*ch.get(0)?)?;
    let right = extract_expr(*ch.get(1)?)?;
    let op    = binary_op_from_tokens(cursor);
    Some(CExpression::BinaryOp { left: Box::new(left), op, right: Box::new(right) })
}

unsafe fn binary_op_from_tokens(cursor: clang_sys::CXCursor) -> BinOp {
    use clang_sys::*;

    let ch = collect_children(cursor);
    if ch.len() < 2 { return BinOp::Add; }

    let tu = clang_Cursor_getTranslationUnit(cursor);

    let left_range  = clang_getCursorExtent(ch[0]);
    let right_range = clang_getCursorExtent(ch[1]);

    let left_end   = clang_getRangeEnd(left_range);
    let right_start = clang_getRangeStart(right_range);

    let mut left_offset  = 0u32;
    let mut right_offset = 0u32;
    let mut file: CXFile = std::ptr::null_mut();

    clang_getSpellingLocation(left_end,   &mut file, std::ptr::null_mut(), std::ptr::null_mut(), &mut left_offset);
    clang_getSpellingLocation(right_start, &mut file, std::ptr::null_mut(), std::ptr::null_mut(), &mut right_offset);

    let full_range = clang_getCursorExtent(cursor);
    let mut toks: *mut CXToken = std::ptr::null_mut();
    let mut n: u32 = 0;
    clang_tokenize(tu, full_range, &mut toks, &mut n);
    if toks.is_null() || n == 0 { return BinOp::Add; }

    let tokens = std::slice::from_raw_parts(toks, n as usize);
    let mut op = BinOp::Add;

    for tok in tokens {
        if clang_getTokenKind(*tok) != CXToken_Punctuation { continue; }

        let tok_loc = clang_getTokenLocation(tu, *tok);
        let mut tok_offset = 0u32;
        clang_getSpellingLocation(tok_loc, &mut file, std::ptr::null_mut(), std::ptr::null_mut(), &mut tok_offset);

        if tok_offset > left_offset && tok_offset < right_offset {
            let sp = cx_string(clang_getTokenSpelling(tu, *tok));
            match sp.as_str() {
                "(" | ")" | "[" | "]" | "." | "->" | ";" | "{" | "}" => continue,
                _ => {}
            }
            op = str_to_binop(&sp);
            break;
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

    let mut op_str    = String::new();
    let mut is_prefix = true;
    if !toks.is_null() && n > 0 {
        let tokens = std::slice::from_raw_parts(toks, n as usize);
        if clang_getTokenKind(tokens[0]) == CXToken_Punctuation {
            op_str = cx_string(clang_getTokenSpelling(tu, tokens[0]));
            is_prefix = true;
        } else if n >= 2 && clang_getTokenKind(tokens[n as usize - 1]) == CXToken_Punctuation {
            op_str = cx_string(clang_getTokenSpelling(tu, tokens[n as usize - 1]));
            is_prefix = false;
        }
        clang_disposeTokens(tu, toks, n);
    }

    let operand = first_child_expr(cursor)?;

    match op_str.as_str() {
        "-"  if is_prefix =>
            Some(CExpression::UnaryOp { op: UnaryOp::Neg,    operand: Box::new(operand) }),
        "!"  =>
            Some(CExpression::UnaryOp { op: UnaryOp::Not,    operand: Box::new(operand) }),
        "*"  if is_prefix =>
            Some(CExpression::UnaryOp { op: UnaryOp::Deref,  operand: Box::new(operand) }),
        "&"  if is_prefix =>
            Some(CExpression::UnaryOp { op: UnaryOp::AddrOf, operand: Box::new(operand) }),
        "~"  =>
            Some(CExpression::UnaryOp { op: UnaryOp::BitNot, operand: Box::new(operand) }),
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
    let stripped = raw.trim_end_matches(|c: char| {
        matches!(c, 'u' | 'U' | 'l' | 'L')
    });
    if stripped.starts_with("0x") || stripped.starts_with("0X") {
        i64::from_str_radix(&stripped[2..], 16).unwrap_or(0)
    }
    else if stripped.starts_with('0') && stripped.len() > 1 {
        i64::from_str_radix(&stripped[1..], 8).unwrap_or(0)
    }
    else {
        stripped.parse::<i64>().unwrap_or(0)
    }
}

unsafe fn eval_float(cursor: clang_sys::CXCursor) -> f64 {
    let raw = token_spelling_of_cursor(cursor);
    let stripped = raw.trim_end_matches(|c: char| {
        matches!(c, 'f' | 'F' | 'l' | 'L')
    });
    stripped.parse::<f64>().unwrap_or(0.0)
}

// ── Cursor child helpers ───────────────────────────────────────────────────────

pub(crate) unsafe fn collect_children(cursor: clang_sys::CXCursor) -> Vec<clang_sys::CXCursor> {
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
