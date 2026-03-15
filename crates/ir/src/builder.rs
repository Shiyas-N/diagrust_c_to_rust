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

        // Build a map from function name → upgraded Rust return type.
        // This lets VarDecl builder upgrade `*mut T` variable types when they
        // receive the result of a function that now returns `&mut T`.
        let fn_return_types: HashMap<String, String> = program.functions.iter().map(|f| {
            let raw_ret = Self::fallback_type(&f.return_type);
            let upgraded = if raw_ret.starts_with("*mut ") {
                let inner = &raw_ret["*mut ".len()..];
                let has_ptr_param = f.parameters.iter().any(|p| {
                    let pt = Self::fallback_type(&p.param_type);
                    pt == format!("*mut {}", inner) || pt == format!("&mut {}", inner)
                });
                if has_ptr_param { format!("&mut {}", inner) } else { raw_ret }
            } else { raw_ret };
            (f.name.clone(), upgraded)
        }).collect();

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
                func, sem, &mut required_uses, &struct_fields, &global_names,
                &fn_param_types, &fn_return_types,
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

    /// Post-process body statements: when a `let var: *mut T` is initialised by
    /// a call to a function whose return type has been upgraded to `&mut T`,
    /// upgrade the variable type to `&mut T` and mark it mutable.
    /// Also mark variables `mut` when they appear as `&mut var` call arguments
    /// so that Rust accepts taking a mutable reference from them.
    fn upgrade_ptr_return_bindings(
        stmts: Vec<IrStatement>,
        fn_return_types: &HashMap<String, String>,
    ) -> Vec<IrStatement> {
        // Collect names of variables that are passed as &mut somewhere in the body,
        // so we can mark their declarations mutable.
        let mut mut_ref_vars: HashSet<String> = HashSet::new();
        Self::collect_mut_ref_vars(&stmts, &mut mut_ref_vars);

        stmts.into_iter().map(|stmt| {
            match stmt {
                IrStatement::LetBinding { name, rust_type, init, is_mutable } => {
                    // Upgrade `*mut T` → `&mut T` when the init is a call to a
                    // function that now returns `&mut T`.
                    let upgraded_type = if rust_type.starts_with("*mut ") {
                        if let Some(Some(fn_name)) = init.as_ref().map(|e| match e {
                            IrExpr::Call { function, .. } => Some(function.as_str()),
                            _ => None,
                        }) {
                            if let Some(ret) = fn_return_types.get(fn_name) {
                                if ret.starts_with("&mut ") { ret.clone() } else { rust_type }
                            } else { rust_type }
                        } else { rust_type }
                    } else { rust_type };

                    // Also mark the binding mutable if the variable is used as &mut arg
                    let needs_mut = is_mutable || mut_ref_vars.contains(&name);
                    IrStatement::LetBinding { name, rust_type: upgraded_type, init, is_mutable: needs_mut }
                }
                other => other,
            }
        }).collect()
    }

    /// Walk all expressions in `stmts` and collect variable names that appear
    /// directly as `IrExpr::RefMut(Var(name))` — i.e., passed as `&mut name`.

    // ── Malloc / Box patching pass ────────────────────────────────────────────
    //
    // C pattern:
    //   struct Foo *p;
    //   p = (struct Foo*) malloc(sizeof(struct Foo));
    //   if (p == NULL) { ... }
    //   p->field = value;
    //   free(p);
    //
    // Generated IR before this pass:
    //   LetBinding { name:"p", rust_type:"*mut Foo", init:None }
    //   Assign     { target:"p", value:BoxNew(StructDefault("Foo")) }
    //   If { condition: BinOp(Var("p"), Equal, NullMut), ... }
    //   FieldAssign { object:"p", field:"field", value:... }
    //   Drop("p")
    //
    // After this pass:
    //   LetBinding { name:"p", rust_type:"Box<Foo>", init:Some(BoxNew(...)), is_mutable:true }
    //   [Assign removed — merged into LetBinding]
    //   If { condition: BoolLit(false), ... }       ← Box::new never fails
    //   FieldAssign { object:"p", field:"field", ... }  ← Box auto-derefs
    //   Drop("p")
    //
    fn patch_malloc_vars(stmts: Vec<IrStatement>) -> Vec<IrStatement> {
        // Pass 1: collect ALL heap-allocated vars (BoxNew or VecNew assignments).
        let mut box_vars: std::collections::HashSet<String> = Default::default();
        for stmt in &stmts {
            match stmt {
                IrStatement::Assign { target, value } if Self::expr_is_heap_alloc(value) => {
                    box_vars.insert(target.clone());
                }
                // Also catch inline let: `let arr: *mut T = VecNew { .. }`
                IrStatement::LetBinding { name, init: Some(value), .. }
                    if Self::expr_is_heap_alloc(value) => {
                    box_vars.insert(name.clone());
                }
                _ => {}
            }
        }
        if box_vars.is_empty() { return stmts; }

        // Pass 2: rewrite.
        let mut out: Vec<IrStatement> = Vec::with_capacity(stmts.len());
        for stmt in stmts {
            match stmt {
                // LetBinding { *mut T, None }  →  upgrade type, init filled later
                // LetBinding { *mut T, VecNew/BoxNew }  →  upgrade type inline
                IrStatement::LetBinding { ref name, ref rust_type, ref init, ref is_mutable }
                    if box_vars.contains(name) && rust_type.starts_with("*mut ") =>
                {
                    let inner = rust_type["*mut ".len()..].to_string();
                    let new_type = Self::heap_alloc_type(&inner, init.as_ref());
                    let cloned_init = init.clone();
                    let cloned_mut = *is_mutable;
                    let cloned_name = name.clone();
                    out.push(IrStatement::LetBinding {
                        name: cloned_name,
                        rust_type: new_type,
                        init: cloned_init,
                        is_mutable: cloned_mut || true,
                    });
                }

                // Assign { target, BoxNew/VecNew }  →  inject init into the LetBinding
                IrStatement::Assign { ref target, ref value }
                    if box_vars.contains(target) && Self::expr_is_heap_alloc(value) =>
                {
                    let mut found = false;
                    for existing in out.iter_mut().rev() {
                        if let IrStatement::LetBinding { name, rust_type, init, .. } = existing {
                            if name == target && init.is_none() {
                                // Only upgrade the type if it is still a raw pointer.
                                // If it was already upgraded to Box<T>/Vec<T>/String by
                                // the LetBinding arm, do NOT run heap_alloc_type again —
                                // that would produce Box<Box<T>>.
                                if rust_type.starts_with("*mut ") {
                                    let inner = rust_type["*mut ".len()..].to_string();
                                    *rust_type = Self::heap_alloc_type(&inner, Some(value));
                                }
                                // else: type already correct, just inject the init
                                *init = Some(value.clone());
                                found = true;
                                break;
                            }
                        }
                    }
                    if !found { out.push(stmt); }
                }

                // If { BinOp(Var(box_var), Equal/NotEqual, Null/NullMut) }
                // → replace condition with BoolLit(false) / BoolLit(true)
                IrStatement::If { condition, then_block, else_block } => {
                    let new_cond = Self::rewrite_null_check_condition(condition, &box_vars);
                    // Recurse into branches
                    let new_then = Self::patch_malloc_vars(then_block);
                    let new_else = else_block.map(Self::patch_malloc_vars);
                    out.push(IrStatement::If { condition: new_cond, then_block: new_then, else_block: new_else });
                }

                // Recurse into loops / nested blocks
                IrStatement::While { condition, body } => {
                    let new_body = Self::patch_malloc_vars(body);
                    out.push(IrStatement::While { condition, body: new_body });
                }
                IrStatement::While2 { init, condition, increment, body } => {
                    let new_body = Self::patch_malloc_vars(body);
                    out.push(IrStatement::While2 { init, condition, increment, body: new_body });
                }
                IrStatement::Loop { body, condition } => {
                    let new_body = Self::patch_malloc_vars(body);
                    out.push(IrStatement::Loop { body: new_body, condition });
                }

                other => {
                    // Rewrite IrExpr::FieldAccess { Deref(Var(box_var)), field }
                    // → IrExpr::FieldAccess { Var(box_var), field }
                    // Box/Vec auto-deref in Rust; (*s).id → s.id
                    let rewritten = Self::simplify_deref_field_accesses(other, &box_vars);
                    out.push(rewritten);
                }
            }
        }
        out
    }

    /// Walk an IrStatement and simplify `(*box_var).field` → `box_var.field`.
    fn simplify_deref_field_accesses(stmt: IrStatement, box_vars: &std::collections::HashSet<String>) -> IrStatement {
        match stmt {
            IrStatement::Expr(e) =>
                IrStatement::Expr(Self::simplify_expr_derefs(e, box_vars)),
            IrStatement::Return(Some(e)) =>
                IrStatement::Return(Some(Self::simplify_expr_derefs(e, box_vars))),
            IrStatement::LetBinding { name, rust_type, init, is_mutable } =>
                IrStatement::LetBinding { name, rust_type, init: init.map(|e| Self::simplify_expr_derefs(e, box_vars)), is_mutable },
            IrStatement::Assign { target, value } =>
                IrStatement::Assign { target, value: Self::simplify_expr_derefs(value, box_vars) },
            IrStatement::FieldAssign { object, field, value } =>
                IrStatement::FieldAssign { object, field, value: Self::simplify_expr_derefs(value, box_vars) },
            other => other,
        }
    }

    fn simplify_expr_derefs(expr: IrExpr, box_vars: &std::collections::HashSet<String>) -> IrExpr {
        match expr {
            // (*box_var).field  →  box_var.field
            IrExpr::FieldAccess { object, field } => {
                let simplified_obj = match *object {
                    IrExpr::Deref(inner) => {
                        if matches!(*inner, IrExpr::Var(ref n) if box_vars.contains(n)) {
                            *inner  // drop the Deref
                        } else {
                            IrExpr::Deref(inner)
                        }
                    }
                    other => Self::simplify_expr_derefs(other, box_vars),
                };
                IrExpr::FieldAccess { object: Box::new(simplified_obj), field }
            }
            // Recurse into other expression containers
            IrExpr::BinOp { left, op, right } => IrExpr::BinOp {
                left:  Box::new(Self::simplify_expr_derefs(*left,  box_vars)),
                op,
                right: Box::new(Self::simplify_expr_derefs(*right, box_vars)),
            },
            IrExpr::UnaryOp { op, operand } => IrExpr::UnaryOp {
                op, operand: Box::new(Self::simplify_expr_derefs(*operand, box_vars)),
            },
            IrExpr::Call { function, args } => IrExpr::Call {
                function,
                args: args.into_iter().map(|a| Self::simplify_expr_derefs(a, box_vars)).collect(),
            },
            other => other,
        }
    }

    fn expr_is_heap_alloc(expr: &IrExpr) -> bool {
        matches!(expr, IrExpr::BoxNew(_) | IrExpr::VecNew { .. })
    }

    /// Given the inner type (stripped of `*mut`) and an optional init expression,
    /// decide the idiomatic Rust type for a heap-allocated variable.
    fn heap_alloc_type(inner: &str, init: Option<&IrExpr>) -> String {
        match init {
            Some(IrExpr::BoxNew(_)) => format!("Box<{}>", inner),
            Some(IrExpr::VecNew { elem_type, .. }) => {
                if elem_type == "u8" || elem_type == "char" || inner == "u8" || inner == "String" {
                    "String".to_string()
                } else {
                    format!("Vec<{}>", elem_type)
                }
            }
            // No init yet — decide by inner type: if struct → Box, else Vec
            None => {
                if inner.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    format!("Box<{}>", inner)
                } else if inner == "u8" || inner == "String" {
                    "String".to_string()
                } else {
                    format!("Vec<{}>", inner)
                }
            }
            _ => format!("*mut {}", inner),
        }
    }
    // keep the old name as an alias for clarity
    fn expr_is_box_alloc(expr: &IrExpr) -> bool {
        matches!(expr, IrExpr::BoxNew(_))
    }

    /// Replace `box_var == NULL` with `false` and `box_var != NULL` with `true`.
    fn is_null_expr(e: &IrExpr) -> bool {
        match e {
            IrExpr::Null | IrExpr::NullMut => true,
            IrExpr::IntLit(0) => true,
            // (void*)0, (int*)0, (T*)0 — any pointer cast of zero
            IrExpr::Cast { to_type, expr } if to_type.starts_with('*') =>
                matches!(expr.as_ref(), IrExpr::IntLit(0)),
            _ => false,
        }
    }

    fn rewrite_null_check_condition(cond: IrExpr, box_vars: &std::collections::HashSet<String>) -> IrExpr {
        // Extract the operator and operand names before any moves.
        let rewrite = if let IrExpr::BinOp { ref left, ref op, ref right } = cond {
            let left_is_heap  = matches!(left.as_ref(),  IrExpr::Var(n) if box_vars.contains(n));
            let right_is_heap = matches!(right.as_ref(), IrExpr::Var(n) if box_vars.contains(n));
            let left_is_null  = Self::is_null_expr(left.as_ref());
            let right_is_null = Self::is_null_expr(right.as_ref());
            if (left_is_heap && right_is_null) || (right_is_heap && left_is_null) {
                match op {
                    ast_input::BinOp::Equal    => Some(IrExpr::BoolLit(false)),
                    ast_input::BinOp::NotEqual => Some(IrExpr::BoolLit(true)),
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        rewrite.unwrap_or(cond)
    }

    fn collect_mut_ref_vars(stmts: &[IrStatement], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                IrStatement::Expr(e) | IrStatement::Return(Some(e)) => Self::scan_expr_for_mut_ref(e, out),
                IrStatement::LetBinding { init: Some(e), .. } => Self::scan_expr_for_mut_ref(e, out),
                IrStatement::Assign { value, .. } => Self::scan_expr_for_mut_ref(value, out),
                IrStatement::If { condition, then_block, else_block } => {
                    Self::scan_expr_for_mut_ref(condition, out);
                    Self::collect_mut_ref_vars(then_block, out);
                    if let Some(eb) = else_block { Self::collect_mut_ref_vars(eb, out); }
                }
                IrStatement::While { condition, body } => {
                    Self::scan_expr_for_mut_ref(condition, out);
                    Self::collect_mut_ref_vars(body, out);
                }
                IrStatement::While2 { body, .. } => Self::collect_mut_ref_vars(body, out),
                IrStatement::Loop { body, .. } => Self::collect_mut_ref_vars(body, out),
                _ => {}
            }
        }
    }

    fn scan_expr_for_mut_ref(expr: &IrExpr, out: &mut HashSet<String>) {
        match expr {
            IrExpr::RefMut(inner) => {
                if let IrExpr::Var(name) = inner.as_ref() { out.insert(name.clone()); }
                Self::scan_expr_for_mut_ref(inner, out);
            }
            IrExpr::Call { args, .. } => { for a in args { Self::scan_expr_for_mut_ref(a, out); } }
            IrExpr::BinOp { left, right, .. } => {
                Self::scan_expr_for_mut_ref(left, out);
                Self::scan_expr_for_mut_ref(right, out);
            }
            IrExpr::UnaryOp { operand, .. } => Self::scan_expr_for_mut_ref(operand, out),
            _ => {}
        }
    }

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
        fn_return_types: &HashMap<String, String>,
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
        // Post-process: upgrade `let m: *mut T = fn_call()` to `let mut m: &mut T`
        // when fn_call's return type has been upgraded from *mut T to &mut T.
        let raw_body = Self::upgrade_ptr_return_bindings(raw_body, fn_return_types);
        // Post-process: merge `let s: *mut T;` + `s = Box::new(T::default())` into
        // `let mut s: Box<T> = Box::new(T::default())`, and rewrite NULL checks.
        let raw_body = Self::patch_malloc_vars(raw_body);
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
        stmts.into_iter().flat_map(|s| {
            match s {
                // return 0 / bare return → drop entirely (main returns ())
                IrStatement::Return(None)
                | IrStatement::Return(Some(IrExpr::IntLit(0))) => vec![],
                // return N (N != 0) → std::process::exit(N)
                IrStatement::Return(Some(IrExpr::IntLit(n))) =>
                    vec![IrStatement::Expr(IrExpr::Exit(Box::new(IrExpr::IntLit(n))))],
                // return expr → std::process::exit(expr as i32)
                IrStatement::Return(Some(e)) =>
                    vec![IrStatement::Expr(IrExpr::Exit(Box::new(e)))],
                // Recurse into If/loop bodies
                IrStatement::If { condition, then_block, else_block } =>
                    vec![IrStatement::If {
                        condition,
                        then_block: Self::strip_main_return(then_block),
                        else_block: else_block.map(Self::strip_main_return),
                    }],
                IrStatement::While { condition, body } =>
                    vec![IrStatement::While { condition, body: Self::strip_main_return(body) }],
                IrStatement::While2 { init, condition, increment, body } =>
                    vec![IrStatement::While2 { init, condition, increment, body: Self::strip_main_return(body) }],
                IrStatement::Loop { body, condition } =>
                    vec![IrStatement::Loop { body: Self::strip_main_return(body), condition }],
                other => vec![other],
            }
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
                // ── strcpy(dst, src) → dst = src.clone() ─────────────
                "strcpy" | "strncpy" => {
                    if args.len() >= 2 {
                        if let CE::Variable(dst) = &args[0] {
                            let src = Self::build_expr(&args[1], sem, struct_fields, fn_param_types).ok()?;
                            return Some(IrStatement::Assign {
                                target: dst.clone(),
                                value:  IrExpr::Clone(Box::new(src)),
                            });
                        }
                    }
                }
                // ── strcat(dst, src) → dst.push_str(src) ─────────────
                "strcat" | "strncat" => {
                    if args.len() >= 2 {
                        if let CE::Variable(dst) = &args[0] {
                            let src = Self::build_expr(&args[1], sem, struct_fields, fn_param_types).ok()?;
                            return Some(IrStatement::Expr(IrExpr::Call {
                                function: "__strcat_stmt__".to_string(),
                                args: vec![IrExpr::Var(dst.clone()), src],
                            }));
                        }
                    }
                }
                // ── sprintf(buf, fmt, ...) as statement ──────────────
                "sprintf" | "snprintf" => {
                    let fmt_idx = if function == "snprintf" { 2 } else { 1 };
                    if args.len() > fmt_idx {
                        if let CE::Variable(dst) = &args[0] {
                            let fmt_arg = &args[fmt_idx];
                            let rest = &args[fmt_idx + 1..];
                            let (rust_fmt, rest_args) = Self::translate_printf_fmt(fmt_arg, rest);
                            let ir_args: Vec<IrExpr> = rest_args.iter()
                                .filter_map(|a| Self::build_expr(a, sem, struct_fields, fn_param_types).ok())
                                .collect();
                            let rhs = if ir_args.is_empty() {
                                IrExpr::StringLit(rust_fmt)
                            } else {
                                IrExpr::Call {
                                    function: format!("__format__:{}", rust_fmt),
                                    args: ir_args,
                                }
                            };
                            return Some(IrStatement::Assign {
                                target: dst.clone(),
                                value: rhs,
                            });
                        }
                    }
                }
                "scanf" | "fscanf" => {
                    let skip = if function == "fscanf" { 2 } else { 1 };
                    let mut vars: Vec<String>  = Vec::new();
                    let mut targets: Vec<IrExpr> = Vec::new();
                    for a in args.iter().skip(skip) {
                        match a {
                            // &var  →  simple variable target
                            CE::UnaryOp { op: ast_input::UnaryOp::AddrOf, operand } => {
                                match operand.as_ref() {
                                    CE::Variable(v) => {
                                        vars.push(v.clone());
                                        targets.push(IrExpr::Var(v.clone()));
                                    }
                                    // &arr[i]  →  index target
                                    CE::ArrayAccess { array, index } => {
                                        if let CE::Variable(arr_name) = array.as_ref() {
                                            vars.push(format!("{}[i]", arr_name));
                                            let idx_ir = Self::build_expr(index, sem, struct_fields, fn_param_types)
                                                .unwrap_or(IrExpr::IntLit(0));
                                            targets.push(IrExpr::ArrayAccess {
                                                array: Box::new(IrExpr::Var(arr_name.clone())),
                                                index: Box::new(IrExpr::AsUsize(Box::new(idx_ir))),
                                            });
                                        }
                                    }
                                    // &(*ptr).field  or other complex
                                    other => {
                                        if let Ok(expr) = Self::build_expr(other, sem, struct_fields, fn_param_types) {
                                            vars.push(format!("{:?}", other).chars().take(16).collect());
                                            targets.push(expr);
                                        }
                                    }
                                }
                            }
                            // bare var (some C code passes without &)
                            CE::Variable(v) => {
                                vars.push(v.clone());
                                targets.push(IrExpr::Var(v.clone()));
                            }
                            _ => {}
                        }
                    }
                    if !targets.is_empty() {
                        return Some(IrStatement::Scanf { vars, targets });
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
                // Clang emits type spellings like "struct Student" or just "Student".
                // Strip the "struct "/"union " prefix before looking up in struct_fields.
                let is_struct = args.first()
                    .and_then(|a| if let CE::Variable(v) = a { Some(v.as_str()) } else { None })
                    .map(|t| {
                        let bare = t.trim_start_matches("struct ")
                                    .trim_start_matches("union ")
                                    .trim();
                        struct_fields.contains_key(bare) || struct_fields.contains_key(t)
                    })
                    .unwrap_or(false);
                // Also use the bare elem_type (may already have "struct " stripped by
                // pointer_element_type, but double-check).
                let bare_elem = elem_type.trim_start_matches("struct ")
                                          .trim_start_matches("union ")
                                          .trim();
                return if is_struct {
                    IrExpr::BoxNew(Box::new(IrExpr::StructDefault(bare_elem.to_string())))
                } else {
                    IrExpr::BoxNew(Box::new(IrExpr::StructDefault(bare_elem.to_string())))
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
        let raw = match ctype {
            CType::Pointer(inner) => Self::fallback_type(inner),
            other => Self::fallback_type(other),
        };
        // Clang sometimes includes "struct "/"union " in type spellings; strip it
        // so struct names are bare identifiers that match struct_fields keys.
        raw.trim_start_matches("struct ")
           .trim_start_matches("union ")
           .trim()
           .to_string()
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
                            // `t` may be:
                            //   (a) a C type name  → sizeof(int), sizeof(MyStruct)
                            //   (b) a variable name → sizeof(x)     (size of its type)
                            //   (c) a numeric / char literal text that the token
                            //       extractor captured as a Variable → sizeof(4), sizeof('a')
                            //
                            // For (c) and (b-unknown) we fall back to i32 / the type
                            // the semantic model knows for that name.
                            let rust_ty = Self::sizeof_arg_to_rust_type(t, sem);
                            return Ok(IrExpr::Call {
                                function: format!("std::mem::size_of::<{}>", rust_ty),
                                args: vec![],
                            });
                        }
                        // sizeof applied to a non-Variable child — just emit size_of::<i32>
                        // as a safe default.
                        return Ok(IrExpr::Call {
                            function: "std::mem::size_of::<i32>".to_string(),
                            args: vec![],
                        });
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

                    // ── String length: strlen(s) → s.len() ──────────────────
                    "strlen" => {
                        if let Some(s) = args.first() {
                            let obj = Self::build_expr(s, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(obj),
                                field: "len()".to_string(),
                            });
                        }
                    }

                    // ── String compare: strcmp(a,b) → a.cmp(&b) as i32 ──────
                    "strcmp" | "strcasecmp" => {
                        if args.len() >= 2 {
                            let a = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let b = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strcmp__".to_string(),
                                args: vec![a, b],
                            });
                        }
                    }

                    // ── String copy: strcpy(dst, src) → dst = src.clone() ────
                    "strcpy" | "strncpy" => {
                        if args.len() >= 2 {
                            let src = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Clone(Box::new(src)));
                        }
                    }

                    // ── String concat: strcat(dst, src) → dst.push_str(&src) ─
                    "strcat" | "strncat" => {
                        if args.len() >= 2 {
                            let dst = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let src = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strcat__".to_string(),
                                args: vec![dst, src],
                            });
                        }
                    }

                    // ── Char classification / conversion ─────────────────────
                    "isdigit" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "is_ascii_digit()".to_string(),
                            });
                        }
                    }
                    "isalpha" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "is_ascii_alphabetic()".to_string(),
                            });
                        }
                    }
                    "isalnum" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "is_ascii_alphanumeric()".to_string(),
                            });
                        }
                    }
                    "isupper" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "is_ascii_uppercase()".to_string(),
                            });
                        }
                    }
                    "islower" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "is_ascii_lowercase()".to_string(),
                            });
                        }
                    }
                    "isspace" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "is_ascii_whitespace()".to_string(),
                            });
                        }
                    }
                    "toupper" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "to_ascii_uppercase()".to_string(),
                            });
                        }
                    }
                    "tolower" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::FieldAccess {
                                object: Box::new(IrExpr::Cast { to_type: "u8".to_string(), expr: Box::new(obj) }),
                                field: "to_ascii_lowercase()".to_string(),
                            });
                        }
                    }

                    // ── Memory: memset(arr, val, n) → arr.fill(val) ──────────
                    "memset" => {
                        if args.len() >= 2 {
                            let arr = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let val = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__memset__".to_string(),
                                args: vec![arr, val],
                            });
                        }
                    }

                    // ── Memory: memcpy(dst, src, n) → dst.copy_from_slice(src) ─
                    "memcpy" | "memmove" => {
                        if args.len() >= 2 {
                            let dst = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let src = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__memcpy__".to_string(),
                                args: vec![dst, src],
                            });
                        }
                    }

                    // ── Memory: memcmp(a, b, n) → a == b ────────────────────
                    "memcmp" => {
                        if args.len() >= 2 {
                            let a = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let b = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::BinOp {
                                left: Box::new(a),
                                op: ast_input::BinOp::Equal,
                                right: Box::new(b),
                            });
                        }
                    }

                    // ── sprintf / snprintf: format to string ─────────────
                    "sprintf" | "snprintf" => {
                        // sprintf(buf, fmt, args...) → buf = format!(fmt, args...)
                        // We emit this as a special __sprintf__ sentinel handled in codegen
                        let fmt_idx = if function == "snprintf" { 2 } else { 1 };
                        if args.len() > fmt_idx {
                            let buf = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let fmt_arg = &args[fmt_idx];
                            let rest = &args[fmt_idx + 1..];
                            let (rust_fmt, rest_args) = Self::translate_printf_fmt(fmt_arg, rest);
                            let mut all_args = vec![buf];
                            all_args.extend(rest_args.iter()
                                .map(|a| Self::build_expr(a, sem, struct_fields, fn_param_types))
                                .collect::<anyhow::Result<Vec<_>>>()?);
                            return Ok(IrExpr::Call {
                                function: format!("__sprintf__:{}", rust_fmt),
                                args: all_args,
                            });
                        }
                    }

                    // ── strstr(haystack, needle) → haystack.find(needle) ────
                    "strstr" => {
                        if args.len() >= 2 {
                            let haystack = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let needle   = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strstr__".to_string(),
                                args: vec![haystack, needle],
                            });
                        }
                    }

                    // ── strchr(s, c) / strrchr(s, c) ────────────────────────
                    "strchr" => {
                        if args.len() >= 2 {
                            let s = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let c = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strchr__".to_string(),
                                args: vec![s, c],
                            });
                        }
                    }
                    "strrchr" => {
                        if args.len() >= 2 {
                            let s = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let c = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strrchr__".to_string(),
                                args: vec![s, c],
                            });
                        }
                    }

                    // ── strtol(s, endptr, base) → i64::from_str_radix ───────
                    "strtol" | "strtoul" | "strtoll" | "strtoull" => {
                        if args.len() >= 1 {
                            let s    = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let base = if args.len() >= 3 {
                                Self::build_expr(&args[2], sem, struct_fields, fn_param_types)?
                            } else { IrExpr::IntLit(10) };
                            return Ok(IrExpr::Call {
                                function: "__strtol__".to_string(),
                                args: vec![s, base],
                            });
                        }
                    }

                    // ── strtof / strtod ──────────────────────────────────────
                    "strtof" => {
                        if let Some(s) = args.first() {
                            let obj = Self::build_expr(s, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strtof__".to_string(),
                                args: vec![obj],
                            });
                        }
                    }
                    "strtod" => {
                        if let Some(s) = args.first() {
                            let obj = Self::build_expr(s, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strtod__".to_string(),
                                args: vec![obj],
                            });
                        }
                    }

                    // ── strdup(s) → s.clone() ────────────────────────────────
                    "strdup" | "_strdup" => {
                        if let Some(s) = args.first() {
                            let obj = Self::build_expr(s, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Clone(Box::new(obj)));
                        }
                    }

                    // ── strncmp(a, b, n) → a[..n].cmp(&b[..n]) as i32 ───────
                    "strncmp" => {
                        if args.len() >= 3 {
                            let a = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let b = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            let n = Self::build_expr(&args[2], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strncmp__".to_string(),
                                args: vec![a, b, n],
                            });
                        }
                    }

                    // ── strnlen(s, max) → s.len().min(max) ──────────────────
                    "strnlen" => {
                        if args.len() >= 2 {
                            let s   = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let max = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strnlen__".to_string(),
                                args: vec![s, max],
                            });
                        }
                    }

                    // ── strtok(s, delim) → split-based iterator ──────────────
                    "strtok" => {
                        if args.len() >= 2 {
                            let s     = Self::build_expr(&args[0], sem, struct_fields, fn_param_types)?;
                            let delim = Self::build_expr(&args[1], sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "__strtok__".to_string(),
                                args: vec![s, delim],
                            });
                        }
                    }

                    // ── String-level toupper/tolower ─────────────────────────
                    // (character-level versions are handled above as FieldAccess)
                    // These handle the case where a whole String is passed.

                    // ── fgets(buf, n, stdin) → read_line ─────────────────────
                    "fgets" => {
                        if let Some(buf) = args.first() {
                            if let CE::Variable(name) = buf {
                                return Ok(IrExpr::ReadLine { var: name.clone() });
                            }
                        }
                    }

                    // ── getchar() → read single byte from stdin ───────────────
                    "getchar" => {
                        return Ok(IrExpr::Call {
                            function: "__getchar__".to_string(),
                            args: vec![],
                        });
                    }

                    // ── putchar(c) ────────────────────────────────────────────
                    "putchar" => {
                        if let Some(c) = args.first() {
                            let obj = Self::build_expr(c, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Print {
                                fmt: "{}
".to_string(),
                                args: vec![IrExpr::Cast { to_type: "char".to_string(), expr: Box::new(obj) }],
                            });
                        }
                    }

                    // ── NULL-check helpers for string args ────────────────────
                    "assert" => {
                        if let Some(cond) = args.first() {
                            let c = Self::build_expr(cond, sem, struct_fields, fn_param_types)?;
                            return Ok(IrExpr::Call {
                                function: "assert".to_string(),
                                args: vec![c],
                            });
                        }
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
            // ── Integer math ────────────────────────────────────────────────
            "abs"    => "i32::abs".to_string(),
            "labs"   => "i64::abs".to_string(),
            "llabs"  => "i128::abs".to_string(),

            // ── Floating-point math (double) ─────────────────────────────────
            "fabs"   => "f64::abs".to_string(),
            "sqrt"   => "f64::sqrt".to_string(),
            "cbrt"   => "f64::cbrt".to_string(),
            "pow"    => "f64::powi".to_string(),
            "floor"  => "f64::floor".to_string(),
            "ceil"   => "f64::ceil".to_string(),
            "round"  => "f64::round".to_string(),
            "trunc"  => "f64::trunc".to_string(),
            "fmod"   => "f64::rem_euclid".to_string(),
            "exp"    => "f64::exp".to_string(),
            "exp2"   => "f64::exp2".to_string(),
            "log"    => "f64::ln".to_string(),
            "log2"   => "f64::log2".to_string(),
            "log10"  => "f64::log10".to_string(),
            "sin"    => "f64::sin".to_string(),
            "cos"    => "f64::cos".to_string(),
            "tan"    => "f64::tan".to_string(),
            "asin"   => "f64::asin".to_string(),
            "acos"   => "f64::acos".to_string(),
            "atan"   => "f64::atan".to_string(),
            "atan2"  => "f64::atan2".to_string(),
            "sinh"   => "f64::sinh".to_string(),
            "cosh"   => "f64::cosh".to_string(),
            "tanh"   => "f64::tanh".to_string(),
            "hypot"  => "f64::hypot".to_string(),
            "fmax"   => "f64::max".to_string(),
            "fmin"   => "f64::min".to_string(),

            // ── Floating-point math (float) ──────────────────────────────────
            "fabsf"  => "f32::abs".to_string(),
            "sqrtf"  => "f32::sqrt".to_string(),
            "cbrtf"  => "f32::cbrt".to_string(),
            "powf"   => "f32::powi".to_string(),
            "floorf" => "f32::floor".to_string(),
            "ceilf"  => "f32::ceil".to_string(),
            "roundf" => "f32::round".to_string(),
            "truncf" => "f32::trunc".to_string(),
            "expf"   => "f32::exp".to_string(),
            "logf"   => "f32::ln".to_string(),
            "log2f"  => "f32::log2".to_string(),
            "log10f" => "f32::log10".to_string(),
            "sinf"   => "f32::sin".to_string(),
            "cosf"   => "f32::cos".to_string(),
            "tanf"   => "f32::tan".to_string(),
            "asinf"  => "f32::asin".to_string(),
            "acosf"  => "f32::acos".to_string(),
            "atanf"  => "f32::atan".to_string(),
            "atan2f" => "f32::atan2".to_string(),
            "fmaxf"  => "f32::max".to_string(),
            "fminf"  => "f32::min".to_string(),

            // ── Random / conversion ──────────────────────────────────────────
            "rand"   => "rand::random::<i32>".to_string(),
            "atoi"   => "str::parse::<i32>".to_string(),
            "atof"   => "str::parse::<f64>".to_string(),
            "atol"   => "str::parse::<i64>".to_string(),

            other    => other.to_string(),
        }
    }

    fn c_name_to_rust(name: &str) -> &str {
        match name {
            "int" => "i32", "char" => "u8", "float" => "f32",
            "double" => "f64", "long" => "i64", other => other,
        }
    }

    /// Resolve the argument of a C `sizeof` expression to the appropriate Rust type string.
    ///
    /// Handles:
    ///  1. Known C type keywords  (`int` → `i32`, etc.)
    ///  2. Variable names in scope → look up the semantic type map
    ///  3. Array-element expressions → `sizeof(arr[0])` → element type of arr
    ///  4. Numeric / char literals captured by the token extractor
    ///     (`sizeof(4)` → `"4"`) → infer from the literal text
    ///  5. Struct / user type names → pass through as-is
    fn sizeof_arg_to_rust_type(arg: &str, sem: &semantic::FunctionSemantics) -> String {
        let trimmed = arg.trim();

        // ── 1. Known C primitive type names ──────────────────────────────────
        let known = match trimmed {
            "void"               => Some("()"),
            "char"               => Some("u8"),
            "unsigned char"      => Some("u8"),
            "short"              => Some("i16"),
            "unsigned short"     => Some("u16"),
            "int"                => Some("i32"),
            "unsigned" | "unsigned int" => Some("u32"),
            "long"               => Some("i64"),
            "unsigned long"      => Some("u64"),
            "long long"          => Some("i64"),
            "unsigned long long" => Some("u64"),
            "float"              => Some("f32"),
            "double"             => Some("f64"),
            "long double"        => Some("f64"),
            "bool" | "_Bool"     => Some("bool"),
            "size_t"             => Some("usize"),
            "ptrdiff_t"          => Some("isize"),
            _ => None,
        };
        if let Some(t) = known { return t.to_string(); }

        // ── 2a. Clang array TYPE spelling: "int [7]", "double [4]", etc. ──
        // clang_getTypeSpelling emits array types as "<elem_type> [<N>]"
        // (note the SPACE before '[').  Must be handled before the variable-
        // subscript check.
        if let Some(bracket) = trimmed.find('[') {
            let before = &trimmed[..bracket];
            let inside = trimmed[bracket + 1..].trim_end_matches(']').trim();
            if before.ends_with(' ') {
                // This is a C type spelling, e.g. "int [7]" or "double []"
                let elem_rust = Self::c_type_name_to_rust(before.trim());
                if inside.is_empty() {
                    return elem_rust; // incomplete array → just element type
                }
                if let Ok(n) = inside.parse::<u64>() {
                    return format!("[{}; {}]", elem_rust, n);
                }
                return elem_rust; // VLA or unknown size
            }
        }

        // ── 2b. Array-element subscript: sizeof(arr[i]) / sizeof(arr[0]) ──────
        // Strip the subscript and resolve the element type of the array variable.
        if let Some(bracket) = trimmed.find('[') {
            let base = trimmed[..bracket].trim();
            // Look up the array's full Rust type, then extract its element type.
            let elem = if let Some(rt) = sem.rust_type_map.get(base) {
                Self::extract_array_elem_type_str(&rt.rust_repr)
            } else {
                // Unknown array — default to i32 (most common C array element type)
                "i32".to_string()
            };
            return elem;
        }

        // ── 3. Variable name in scope ─────────────────────────────────────────
        if let Some(rt) = sem.rust_type_map.get(trimmed) {
            return rt.rust_repr.clone();
        }

        // ── 4. Literal value used as sizeof argument ──────────────────────────
        //    sizeof(4)   → integer literal → i32
        //    sizeof(4L)  → long literal   → i64
        //    sizeof(4U)  → unsigned lit   → u32
        //    sizeof(4.0) / sizeof(4.0f)   → float literal → f32/f64
        //    sizeof('a') → char literal   → u8

        // char literal
        if trimmed.starts_with('\'') { return "u8".to_string(); }

        let lower = trimmed.to_ascii_lowercase();
        let digits = lower.trim_end_matches(|c: char| "ulfd".contains(c));

        // float literal: contains '.' or 'e'/'E'
        if digits.contains('.') || digits.contains('e') {
            if lower.ends_with('f') { return "f32".to_string(); }
            return "f64".to_string();
        }

        // integer literal suffixes
        if lower.ends_with("ull") || lower.ends_with("llu") { return "u64".to_string(); }
        if lower.ends_with("ll")                             { return "i64".to_string(); }
        if lower.ends_with("ul") || lower.ends_with("lu")   { return "u64".to_string(); }
        if lower.ends_with('u')                              { return "u32".to_string(); }
        if lower.ends_with('l')                              { return "i64".to_string(); }

        // bare decimal / hex / octal integer
        if trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return "i32".to_string();
        }

        // ── 5. Struct / user type — pass through as-is ───────────────────────
        trimmed.to_string()
    }

    /// Extract the element type from a Rust array or Vec type string.
    ///
    /// `[i32; 7]`  → `"i32"`
    /// Convert a C primitive type name string to its Rust equivalent.
    /// Used by sizeof_arg_to_rust_type for Clang array type spellings.
    fn c_type_name_to_rust(c: &str) -> String {
        match c.trim() {
            "void"                       => "()",
            "char" | "unsigned char"     => "u8",
            "signed char"                => "i8",
            "short" | "short int"
            | "signed short"
            | "signed short int"         => "i16",
            "unsigned short"
            | "unsigned short int"       => "u16",
            "int" | "signed" | "signed int" => "i32",
            "unsigned" | "unsigned int"  => "u32",
            "long" | "long int"
            | "signed long"
            | "signed long int"          => "i64",
            "unsigned long"
            | "unsigned long int"        => "u64",
            "long long" | "long long int"
            | "signed long long"
            | "signed long long int"     => "i64",
            "unsigned long long"
            | "unsigned long long int"   => "u64",
            "float"                      => "f32",
            "double"                     => "f64",
            "long double"                => "f64",
            "bool" | "_Bool"             => "bool",
            "size_t"                     => "usize",
            "ptrdiff_t"                  => "isize",
            other                        => other,
        }.to_string()
    }

        /// `[f64; 3]`  → `"f64"`
    /// `Vec<i32>`  → `"i32"`
    /// `i32`       → `"i32"`   (already a scalar — use it directly)
    fn extract_array_elem_type_str(rust_type: &str) -> String {
        let t = rust_type.trim();
        // [T; N]
        if t.starts_with('[') {
            if let (Some(start), Some(semi)) = (t.find('['), t.find(';')) {
                return t[start + 1..semi].trim().to_string();
            }
        }
        // Vec<T>
        if let Some(inner) = t.strip_prefix("Vec<") {
            if let Some(inner) = inner.strip_suffix('>') {
                return inner.trim().to_string();
            }
        }
        // Scalar type (e.g. sizeof applied to a non-array variable)
        t.to_string()
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
            // char* / unsigned char* → String (C-style string)
            CType::Pointer(inner) if matches!(inner.as_ref(), CType::Char | CType::UChar) =>
                "String".to_string(),
            // void* → *mut u8 (raw byte pointer)
            CType::Pointer(inner) if matches!(inner.as_ref(), CType::Void) =>
                "*mut u8".to_string(),
            // Other pointers → Box<T> when it's a struct, *mut T for primitives
            CType::Pointer(inner) => format!("*mut {}", Self::fallback_type(inner)),
            // char[] → String; other arrays stay as [T; N]
            CType::Array(inner, _) if matches!(inner.as_ref(), CType::Char | CType::UChar) =>
                "String".to_string(),
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
            // std::mem::size_of returns usize; insert cast when assigned to
            // any signed/unsigned integer type other than usize.
            _ if Self::expr_yields_usize(&expr) && Self::is_target_integer(rust_type) => {
                IrExpr::Cast { to_type: rust_type.to_string(), expr: Box::new(expr) }
            }
            _ => expr,
        }
    }

    /// True when `expr` is — or is a binary operation entirely composed of —
    /// `std::mem::size_of` calls, meaning its Rust type is `usize`.
    fn expr_yields_usize(expr: &IrExpr) -> bool {
        match expr {
            IrExpr::Call { function, .. } => function.starts_with("std::mem::size_of"),
            IrExpr::BinOp { left, right, .. } => {
                Self::expr_yields_usize(left) || Self::expr_yields_usize(right)
            }
            IrExpr::Cast { to_type, .. } => to_type == "usize",
            _ => false,
        }
    }

    /// Integer types that are NOT usize — i.e. need an explicit cast from usize.
    fn is_target_integer(t: &str) -> bool {
        matches!(t, "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
                  | "u8" | "u16" | "u32" | "u64" | "u128")
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
    /// Returns true if `use std::io::{self, Write}` is needed.
    /// This covers: print! (needs flush), scanf (needs stdin().read_line), ReadLine.
    fn ir_uses_print_flush(functions: &[IrFunction]) -> bool {
        functions.iter().any(|f| Self::stmts_use_io(&f.body))
    }

    fn stmts_use_io(stmts: &[IrStatement]) -> bool {
        stmts.iter().any(|s| match s {
            // print! with flush
            IrStatement::Expr(IrExpr::Print { .. })
            | IrStatement::Expr(IrExpr::EPrintLn { .. }) => true,
            // scanf / read_line all use io::stdin()
            IrStatement::Scanf { .. } => true,
            IrStatement::Expr(IrExpr::ReadLine { .. }) => true,
            IrStatement::If { then_block, else_block, .. } =>
                Self::stmts_use_io(then_block)
                || else_block.as_ref().map(|e| Self::stmts_use_io(e)).unwrap_or(false),
            IrStatement::While { body, .. }
            | IrStatement::Loop { body, .. } => Self::stmts_use_io(body),
            IrStatement::While2 { body, .. } => Self::stmts_use_io(body),
            _ => false,
        })
    }

    // Keep old name for callers that check only print
    fn stmts_use_print(stmts: &[IrStatement]) -> bool {
        Self::stmts_use_io(stmts)
    }

    /// Returns true if any function uses raw pointer types (needs `use std::ptr`)
    fn ir_uses_raw_ptr(functions: &[IrFunction]) -> bool {
        functions.iter().any(|f| {
            // Check function signature
            let sig_has_ptr = f.parameters.iter().any(|p|
                p.rust_type.contains("*mut") || p.rust_type.contains("*const"))
                || f.rust_return_type.contains("*mut")
                || f.rust_return_type.contains("*const");
            // Also check local variable bindings in the body
            let body_has_ptr = Self::stmts_use_raw_ptr(&f.body);
            sig_has_ptr || body_has_ptr
        })
    }

    fn stmts_use_raw_ptr(stmts: &[IrStatement]) -> bool {
        stmts.iter().any(|s| match s {
            IrStatement::LetBinding { rust_type, .. } =>
                rust_type.contains("*mut") || rust_type.contains("*const"),
            IrStatement::If { then_block, else_block, .. } =>
                Self::stmts_use_raw_ptr(then_block)
                || else_block.as_ref().map(|e| Self::stmts_use_raw_ptr(e)).unwrap_or(false),
            IrStatement::While { body, .. }
            | IrStatement::Loop { body, .. } => Self::stmts_use_raw_ptr(body),
            IrStatement::While2 { body, .. } => Self::stmts_use_raw_ptr(body),
            _ => false,
        })
    }

    fn coerce_arg(arg: IrExpr, param_type: &str, sem: &semantic::FunctionSemantics) -> IrExpr {
        if param_type.starts_with("&mut [") {
            match &arg {
                IrExpr::Var(name) => {
                    // If this var is already a &mut [T] in the current function
                    // (it is itself a pointer/array PARAMETER), do NOT wrap in &mut —
                    // that causes E0596 ("cannot borrow as mutable, already a mutable borrow").
                    // Check 1: semantic rust_type_map already says &mut [...]
                    let already_by_type = sem.rust_type_map
                        .get(name)
                        .map(|rt| rt.rust_repr.starts_with("&mut ["))
                        .unwrap_or(false);
                    // Check 2: it is a *parameter* with a pointer/array C type
                    // (the IR builder turns these into &mut [T] params).
                    let already_as_param = sem.symbol_table.get(name)
                        .map(|s| s.is_parameter && (s.is_pointer || s.is_array))
                        .unwrap_or(false);
                    if already_by_type || already_as_param { return arg; }
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
                // C `&x` is parsed as UnaryOp{AddrOf, x} → must also upgrade to &mut x
                IrExpr::UnaryOp { op: ast_input::UnaryOp::AddrOf, operand } =>
                    return IrExpr::RefMut(operand),
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
