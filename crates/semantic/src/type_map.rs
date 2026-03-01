//! # Type Mapping to Rust (Process 0.3.4)
//!
//! Maps C types and ownership information to idiomatic Rust type strings.
//! Implements SRS §4.6.4 type mapping rules.

use ast_input::*;
use std::collections::HashMap;
use crate::ownership::OwnershipKind;

/// The Rust representation of a C type after semantic analysis.
#[derive(Debug, Clone)]
pub struct RustType {
    /// Full Rust type string, e.g. `"Box<i32>"`, `"&mut [i32; 10]"`.
    pub rust_repr: String,
    /// Whether this type requires `unsafe` to use.
    pub requires_unsafe: bool,
    /// Whether this is an idiomatic Rust type (vs a raw pointer fallback).
    pub is_idiomatic: bool,
}

pub struct TypeMapper;

impl TypeMapper {
    pub fn map_function(
        func: &CFunction,
        ownership_map: &HashMap<String, OwnershipKind>,
    ) -> HashMap<String, RustType> {
        let mut map = HashMap::new();

        // Parameters
        for param in &func.parameters {
            let ownership = ownership_map.get(&param.name).cloned().unwrap_or(OwnershipKind::Value);
            let rt = Self::map_type(&param.param_type, &ownership, false);
            map.insert(param.name.clone(), rt);
        }

        // Walk body
        Self::map_stmts(&func.body, ownership_map, &mut map);

        map
    }

    fn map_stmts(
        stmts: &[CStatement],
        ownership_map: &HashMap<String, OwnershipKind>,
        map: &mut HashMap<String, RustType>,
    ) {
        for stmt in stmts {
            match stmt {
                CStatement::VarDecl { name, var_type, .. } => {
                    let ownership = ownership_map.get(name).cloned().unwrap_or(OwnershipKind::Value);
                    let rt = Self::map_type(var_type, &ownership, false);
                    map.insert(name.clone(), rt);
                }
                CStatement::If { then_body, else_body, .. } => {
                    Self::map_stmts(then_body, ownership_map, map);
                    if let Some(eb) = else_body { Self::map_stmts(eb, ownership_map, map); }
                }
                CStatement::While { body, .. } | CStatement::For { body, .. } => {
                    Self::map_stmts(body, ownership_map, map);
                }
                _ => {}
            }
        }
    }

    /// Map a C type + ownership intent to a Rust type string.
    pub fn map_type(ctype: &CType, ownership: &OwnershipKind, _is_param: bool) -> RustType {
        match ctype {
            CType::Void => RustType {
                rust_repr: "()".to_string(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            CType::Int => RustType {
                rust_repr: "i32".to_string(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            CType::Char => RustType {
                rust_repr: "u8".to_string(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            CType::Float => RustType {
                rust_repr: "f32".to_string(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            CType::Double => RustType {
                rust_repr: "f64".to_string(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            CType::Pointer(inner) => Self::map_pointer(inner, ownership),
            CType::Array(inner, Some(size)) => {
                let inner_rt = Self::map_type(inner, &OwnershipKind::Value, false);
                RustType {
                    rust_repr: format!("[{}; {}]", inner_rt.rust_repr, size),
                    requires_unsafe: false,
                    is_idiomatic: true,
                }
            }
            CType::Array(inner, None) => {
                let inner_rt = Self::map_type(inner, &OwnershipKind::Value, false);
                RustType {
                    rust_repr: format!("Vec<{}>", inner_rt.rust_repr),
                    requires_unsafe: false,
                    is_idiomatic: true,
                }
            }
            CType::Unknown(name) => RustType {
                rust_repr: name.clone(),
                requires_unsafe: false,
                is_idiomatic: false,
            },
        }
    }

    fn map_pointer(inner: &CType, ownership: &OwnershipKind) -> RustType {
        let inner_base = Self::c_primitive_str(inner).unwrap_or_else(|| "u8".to_string());
        match ownership {
            OwnershipKind::UniqueOwned => RustType {
                rust_repr: format!("Box<{}>", inner_base),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            OwnershipKind::SharedOwned => RustType {
                rust_repr: format!("Rc<{}>", inner_base),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            OwnershipKind::InteriorMutable => RustType {
                rust_repr: format!("RefCell<{}>", inner_base),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            OwnershipKind::MutableBorrow => RustType {
                rust_repr: format!("&mut {}", inner_base),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            OwnershipKind::ImmutableBorrow => RustType {
                rust_repr: format!("&{}", inner_base),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            // Fallback for pointer with no better classification: use *mut
            _ => RustType {
                rust_repr: format!("*mut {}", inner_base),
                requires_unsafe: true,
                is_idiomatic: false,
            },
        }
    }

    fn c_primitive_str(ctype: &CType) -> Option<String> {
        match ctype {
            CType::Int => Some("i32".to_string()),
            CType::Char => Some("u8".to_string()),
            CType::Float => Some("f32".to_string()),
            CType::Double => Some("f64".to_string()),
            CType::Void => Some("()".to_string()),
            // Nested pointer: int** → &mut i32 (inner of outer pointer)
            CType::Pointer(inner) => {
                let inner_str = Self::c_primitive_str(inner).unwrap_or_else(|| "u8".to_string());
                Some(format!("&mut {}", inner_str))
            }
            _ => None,
        }
    }

    /// Map a raw C type string to Rust without semantic context.
    pub fn simple_map(ctype: &CType) -> String {
        Self::map_type(ctype, &OwnershipKind::Value, false).rust_repr
    }
}
