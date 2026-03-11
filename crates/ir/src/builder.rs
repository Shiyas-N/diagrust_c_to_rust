// ir/src/builder.rs
//
// IR Builder — converts the C AST into the intermediate representation.

use ast_input::{CType, CExpression, CStatement, CProgram};
use semantic::{self, OwnershipKind, SemanticModel};
use std::collections::{HashMap, HashSet};

use crate::types::*;

// ─────────────────────────────────────────────────────────────────────────────
// IR Builder
// ─────────────────────────────────────────────────────────────────────────────

pub struct IrBuilder;

impl IrBuilder {
    pub fn build(program: &CProgram, semantic: &SemanticModel) -> anyhow::Result<IrProgram> {
        let mut functions = Vec::new();
        let mut required_uses: HashSet<String> = Default::default();

        let struct_fields: HashMap<String, Vec<String>> = program.structs.iter()
            .map(|s| (s.name.clone(), s.fields.iter().map(|f| f.name.clone()).collect()))
            .collect();

        let global_names: HashSet<String> = program.global_vars.iter()
            .map(|g| g.name.clone())
            .collect();

        let fn_param_types: HashMap<String, Vec<String>> = program.functions.iter().map(|f| {
            let array_params = Self::detect_array_params(f);
            // Check if this function returns *mut T (will be upgraded to &mut T)
            let ret_raw = Self::fallback_type(&f.return_type);
            let return_inner = if ret_raw.starts_with("*mut ") {
                let inner = ret_raw["*mut ".len()..].to_string();
                // Only upgrade if at least one param is *mut T of the same inner type
                let has_ptr_param = f.parameters.iter().any(|p| {
                    Self::fallback_type(&p.param_type) == format!("*mut {}", inner)
                });
                if has_ptr_param { Some(inner) } else { None }
            } else { None };

            let types: Vec<String> = f.parameters.iter().map(|p| {
                if array_params.contains(&p.name) {
                    let inner = match &p.param_type {
                        CType::Pointer(inner) | CType::Array(inner, _) => Self::fallback_type(inner),
                        other => Self::fallback_type(other),
                    };
                    format!("&mut [{}]", inner)
                } else {
                    let base = Self::fallback_type(&p.param_type);
                    // Upgrade *mut T → &mut T for pointer-return functions
                    if let Some(ref inner) = return_inner {
                        if base == format!("*mut {}", inner) {
                            return format!("&mut {}", inner);
                        }
                    }
                    base
                }
            }).collect();
            (f.name.clone(), types)
        }).collect();

        for (func, sem) in program.functions.iter().zip(semantic.functions.iter()) {
            let ir_func = Self::build_function(
                func, sem, &mut required_uses, &struct_fields, &global_names, &fn_param_types
            )?;
            functions.push(ir_func);
        }

        let global_vars = program.global_vars.iter().map(|gv| {
            let rust_type = Self::fallback_type(&gv.var_type);
            let init = gv.init.as_ref().and_then(|e| {
                let dummy_sem = semantic::FunctionSemantics::empty();
                Self::build_expr(e, &dummy_sem, &struct_fields, &HashMap::new()).ok()
            });
            IrGlobalVar { name: gv.name.clone(), rust_type, init, is_const: gv.is_const }
        }).collect();

        // Smart import injection: only add what the generated IR actually uses.
        // std::io::{self, Write} is only needed when print! (with flush) is emitted.
        // std::ptr is only needed when raw pointer operations appear.
        if Self::ir_uses_print_flush(&functions) {
            required_uses.insert("std::io::{self, Write}".to_string());
        }
        if Self::ir_uses_raw_ptr(&functions) {
            required_uses.insert("std::ptr".to_string());
        }

        let structs = program.structs.iter().map(|sd| IrStructDef {
            name: sd.name.clone(),
            fields: sd.fields.iter().map(|f| IrStructField {
                name: f.name.clone(),
                rust_type: Self::fallback_type(&f.field_type),
            }).collect(),
        }).collect();

        Ok(IrProgram {
            functions,
            structs,
            global_vars,
            required_uses: required_uses.into_iter().collect(),
            is_executable: program.has_main,
        })
    }

    // ── Array parameter detection ────────────────────────────────────────────

    fn detect_array_params(func: &ast_input::CFunction) -> HashSet<String> {
        let param_names: HashSet<&str> = func.parameters.iter().map(|p| p.name.as_str()).collect();
        let mut array_params = HashSet::new();
        Self::scan_stmts_for_array_params(&func.body, &param_names, &mut array_params);
        array_params
    }

    fn scan_stmts_for_array_params(
        stmts: &[CStatement],
        params: &HashSet<&str>,
        out: &mut HashSet<String>,
    ) {
        for stmt in stmts {
            match stmt {
                CStatement::IndexedAssignment { array, .. } => {
                    if params.contains(array.as_str()) { out.insert(array.clone()); }
                }
                CStatement::Expression(e) | CStatement::Return(Some(e)) => {
                    Self::scan_expr_for_array_params(e, params, out);
                }
                CStatement::VarDecl { init: Some(e), .. } => {
                    Self::scan_expr_for_array_params(e, params, out);
                }
                CStatement::Assignment { value, .. } => {
                    Self::scan_expr_for_array_params(value, params, out);
                }
                CStatement::If { condition, then_body, else_body } => {
                    Self::scan_expr_for_array_params(condition, params, out);
                    Self::scan_stmts_for_array_params(then_body, params, out);
                    if let Some(eb) = else_body {
                        Self::scan_stmts_for_array_params(eb, params, out);
                    }
                }
                CStatement::While { condition, body } => {
                    Self::scan_expr_for_array_params(condition, params, out);
                    Self::scan_stmts_for_array_params(body, params, out);
                }
                CStatement::For { init, condition, increment, body } => {
                    Self::scan_stmts_for_array_params(std::slice::from_ref(init), params, out);
                    Self::scan_expr_for_array_params(condition, params, out);
                    Self::scan_expr_for_array_params(increment, params, out);
                    Self::scan_stmts_for_array_params(body, params, out);
                }
                CStatement::Switch { cases, default, .. } => {
                    for (_, body) in cases {
                        Self::scan_stmts_for_array_params(body, params, out);
                    }
                    if let Some(d) = default {
                        Self::scan_stmts_for_array_params(d, params, out);
                    }
                }
                CStatement::DoWhile { body, condition } => {
                    Self::scan_stmts_for_array_params(body, params, out);
                    Self::scan_expr_for_array_params(condition, params, out);
                }
                _ => {}
            }
        }
    }

    fn scan_expr_for_array_params(
        expr: &CExpression,
        params: &HashSet<&str>,
        out: &mut HashSet<String>,
    ) {
        match expr {
            CExpression::ArrayAccess { array, index } => {
                if let CExpression::Variable(name) = array.as_ref() {
                    if params.contains(name.as_str()) {
                        out.insert(name.clone());
                    }
                }
                Self::scan_expr_for_array_params(array, params, out);
                Self::scan_expr_for_array_params(index, params, out);
            }
            CExpression::BinaryOp { left, right, .. } => {
                Self::scan_expr_for_array_params(left, params, out);
                Self::scan_expr_for_array_params(right, params, out);
            }
            CExpression::Call { args, .. } => {
                for a in args { Self::scan_expr_for_array_params(a, params, out); }
            }
            CExpression::UnaryOp { operand, .. } => {
                Self::scan_expr_for_array_params(operand, params, out);
            }
            _ => {}
        }
    }

    // ── Function building ────────────────────────────────────────────────────

    fn build_function(
        func: &ast_input::CFunction,
        sem: &semantic::FunctionSemantics,
        uses: &mut HashSet<String>,
        struct_fields: &HashMap<String, Vec<String>>,
        global_names: &HashSet<String>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> anyhow::Result<IrFunction> {
        let is_main = func.name == "main";

        let array_params = Self::detect_array_params(func);

        let parameters: Vec<IrParameter> = if is_main {
            vec![]
        } else {
            // Pre-check: if return type is `*mut T` and we have `*mut T` pointer params,
            // we will upgrade return to `&mut T`.  In that case, also upgrade the params.
            let ret_raw = Self::fallback_type(&func.return_type);
            let return_inner = if ret_raw.starts_with("*mut ") {
                Some(ret_raw["*mut ".len()..].to_string())
            } else { None };

            func.parameters.iter().map(|p| {
                let rt = if array_params.contains(&p.name) {
                    let inner = match &p.param_type {
                        CType::Pointer(inner) | CType::Array(inner, _) => Self::fallback_type(inner),
                        other => Self::fallback_type(other),
                    };
                    format!("&mut [{}]", inner)
                } else {
                    let base_rt = sem.rust_type_map.get(&p.name)
                        .map(|r| r.rust_repr.clone())
                        .unwrap_or_else(|| Self::fallback_type(&p.param_type));

                    // Upgrade *mut T → &mut T when the function returns a reference
                    // to one of its pointer parameters (same inner type).
                    if let Some(ref inner) = return_inner {
                        if base_rt == format!("*mut {}", inner) {
                            format!("&mut {}", inner)
                        } else {
                            base_rt
                        }
                    } else {
                        base_rt
                    }
                };
                let ownership = sem.ownership_map.get(&p.name).cloned().unwrap_or(OwnershipKind::Value);
                let is_mut = array_params.contains(&p.name) || sem.mutable_vars.contains(&p.name);
                IrParameter { name: p.name.clone(), rust_type: rt, is_mutable: is_mut, ownership }
            }).collect()
        };

        let requires_unsafe = sem.rust_type_map.values().any(|rt| rt.requires_unsafe);
        if requires_unsafe { uses.insert("std::ptr".to_string()); }
        if sem.ownership_map.values().any(|o| *o == OwnershipKind::SharedOwned) {
            uses.insert("std::rc::Rc".to_string());
        }
        if sem.ownership_map.values().any(|o| *o == OwnershipKind::InteriorMutable) {
            uses.insert("std::cell::RefCell".to_string());
        }
        // Raw pointers need std::ptr for null/null_mut helpers
        if sem.ownership_map.values().any(|o| *o == OwnershipKind::RawPointer) {
            uses.insert("std::ptr".to_string());
        }

        let rust_return_type = if is_main {
            "()".to_string()
        } else {
            // If the function returns a pointer (`*mut T`) and has pointer/mut-ref params
            // of the same inner type, translate the return to `&mut T` so that Rust
            // lifetime rules apply cleanly (codegen will add `<'a>` annotations).
            let raw_ret = Self::fallback_type(&func.return_type);
            if raw_ret.starts_with("*mut ") {
                let inner = &raw_ret["*mut ".len()..];
                let any_ref_param = func.parameters.iter().any(|p| {
                    let pt = Self::fallback_type(&p.param_type);
                    pt == format!("*mut {}", inner) || pt == format!("&mut {}", inner)
                });
                if any_ref_param {
                    format!("&mut {}", inner)
                } else {
                    raw_ret
                }
            } else {
                raw_ret
            }
        };

        let raw_body = Self::build_stmts(&func.body, sem, struct_fields, fn_param_types)?;
        let raw_body = if is_main { Self::strip_main_return(raw_body) } else { raw_body };

        let raw_body = Self::rewrite_swap_patterns(raw_body);

        let (body, needs_unsafe_for_globals) = if !global_names.is_empty() {
            Self::detect_global_accesses(raw_body, global_names)
        } else {
            (raw_body, false)
        };

        let requires_unsafe = requires_unsafe || needs_unsafe_for_globals;

        Ok(IrFunction { name: func.name.clone(), rust_return_type, parameters, body, requires_unsafe, is_main })
    }

    // ── Swap pattern detection ───────────────────────────────────────────────

    fn rewrite_swap_patterns(stmts: Vec<IrStatement>) -> Vec<IrStatement> {
        let mut out = Vec::with_capacity(stmts.len());
        let mut i = 0;
        while i < stmts.len() {
            if i + 2 < stmts.len() {
                if let Some(swap) = Self::try_match_swap(&stmts[i], &stmts[i+1], &stmts[i+2]) {
                    out.push(swap);
                    i += 3;
                    continue;
                }
            }
            let stmt = match stmts[i].clone() {
                IrStatement::If { condition, then_block, else_block } => IrStatement::If {
                    condition,
                    then_block: Self::rewrite_swap_patterns(then_block),
                    else_block: else_block.map(Self::rewrite_swap_patterns),
                },
                IrStatement::While { condition, body } => IrStatement::While {
                    condition,
                    body: Self::rewrite_swap_patterns(body),
                },
                IrStatement::While2 { init, condition, increment, body } => IrStatement::While2 {
                    init,
                    condition,
                    increment,
                    body: Self::rewrite_swap_patterns(body),
                },
                IrStatement::Match { expr, arms, default } => IrStatement::Match {
                    expr,
                    arms: arms.into_iter().map(|(pats, body)| (pats, Self::rewrite_swap_patterns(body))).collect(),
                    default: default.map(Self::rewrite_swap_patterns),
                },
                IrStatement::Loop { body, condition } => IrStatement::Loop {
                    body: Self::rewrite_swap_patterns(body),
                    condition,
                },
                IrStatement::UnsafeBlock(inner) => IrStatement::UnsafeBlock(
                    Self::rewrite_swap_patterns(inner)
                ),
                other => other,
            };
            out.push(stmt);
            i += 1;
        }
        out
    }

    fn try_match_swap(s0: &IrStatement, s1: &IrStatement, s2: &IrStatement) -> Option<IrStatement> {
        let (tmp_name, arr_name0, idx_i) = match s0 {
            IrStatement::LetBinding { name: tmp, init: Some(IrExpr::ArrayAccess { array, index }), .. } => {
                if let IrExpr::Var(arr) = array.as_ref() {
                    (tmp.clone(), arr.clone(), index.as_ref().clone())
                } else { return None; }
            }
            _ => return None,
        };
        let (arr_name1, idx_i2, idx_j) = match s1 {
            IrStatement::IndexAssign { array: arr, index: idx_i2, value: IrExpr::ArrayAccess { array: arr2, index: idx_j } } => {
                if let IrExpr::Var(arr2) = arr2.as_ref() {
                    if arr == arr2 { (arr.clone(), idx_i2.clone(), idx_j.as_ref().clone()) } else { return None; }
                } else { return None; }
            }
            _ => return None,
        };
        let (arr_name2, idx_j2, src) = match s2 {
            IrStatement::IndexAssign { array: arr, index: idx_j2, value: IrExpr::Var(src) } => {
                (arr.clone(), idx_j2.clone(), src.clone())
            }
            _ => return None,
        };

        if arr_name0 == arr_name1 && arr_name1 == arr_name2
            && src == tmp_name
            && expr_eq(&idx_i, &idx_i2) && expr_eq(&idx_j, &idx_j2)
        {
            Some(IrStatement::SwapElements { array: arr_name0, i: idx_i, j: idx_j })
        } else {
            None
        }
    }

    // ── Global access detection ──────────────────────────────────────────────

    fn detect_global_accesses(
        stmts: Vec<IrStatement>,
        globals: &HashSet<String>,
    ) -> (Vec<IrStatement>, bool) {
        let touched = Self::any_stmt_touches_globals(&stmts, globals);
        (stmts, touched)
    }

    fn any_stmt_touches_globals(stmts: &[IrStatement], globals: &HashSet<String>) -> bool {
        stmts.iter().any(|s| Self::stmt_touches_globals_deep(s, globals))
    }

    fn stmt_touches_globals_deep(stmt: &IrStatement, globals: &HashSet<String>) -> bool {
        match stmt {
            IrStatement::UnsafeBlock(inner) =>
                Self::any_stmt_touches_globals(inner, globals),
            IrStatement::If { condition, then_block, else_block } =>
                Self::expr_touches_globals(condition, globals)
                || Self::any_stmt_touches_globals(then_block, globals)
                || else_block.as_deref().map_or(false, |b| Self::any_stmt_touches_globals(b, globals)),
            IrStatement::While { condition, body } =>
                Self::expr_touches_globals(condition, globals)
                || Self::any_stmt_touches_globals(body, globals),
            IrStatement::While2 { condition, increment, body, .. } =>
                Self::expr_touches_globals(condition, globals)
                || Self::expr_touches_globals(increment, globals)
                || Self::any_stmt_touches_globals(body, globals),
            IrStatement::Match { expr, arms, default } =>
                Self::expr_touches_globals(expr, globals)
                || arms.iter().any(|(_, body)| Self::any_stmt_touches_globals(body, globals))
                || default.as_deref().map_or(false, |d| Self::any_stmt_touches_globals(d, globals)),
            IrStatement::Loop { body, condition } =>
                Self::any_stmt_touches_globals(body, globals)
                || Self::expr_touches_globals(condition, globals),
            other => Self::stmt_touches_globals(other, globals),
        }
    }

    fn stmt_touches_globals(stmt: &IrStatement, globals: &HashSet<String>) -> bool {
        match stmt {
            IrStatement::UnsafeBlock(_) => false,
            IrStatement::Assign { target, value } =>
                globals.contains(target) || Self::expr_touches_globals(value, globals),
            IrStatement::LetBinding { init: Some(e), .. } => Self::expr_touches_globals(e, globals),
            IrStatement::Expr(e) => Self::expr_touches_globals(e, globals),
            IrStatement::Return(Some(e)) => Self::expr_touches_globals(e, globals),
            IrStatement::FieldAssign { object, value, .. } =>
                globals.contains(object) || Self::expr_touches_globals(value, globals),
            IrStatement::IndexAssign { array, index, value } =>
                globals.contains(array)
                || Self::expr_touches_globals(index, globals)
                || Self::expr_touches_globals(value, globals),
            _ => false,
        }
    }

    fn expr_touches_globals(expr: &IrExpr, globals: &HashSet<String>) -> bool {
        match expr {
            IrExpr::Var(n) => globals.contains(n),
            IrExpr::BinOp { left, right, .. } =>
                Self::expr_touches_globals(left, globals) || Self::expr_touches_globals(right, globals),
            IrExpr::ArrayAccess { array, index } =>
                Self::expr_touches_globals(array, globals) || Self::expr_touches_globals(index, globals),
            IrExpr::FieldAccess { object, .. } => Self::expr_touches_globals(object, globals),
            IrExpr::Call { args, .. } => args.iter().any(|a| Self::expr_touches_globals(a, globals)),
            IrExpr::Deref(e) | IrExpr::UnaryOp { operand: e, .. } => Self::expr_touches_globals(e, globals),
            _ => false,
        }
    }

    fn strip_main_return(stmts: Vec<IrStatement>) -> Vec<IrStatement> {
        stmts.into_iter().filter(|s| {
            !matches!(s, IrStatement::Return(Some(IrExpr::IntLit(0)))
                       | IrStatement::Return(None))
        }).collect()
    }

    // ── Statement building ───────────────────────────────────────────────────

    fn build_stmts(
        stmts: &[CStatement],
        sem: &semantic::FunctionSemantics,
        struct_fields: &HashMap<String, Vec<String>>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> anyhow::Result<Vec<IrStatement>> {
        stmts.iter().map(|s| Self::build_stmt(s, sem, struct_fields, fn_param_types)).collect()
    }

    fn build_stmt(
        stmt: &CStatement,
        sem: &semantic::FunctionSemantics,
        struct_fields: &HashMap<String, Vec<String>>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> anyhow::Result<IrStatement> {
        use ast_input::CStatement as CS;
        Ok(match stmt {
            CS::VarDecl { name, var_type, init } => {
                let rust_type = sem.rust_type_map.get(name)
                    .map(|r| r.rust_repr.clone())
                    .unwrap_or_else(|| Self::fallback_type(var_type));
                let is_mutable = sem.mutable_vars.contains(name) || init.is_none();
                let init_ir = match init {
                    Some(e) => {
                        let expr = Self::build_expr(e, sem, struct_fields, fn_param_types)?;
                        let expr = Self::coerce_to_type(expr, &rust_type);
                        Some(expr)
                    }
                    None => None,
                };
                IrStatement::LetBinding { name: name.clone(), rust_type, init: init_ir, is_mutable }
            }

            CS::Assignment { target, value } => {
                if let Some(alloc) = Self::try_rewrite_alloc(value, sem, struct_fields, fn_param_types) {
                    return Ok(IrStatement::Assign { target: target.clone(), value: alloc });
                }
                IrStatement::Assign { target: target.clone(), value: Self::build_expr(value, sem, struct_fields, fn_param_types)? }
            }

            CS::IndexedAssignment { array, index, value } =>
                IrStatement::IndexAssign {
                    array: array.clone(),
                    index: Self::build_index_expr(index, sem, struct_fields, fn_param_types)?,
                    value: Self::build_expr(value, sem, struct_fields, fn_param_types)?,
                },

            CS::FieldAssignment { object, field, value } =>
                IrStatement::FieldAssign {
                    object: object.clone(), field: field.clone(),
                    value: Self::build_expr(value, sem, struct_fields, fn_param_types)?,
                },

            CS::Return(Some(e)) => IrStatement::Return(Some(Self::build_expr(e, sem, struct_fields, fn_param_types)?)),
            CS::Return(None)    => IrStatement::Return(None),
            CS::Break           => IrStatement::Break,
            CS::Continue        => IrStatement::Continue,

            CS::Expression(e) => {
                if let CExpression::Call { function, args } = e {
                    if function == "free" {
                        if let Some(CExpression::Variable(v)) = args.first() {
                            return Ok(IrStatement::Drop(v.clone()));
                        }
                    }
                }
                if let Some(ir) = Self::try_rewrite_io_stmt(e, sem, struct_fields, fn_param_types) {
                    return Ok(ir);
                }
                IrStatement::Expr(Self::build_expr(e, sem, struct_fields, fn_param_types)?)
            }

            CS::If { condition, then_body, else_body } => IrStatement::If {
                condition: Self::build_expr(condition, sem, struct_fields, fn_param_types)?,
                then_block: Self::build_stmts(then_body, sem, struct_fields, fn_param_types)?,
                else_block: if let Some(eb) = else_body {
                    Some(Self::build_stmts(eb, sem, struct_fields, fn_param_types)?)
                } else { None },
            },

            CS::While { condition, body } => IrStatement::While {
                condition: Self::build_expr(condition, sem, struct_fields, fn_param_types)?,
                body: Self::build_stmts(body, sem, struct_fields, fn_param_types)?,
            },

            CS::For { init, condition, increment, body } => {
                let mut init_stmt = Self::build_stmt(init, sem, struct_fields, fn_param_types)?;
                if let IrStatement::LetBinding { ref mut is_mutable, .. } = init_stmt {
                    *is_mutable = true;
                }
                IrStatement::While2 {
                    init: Box::new(init_stmt),
                    condition: Self::build_expr(condition, sem, struct_fields, fn_param_types)?,
                    increment: Self::build_expr(increment, sem, struct_fields, fn_param_types)?,
                    body: Self::build_stmts(body, sem, struct_fields, fn_param_types)?,
                }
            }

            // ── Switch → Match ───────────────────────────────────────────
            CS::Switch { expr, cases, default } => {
                let match_expr = Self::build_expr(expr, sem, struct_fields, fn_param_types)?;
                let mut arms = Vec::new();
                for (values, body) in cases {
                    let patterns: Vec<IrExpr> = values.iter()
                        .map(|v| Self::build_expr(v, sem, struct_fields, fn_param_types))
                        .collect::<anyhow::Result<_>>()?;
                    let body_stmts = Self::build_stmts(body, sem, struct_fields, fn_param_types)?;
                    arms.push((patterns, body_stmts));
                }
                let default_block = if let Some(d) = default {
                    Some(Self::build_stmts(d, sem, struct_fields, fn_param_types)?)
                } else { None };
                IrStatement::Match { expr: match_expr, arms, default: default_block }
            }

            // ── Do-While → Loop ──────────────────────────────────────────
            CS::DoWhile { body, condition } => {
                let body_stmts = Self::build_stmts(body, sem, struct_fields, fn_param_types)?;
                let cond = Self::build_expr(condition, sem, struct_fields, fn_param_types)?;
                IrStatement::Loop { body: body_stmts, condition: cond }
            }

            CS::StructDecl { name, struct_type, fields } => {
                let real_names = struct_fields.get(struct_type);
                let init = if fields.is_empty() {
                    Some(IrExpr::StructDefault(struct_type.clone()))
                } else {
                    let resolved: Vec<(String, IrExpr)> = fields.iter().enumerate()
                        .map(|(i, (fname, fexpr))| {
                            let real = real_names.and_then(|ns| ns.get(i)).cloned().unwrap_or_else(|| fname.clone());
                            Self::build_expr(fexpr, sem, struct_fields, fn_param_types).map(|e| (real, e))
                        })
                        .collect::<anyhow::Result<_>>()?;
                    Some(IrExpr::StructLiteral { struct_type: struct_type.clone(), fields: resolved })
                };
                IrStatement::LetBinding { name: name.clone(), rust_type: struct_type.clone(), init, is_mutable: true }
            }
        })
    }

    // ── stdio rewriting ──────────────────────────────────────────────────────

    fn try_rewrite_io_stmt(
        expr: &CExpression,
        sem: &semantic::FunctionSemantics,
        struct_fields: &HashMap<String, Vec<String>>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> Option<IrStatement> {
        use ast_input::CExpression as CE;
        if let CE::Call { function, args } = expr {
            match function.as_str() {
                "printf" | "fprintf" => {
                    let (is_err, fmt_idx) = if function == "fprintf" { (true, 1) } else { (false, 0) };
                    let fmt_arg = args.get(fmt_idx)?;
                    let (rust_fmt, rest_args) = Self::translate_printf_fmt(fmt_arg, &args[fmt_idx + 1..]);
                    let ir_args = rest_args.iter()
                        .map(|a| Self::build_expr(a, sem, struct_fields, fn_param_types))
                        .collect::<anyhow::Result<Vec<_>>>().ok()?;
                    let ends_newline = rust_fmt.ends_with('\n');
                    let ir = if is_err {
                        IrExpr::EPrintLn { fmt: rust_fmt.trim_end_matches('\n').to_string(), args: ir_args }
                    } else if ends_newline {
                        IrExpr::PrintLn { fmt: rust_fmt.trim_end_matches('\n').to_string(), args: ir_args }
                    } else {
                        IrExpr::Print { fmt: rust_fmt, args: ir_args }
                    };
                    return Some(IrStatement::Expr(ir));
                }
                "puts" => {
                    let s_ir = Self::build_expr(args.first()?, sem, struct_fields, fn_param_types).ok()?;
                    return Some(IrStatement::Expr(IrExpr::PrintLn { fmt: "{}".to_string(), args: vec![s_ir] }));
                }
                "scanf" | "fscanf" => {
                    let vars: Vec<String> = args.iter().skip(1).filter_map(|a| {
                        if let CE::UnaryOp { op: ast_input::UnaryOp::AddrOf, operand } = a {
                            if let CE::Variable(v) = operand.as_ref() {
                                return Some(v.clone());
                            }
                        }
                        None
                    }).collect();
                    if !vars.is_empty() {
                        return Some(IrStatement::Scanf { vars });
                    }
                }
                "exit" => {
                    let code = args.first()
                        .and_then(|a| Self::build_expr(a, sem, struct_fields, fn_param_types).ok())
                        .unwrap_or(IrExpr::IntLit(0));
                    return Some(IrStatement::Expr(IrExpr::Exit(Box::new(code))));
                }
                _ => {}
            }
        }
        None
    }

    fn translate_printf_fmt<'a>(
        fmt_expr: &'a CExpression,
        value_args: &'a [CExpression],
    ) -> (String, &'a [CExpression]) {
        use ast_input::{CExpression as CE, CLiteral};
        if let CE::Literal(CLiteral::String(s)) = fmt_expr {
            let rust = Self::translate_format_specifiers(s);
            (rust, value_args)
        } else {
            ("{:?}".to_string(), value_args)
        }
    }

    fn translate_format_specifiers(s: &str) -> String {
        let mut result = String::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 1 < bytes.len() {
                i += 1;
                if bytes[i] == b'%' { result.push('%'); i += 1; continue; }
                while i < bytes.len() && b"-+ 0#".contains(&bytes[i]) { i += 1; }
                while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
                let mut precision: Option<String> = None;
                if i < bytes.len() && bytes[i] == b'.' {
                    i += 1;
                    let mut prec = String::new();
                    while i < bytes.len() && bytes[i].is_ascii_digit() { prec.push(bytes[i] as char); i += 1; }
                    precision = Some(prec);
                }
                while i < bytes.len() && b"hlLz".contains(&bytes[i]) { i += 1; }
                if i < bytes.len() {
                    let spec = bytes[i] as char;
                    i += 1;
                    let fmt = match spec {
                        'd' | 'i' | 'u' => "{}".to_string(),
                        'o' => "{:o}".to_string(),
                        'x' => "{:x}".to_string(),
                        'X' => "{:X}".to_string(),
                        'e' => "{:e}".to_string(),
                        'E' => "{:E}".to_string(),
                        'g' | 'G' => "{}".to_string(),
                        'f' | 'F' => {
                            if let Some(ref p) = precision { format!("{{:.{}}}", p) }
                            else { "{:.6}".to_string() }
                        }
                        'c' | 's' => "{}".to_string(),
                        'p' => "{:p}".to_string(),
                        other => format!("{{{}}}", other),
                    };
                    result.push_str(&fmt);
                }
            } else if bytes[i] == b'\\' && i + 1 < bytes.len() {
                match bytes[i + 1] {
                    b'n' => { result.push('\n'); i += 2; }
                    b't' => { result.push('\t'); i += 2; }
                    b'r' => { result.push('\r'); i += 2; }
                    b'\\' => { result.push('\\'); i += 2; }
                    b'"' => { result.push('"'); i += 2; }
                    _ => { result.push(bytes[i] as char); i += 1; }
                }
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        }
        result
    }

    // ── malloc rewriting ─────────────────────────────────────────────────────

    fn try_rewrite_alloc(
        expr: &CExpression,
        sem: &semantic::FunctionSemantics,
        struct_fields: &HashMap<String, Vec<String>>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> Option<IrExpr> {
        use ast_input::CExpression as CE;
        let (cast_type, inner) = match expr {
            CE::Cast { to_type, expr: inner } => (Some(to_type.clone()), inner.as_ref()),
            other => (None, other),
        };
        match inner {
            CE::Call { function, args } => match function.as_str() {
                "malloc" => {
                    let elem = cast_type.as_ref().map(|t| Self::pointer_element_type(t)).unwrap_or_else(|| "u8".to_string());
                    let size = args.first()?;
                    Some(Self::rewrite_malloc_expr(size, &elem, sem, struct_fields, fn_param_types))
                }
                "calloc" => {
                    let elem = cast_type.as_ref().map(|t| Self::pointer_element_type(t)).unwrap_or_else(|| "u8".to_string());
                    let count = Self::build_expr(args.first()?, sem, struct_fields, fn_param_types).ok()?;
                    Some(IrExpr::VecNew { elem_type: elem, len: Box::new(count) })
                }
                "realloc" => {
                    let elem = cast_type.as_ref().map(|t| Self::pointer_element_type(t)).unwrap_or_else(|| "u8".to_string());
                    let size = args.get(1)?;
                    Some(Self::rewrite_malloc_expr(size, &elem, sem, struct_fields, fn_param_types))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn rewrite_malloc_expr(
        size_expr: &CExpression,
        elem_type: &str,
        sem: &semantic::FunctionSemantics,
        struct_fields: &HashMap<String, Vec<String>>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> IrExpr {
        use ast_input::CExpression as CE;
        if let CE::Call { function, args } = size_expr {
            if function == "sizeof" {
                let is_struct = args.first()
                    .and_then(|a| if let CE::Variable(v) = a { Some(v.as_str()) } else { None })
                    .map(|t| struct_fields.contains_key(t))
                    .unwrap_or(false);
                return if is_struct {
                    IrExpr::BoxNew(Box::new(IrExpr::StructDefault(elem_type.to_string())))
                } else {
                    IrExpr::BoxNew(Box::new(IrExpr::IntLit(0)))
                };
            }
        }
        if let CE::BinaryOp { left, op: ast_input::BinOp::Mul, right } = size_expr {
            let (count, _) = if Self::is_sizeof(right) { (left.as_ref(), right.as_ref()) }
                             else { (right.as_ref(), left.as_ref()) };
            let count_ir = Self::build_expr(count, sem, struct_fields, fn_param_types).unwrap_or(IrExpr::IntLit(1));
            return IrExpr::VecNew { elem_type: elem_type.to_string(), len: Box::new(count_ir) };
        }
        let count_ir = Self::build_expr(size_expr, sem, struct_fields, fn_param_types).unwrap_or(IrExpr::IntLit(0));
        IrExpr::VecNew { elem_type: elem_type.to_string(), len: Box::new(count_ir) }
    }

    fn is_sizeof(expr: &CExpression) -> bool {
        matches!(expr, CExpression::Call { function, .. } if function == "sizeof")
    }

    fn pointer_element_type(ctype: &CType) -> String {
        match ctype {
            CType::Pointer(inner) => Self::fallback_type(inner),
            other => Self::fallback_type(other),
        }
    }

    // ── Expression builder ───────────────────────────────────────────────────

    fn build_expr(
        expr: &CExpression,
        sem: &semantic::FunctionSemantics,
        struct_fields: &HashMap<String, Vec<String>>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> anyhow::Result<IrExpr> {
        use ast_input::{CExpression as CE, CLiteral};
        Ok(match expr {
            CE::Literal(CLiteral::Int(n))    => IrExpr::IntLit(*n),
            CE::Literal(CLiteral::Float(f))  => IrExpr::FloatLit(*f),
            CE::Literal(CLiteral::String(s)) => IrExpr::StringLit(s.clone()),
            CE::Literal(CLiteral::Char(c))   => IrExpr::CharLit(*c),
            CE::Null                          => IrExpr::NullMut,
            CE::Variable(name)               => IrExpr::Var(name.clone()),

            CE::BinaryOp { left, op, right } => {
                let left_ir  = Self::build_expr(left,  sem, struct_fields, fn_param_types)?;
                let right_ir = Self::build_expr(right, sem, struct_fields, fn_param_types)?;
                let left_ty  = Self::infer_expr_type(&left_ir,  sem);
                let right_ty = Self::infer_expr_type(&right_ir, sem);
                let (left_ir, right_ir) = Self::coerce_numeric_pair(
                    left_ir, right_ir, left_ty.as_deref(), right_ty.as_deref()
                );
                IrExpr::BinOp {
                    left:  Box::new(left_ir),
                    op: *op,
                    right: Box::new(right_ir),
                }
            }

            CE::UnaryOp { op, operand } => IrExpr::UnaryOp {
                op: *op,
                operand: Box::new(Self::build_expr(operand, sem, struct_fields, fn_param_types)?),
            },

            CE::Call { function, args } => {
                if let Some(alloc) = Self::try_rewrite_alloc(expr, sem, struct_fields, fn_param_types) {
                    return Ok(alloc);
                }
                match function.as_str() {
                    "sizeof" => {
                        if let Some(CE::Variable(t)) = args.first() {
                            return Ok(IrExpr::Call {
                                function: format!("std::mem::size_of::<{}>", Self::c_name_to_rust(t)),
                                args: vec![],
                            });
                        }
                    }
                    "printf" | "fprintf" | "puts" => {
                        if let Some(IrStatement::Expr(e)) = Self::try_rewrite_io_stmt(expr, sem, struct_fields, fn_param_types) {
                            return Ok(e);
                        }
                    }
                    "exit" => {
                        let code = args.first()
                            .and_then(|a| Self::build_expr(a, sem, struct_fields, fn_param_types).ok())
                            .unwrap_or(IrExpr::IntLit(0));
                        return Ok(IrExpr::Exit(Box::new(code)));
                    }
                    _ => {}
                }
                let rust_fn = Self::map_stdlib_fn(function);
                let built_args: Vec<IrExpr> = args.iter()
                    .map(|a| Self::build_expr(a, sem, struct_fields, fn_param_types))
                    .collect::<anyhow::Result<_>>()?;

                let coerced_args = if let Some(param_types) = fn_param_types.get(function.as_str()) {
                    built_args.into_iter().enumerate().map(|(i, arg)| {
                        if let Some(param_type) = param_types.get(i) {
                            Self::coerce_arg(arg, param_type, sem)
                        } else { arg }
                    }).collect()
                } else { built_args };

                IrExpr::Call { function: rust_fn, args: coerced_args }
            }

            CE::ArrayAccess { array, index } => IrExpr::ArrayAccess {
                array: Box::new(Self::build_expr(array, sem, struct_fields, fn_param_types)?),
                index: Box::new(Self::build_index_expr(index, sem, struct_fields, fn_param_types)?),
            },

            CE::ArrayLiteral(elems) => IrExpr::ArrayLiteral(
                elems.iter().map(|e| Self::build_expr(e, sem, struct_fields, fn_param_types)).collect::<anyhow::Result<_>>()?
            ),

            CE::Cast { to_type, expr: inner } => {
                let rust_type = Self::fallback_type(to_type);
                IrExpr::Cast { to_type: rust_type, expr: Box::new(Self::build_expr(inner, sem, struct_fields, fn_param_types)?) }
            }

            CE::FieldAccess { object, field } => {
                let obj_ir = Self::build_expr(object, sem, struct_fields, fn_param_types)?;
                let needs_deref = match object.as_ref() {
                    CE::Variable(name) => sem.symbol_table.c_type(name)
                        .map(|t| matches!(t, CType::Pointer(_))).unwrap_or(false),
                    _ => false,
                };
                if needs_deref {
                    IrExpr::FieldAccess { object: Box::new(IrExpr::Deref(Box::new(obj_ir))), field: field.clone() }
                } else {
                    IrExpr::FieldAccess { object: Box::new(obj_ir), field: field.clone() }
                }
            }

            CE::StructLiteral { struct_type, fields } => {
                let real_names = struct_fields.get(struct_type);
                let resolved = fields.iter().enumerate()
                    .map(|(i, (fname, fexpr))| {
                        let real = real_names.and_then(|ns| ns.get(i)).cloned().unwrap_or_else(|| fname.clone());
                        Self::build_expr(fexpr, sem, struct_fields, fn_param_types).map(|e| (real, e))
                    })
                    .collect::<anyhow::Result<_>>()?;
                IrExpr::StructLiteral { struct_type: struct_type.clone(), fields: resolved }
            }
        })
    }

    fn build_index_expr(
        expr: &CExpression,
        sem: &semantic::FunctionSemantics,
        struct_fields: &HashMap<String, Vec<String>>,
        fn_param_types: &HashMap<String, Vec<String>>,
    ) -> anyhow::Result<IrExpr> {
        use ast_input::{CExpression as CE, CLiteral};
        if let CE::Literal(CLiteral::Int(_)) = expr {
            return Self::build_expr(expr, sem, struct_fields, fn_param_types);
        }
        Ok(IrExpr::AsUsize(Box::new(Self::build_expr(expr, sem, struct_fields, fn_param_types)?)))
    }

    // ── Type helpers ─────────────────────────────────────────────────────────

    fn map_stdlib_fn(name: &str) -> String {
        match name {
            "abs"   => "i32::abs".to_string(),
            "fabs"  => "f64::abs".to_string(),
            "sqrt"  => "f64::sqrt".to_string(),
            "pow"   => "f64::powi".to_string(),
            "floor" => "f64::floor".to_string(),
            "ceil"  => "f64::ceil".to_string(),
            "rand"  => "rand::random::<i32>".to_string(),
            "atoi"  => "str::parse::<i32>".to_string(),
            "atof"  => "str::parse::<f64>".to_string(),
            other   => other.to_string(),
        }
    }

    fn c_name_to_rust(name: &str) -> &str {
        match name {
            "int" => "i32", "char" => "u8", "float" => "f32",
            "double" => "f64", "long" => "i64", other => other,
        }
    }

    pub fn fallback_type(ctype: &CType) -> String {
        match ctype {
            CType::Void   => "()" .to_string(),
            CType::Int    => "i32".to_string(),
            CType::Char   => "u8" .to_string(),
            CType::UChar  => "u8" .to_string(),
            CType::Short  => "i16".to_string(),
            CType::Long   => "i64".to_string(),
            CType::UInt   => "u32".to_string(),
            CType::ULong  => "u64".to_string(),
            CType::Bool   => "bool".to_string(),
            CType::Float  => "f32".to_string(),
            CType::Double => "f64".to_string(),
            // char* → String (idiomatic Rust string)
            CType::Pointer(inner) if matches!(inner.as_ref(), CType::Char | CType::UChar) => {
                "String".to_string()
            }
            // void* → *mut u8 (unsafe raw)
            CType::Pointer(inner) if matches!(inner.as_ref(), CType::Void) => {
                "*mut u8".to_string()
            }
            CType::Pointer(inner) => format!("*mut {}", Self::fallback_type(inner)),
            CType::Array(inner, Some(n)) => format!("[{}; {}]", Self::fallback_type(inner), n),
            CType::Array(inner, None)    => format!("Vec<{}>",  Self::fallback_type(inner)),
            CType::Struct(name)          => name.clone(),
            CType::Unknown(s)            => s.clone(),
        }
    }

    fn coerce_to_type(expr: IrExpr, rust_type: &str) -> IrExpr {
        match (&expr, rust_type) {
            (IrExpr::IntLit(n), "f32") => IrExpr::FloatLit(*n as f64),
            (IrExpr::IntLit(n), "f64") => IrExpr::FloatLit(*n as f64),
            _ => expr,
        }
    }

    fn infer_expr_type(expr: &IrExpr, sem: &semantic::FunctionSemantics) -> Option<String> {
        match expr {
            IrExpr::FloatLit(_)  => Some("f64".to_string()),
            IrExpr::IntLit(_)    => Some("i64".to_string()),
            IrExpr::Var(name)    => sem.rust_type_map.get(name).map(|r| r.rust_repr.clone()),
            IrExpr::FieldAccess { object, field } => {
                if let IrExpr::Var(obj_name) = object.as_ref() {
                    let _ = (obj_name, field);
                }
                None
            }
            IrExpr::Cast { to_type, .. } => Some(to_type.clone()),
            _ => None,
        }
    }

    fn coerce_numeric_pair(
        left: IrExpr, right: IrExpr,
        left_ty: Option<&str>, right_ty: Option<&str>,
    ) -> (IrExpr, IrExpr) {
        let left_is_float  = matches!(left_ty,  Some("f32") | Some("f64"))
            || matches!(&left,  IrExpr::FloatLit(_));
        let right_is_float = matches!(right_ty, Some("f32") | Some("f64"))
            || matches!(&right, IrExpr::FloatLit(_));
        let left_is_int  = !left_is_float
            && (matches!(left_ty,  Some("i32") | Some("i64") | Some("usize"))
                || matches!(&left,  IrExpr::IntLit(_)));
        let right_is_int = !right_is_float
            && (matches!(right_ty, Some("i32") | Some("i64") | Some("usize"))
                || matches!(&right, IrExpr::IntLit(_)));

        if left_is_float && right_is_int {
            let float_ty = left_ty.filter(|t| t.starts_with('f')).unwrap_or("f32");
            let right = match right {
                IrExpr::IntLit(n) => IrExpr::FloatLit(n as f64),
                other => IrExpr::Cast { to_type: float_ty.to_string(), expr: Box::new(other) },
            };
            return (left, right);
        }
        if right_is_float && left_is_int {
            let float_ty = right_ty.filter(|t| t.starts_with('f')).unwrap_or("f32");
            let left = match left {
                IrExpr::IntLit(n) => IrExpr::FloatLit(n as f64),
                other => IrExpr::Cast { to_type: float_ty.to_string(), expr: Box::new(other) },
            };
            return (left, right);
        }
        (left, right)
    }

    // ── IO usage detection ───────────────────────────────────────────────────
    /// Returns true if any function uses `Print` (i.e. needs io::stdout().flush())
    fn ir_uses_print_flush(functions: &[IrFunction]) -> bool {
        functions.iter().any(|f| Self::stmts_use_print(&f.body))
    }

    fn stmts_use_print(stmts: &[IrStatement]) -> bool {
        stmts.iter().any(|s| match s {
            IrStatement::Expr(IrExpr::Print { .. })
            | IrStatement::Expr(IrExpr::EPrintLn { .. }) => true,
            IrStatement::If { then_block, else_block, .. } =>
                Self::stmts_use_print(then_block)
                || else_block.as_ref().map(|e| Self::stmts_use_print(e)).unwrap_or(false),
            IrStatement::While { body, .. }
            | IrStatement::Loop { body, .. } => Self::stmts_use_print(body),
            IrStatement::While2 { body, .. } => Self::stmts_use_print(body),
            _ => false,
        })
    }

    /// Returns true if any function uses raw pointer types (needs `use std::ptr`)
    fn ir_uses_raw_ptr(functions: &[IrFunction]) -> bool {
        functions.iter().any(|f| {
            f.parameters.iter().any(|p| p.rust_type.contains("*mut") || p.rust_type.contains("*const"))
            || f.rust_return_type.contains("*mut")
            || f.rust_return_type.contains("*const")
        })
    }

    fn coerce_arg(arg: IrExpr, param_type: &str, sem: &semantic::FunctionSemantics) -> IrExpr {
        if param_type.starts_with("&mut [") {
            match &arg {
                IrExpr::Var(name) => {
                    // If this var is already a &mut [T] in the current function
                    // (it IS the mutable slice param), do NOT wrap in another &mut —
                    // that causes E0596. Rust handles implicit reborrow automatically.
                    let already_mut_slice = sem.rust_type_map
                        .get(name)
                        .map(|rt| rt.rust_repr.starts_with("&mut ["))
                        .unwrap_or(false);
                    if already_mut_slice { return arg; }
                    return IrExpr::RefMut(Box::new(arg));
                }
                IrExpr::ArrayAccess { .. } => return arg,
                _ => {}
            }
        }
        // Handle `&mut T` (non-slice) param: if we're passing `&x`, upgrade to `&mut x`
        if param_type.starts_with("&mut ") && !param_type.starts_with("&mut [") {
            match arg {
                IrExpr::Ref(inner) => return IrExpr::RefMut(inner),
                IrExpr::Var(ref _name) => return IrExpr::RefMut(Box::new(arg)),
                other => return other,
            }
        }
        if param_type.starts_with("Vec<") {
            if let IrExpr::Var(_) = &arg {
                return IrExpr::Call { function: "__to_vec__".to_string(), args: vec![arg] };
            }
        }
        match &arg {
            IrExpr::IntLit(n) if param_type == "f32" => return IrExpr::FloatLit(*n as f64),
            IrExpr::IntLit(n) if param_type == "f64" => return IrExpr::FloatLit(*n as f64),
            IrExpr::Var(_) if param_type == "f32" =>
                return IrExpr::Cast { to_type: "f32".to_string(), expr: Box::new(arg) },
            IrExpr::Var(_) if param_type == "f64" =>
                return IrExpr::Cast { to_type: "f64".to_string(), expr: Box::new(arg) },
            _ => {}
        }
        arg
    }
}
