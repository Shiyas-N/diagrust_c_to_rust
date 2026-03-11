//! # Type Mapping to Rust (Process 0.3.4) — Enhanced
//!
//! Maps C types and ownership information to idiomatic Rust type strings.
//!
//! ## Improvements over v1
//!
//! * **`char *` → `String` / `&str`** — `char *` pointer inner types are now
//!   recognised and translated to owned `String` (UniqueOwned / StringOwned) or
//!   borrowed `&str` (ImmutableBorrow / StringBorrow) instead of `*mut u8`.
//!
//! * **`void *` handling** — `void *` → `*mut u8` (unsafe raw pointer) with
//!   `requires_unsafe = true` rather than silently generating invalid code.
//!
//! * **Multi-level pointers** — `int **` → `*mut *mut i32` (unsafe fallback)
//!   rather than `&mut &mut i32` which is rarely valid in a translation context.
//!
//! * **`RawPointer` ownership** — maps to `*mut T` / `*const T` appropriately.
//!
//! * **`StringOwned` / `StringBorrow`** — two new ownership kinds produce
//!   idiomatic `String` and `&str` types.

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
            let rt = Self::map_type(&param.param_type, &ownership, true);
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
                    if let Some(eb) = else_body {
                        Self::map_stmts(eb, ownership_map, map);
                    }
                }
                CStatement::While { body, .. }
                | CStatement::For { body, .. }
                | CStatement::DoWhile { body, .. } => {
                    Self::map_stmts(body, ownership_map, map);
                }
                CStatement::Switch { cases, default, .. } => {
                    for (_, body) in cases {
                        Self::map_stmts(body, ownership_map, map);
                    }
                    if let Some(d) = default {
                        Self::map_stmts(d, ownership_map, map);
                    }
                }
                CStatement::StructDecl { name, struct_type, .. } => {
                    map.insert(name.clone(), RustType {
                        rust_repr: struct_type.clone(),
                        requires_unsafe: false,
                        is_idiomatic: true,
                    });
                }
                _ => {}
            }
        }
    }

    // ── Main type mapper ─────────────────────────────────────────────────────

    /// Map a C type + ownership intent to a Rust type string.
    pub fn map_type(ctype: &CType, ownership: &OwnershipKind, _is_param: bool) -> RustType {
        match ctype {
            CType::Void => ok("()"),
            CType::Int  => ok("i32"),
            CType::Char | CType::UChar => {
                // A bare `char` or `unsigned char` (not a pointer) → u8
                ok("u8")
            }
            CType::Float  => ok("f32"),
            CType::Double => ok("f64"),
            CType::Long   => ok("i64"),
            CType::UInt   => ok("u32"),
            CType::ULong  => ok("u64"),
            CType::Short  => ok("i16"),
            CType::Bool   => ok("bool"),

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

            CType::Struct(name) => RustType {
                rust_repr: name.clone(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            CType::Unknown(name) => RustType {
                rust_repr: name.clone(),
                requires_unsafe: false,
                is_idiomatic: false,
            },
        }
    }

    fn map_pointer(inner: &CType, ownership: &OwnershipKind) -> RustType {
        // ── char * special case ──────────────────────────────────────────────
        // `char *` and `unsigned char *` are idiomatic strings in Rust.
        if Self::is_char_inner(inner) {
            return match ownership {
                OwnershipKind::StringOwned
                | OwnershipKind::UniqueOwned
                | OwnershipKind::HeapVec => RustType {
                    rust_repr: "String".to_string(),
                    requires_unsafe: false,
                    is_idiomatic: true,
                },
                OwnershipKind::StringBorrow
                | OwnershipKind::ImmutableBorrow => RustType {
                    rust_repr: "&str".to_string(),
                    requires_unsafe: false,
                    is_idiomatic: true,
                },
                OwnershipKind::MutableBorrow => RustType {
                    rust_repr: "&mut String".to_string(),
                    requires_unsafe: false,
                    is_idiomatic: true,
                },
                _ => RustType {
                    rust_repr: "String".to_string(),
                    requires_unsafe: false,
                    is_idiomatic: true,
                },
            };
        }

        // ── void * special case ──────────────────────────────────────────────
        // `void *` has no safe Rust equivalent without knowing the pointee type;
        // fall back to `*mut u8` (unsafe) rather than emitting wrong code.
        if matches!(inner, CType::Void) {
            return RustType {
                rust_repr: "*mut u8".to_string(),
                requires_unsafe: true,
                is_idiomatic: false,
            };
        }

        // ── Multi-level pointer (int **, char **, …) ─────────────────────────
        // Represented as RawPointer by the ownership inferrer; emit *mut *mut T.
        if matches!(ownership, OwnershipKind::RawPointer) {
            let inner_base = Self::c_type_to_rust(inner);
            return RustType {
                rust_repr: format!("*mut {}", inner_base),
                requires_unsafe: true,
                is_idiomatic: false,
            };
        }

        let inner_base = Self::c_type_to_rust(inner);

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
            OwnershipKind::StringOwned => RustType {
                rust_repr: "String".to_string(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            OwnershipKind::StringBorrow => RustType {
                rust_repr: "&str".to_string(),
                requires_unsafe: false,
                is_idiomatic: true,
            },
            // Raw pointer fallback (also covers multi-level pointers above)
            OwnershipKind::RawPointer | OwnershipKind::Value | _ => RustType {
                rust_repr: format!("*mut {}", inner_base),
                requires_unsafe: true,
                is_idiomatic: false,
            },
        }
    }

    /// Returns true when the innermost C type under a pointer is `char` / `u8`.
    fn is_char_inner(ctype: &CType) -> bool {
        match ctype {
            CType::Char | CType::UChar => true,
            CType::Pointer(inner)      => Self::is_char_inner(inner),
            _                          => false,
        }
    }

    /// Recursively map a C type to a Rust primitive string, independent of
    /// ownership context (used for inner types).
    pub fn c_type_to_rust(ctype: &CType) -> String {
        match ctype {
            CType::Void   => "()".to_string(),
            CType::Int    => "i32".to_string(),
            CType::Char   => "u8".to_string(),
            CType::UChar  => "u8".to_string(),
            CType::Float  => "f32".to_string(),
            CType::Double => "f64".to_string(),
            CType::Long   => "i64".to_string(),
            CType::UInt   => "u32".to_string(),
            CType::ULong  => "u64".to_string(),
            CType::Short  => "i16".to_string(),
            CType::Bool   => "bool".to_string(),
            CType::Struct(n) => n.clone(),
            // Nested pointer: emit *mut inner (unsafe fallback for inner levels)
            CType::Pointer(inner) => format!("*mut {}", Self::c_type_to_rust(inner)),
            CType::Array(inner, Some(n)) => format!("[{}; {}]", Self::c_type_to_rust(inner), n),
            CType::Array(inner, None)    => format!("Vec<{}>", Self::c_type_to_rust(inner)),
            CType::Unknown(s)            => s.clone(),
        }
    }

    /// Kept for backward-compatibility with call sites that use `c_primitive_str`.
    pub fn c_primitive_str(ctype: &CType) -> Option<String> {
        Some(Self::c_type_to_rust(ctype))
    }

    /// Map a raw C type string to Rust without semantic context.
    pub fn simple_map(ctype: &CType) -> String {
        Self::map_type(ctype, &OwnershipKind::Value, false).rust_repr
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn ok(s: &str) -> RustType {
    RustType { rust_repr: s.to_string(), requires_unsafe: false, is_idiomatic: true }
}
