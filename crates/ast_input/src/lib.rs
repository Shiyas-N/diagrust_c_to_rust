// crates/ast_input/Cargo.toml
/*
[package]
name = "ast_input"
version.workspace = true
edition.workspace = true

[dependencies]
anyhow.workspace = true
thiserror.workspace = true
regex = "1.10"
*/

// crates/ast_input/src/lib.rs
use anyhow::{Context, Result};
use std::path::Path;
use thiserror::Error;

mod control_flow;
use control_flow::ControlFlowParser;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(String),
    
    #[error("Syntax error at line {line}: {message}")]
    SyntaxError { line: usize, message: String },
    
    #[error("Empty or invalid C source")]
    EmptySource,
}

/// Represents a simplified C Abstract Syntax Tree
#[derive(Debug, Clone)]
pub struct CProgram {
    pub functions: Vec<CFunction>,
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

#[derive(Debug, Clone, PartialEq)]
pub enum CType {
    Void,
    Int,
    Char,
    Float,
    Double,
    Pointer(Box<CType>),
    Array(Box<CType>, Option<usize>), // base_type, optional_size
    Unknown(String),
}

#[derive(Debug, Clone)]
pub enum CStatement {
    Return(Option<CExpression>),
    VarDecl { name: String, var_type: CType, init: Option<CExpression> },
    Assignment { target: String, value: CExpression },
    IndexedAssignment { array: String, index: CExpression, value: CExpression }, // NEW: arr[i] = val
    Expression(CExpression),
    If { condition: CExpression, then_body: Vec<CStatement>, else_body: Option<Vec<CStatement>> },
    While { condition: CExpression, body: Vec<CStatement> },
    For { init: Box<CStatement>, condition: CExpression, increment: CExpression, body: Vec<CStatement> },
    Break,    // NEW
    Continue, // NEW
}

#[derive(Debug, Clone)]
pub enum CExpression {
    Literal(CLiteral),
    Variable(String),
    BinaryOp { left: Box<CExpression>, op: BinOp, right: Box<CExpression> },
    UnaryOp { op: UnaryOp, operand: Box<CExpression> },
    Call { function: String, args: Vec<CExpression> },
    ArrayAccess { array: Box<CExpression>, index: Box<CExpression> }, // NEW: arr[i]
    ArrayLiteral(Vec<CExpression>), // NEW: {1, 2, 3}
    Cast { to_type: CType, expr: Box<CExpression> }, // NEW: (int)x
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
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg, Not, Deref, AddrOf,
}

/// Input validator - checks if C code is valid for translation
pub struct InputValidator {
    strict_mode: bool,
}

impl InputValidator {
    pub fn new(strict_mode: bool) -> Self {
        Self { strict_mode }
    }
    
    /// Validate C source code before processing
    pub fn validate(&self, source: &str) -> Result<(), ValidationError> {
        if source.trim().is_empty() {
            return Err(ValidationError::EmptySource);
        }
        
        // Check for unsupported features
        self.check_unsupported_features(source)?;
        
        // Basic syntax validation
        self.check_basic_syntax(source)?;
        
        // Check for common syntax errors
        self.check_common_errors(source)?;
        
        // Advanced validations (if strict mode)
        if self.strict_mode {
            self.check_type_mismatches(source)?;
            self.check_uninitialized_usage(source)?;
            self.check_function_definitions(source)?;
        }
        
        Ok(())
    }
    
    fn check_unsupported_features(&self, source: &str) -> Result<(), ValidationError> {
        // Features not supported in MVP
        let unsupported = [
            ("goto", "goto statements"),
            ("asm", "inline assembly"),
            ("__asm", "inline assembly"),
            ("union", "unions"),
            ("#define", "complex macros (simple defines ok)"),
            ("switch", "switch statements (use if-else)"),
            ("typedef", "typedef (use direct types)"),
            ("static", "static variables"),
            ("extern", "extern declarations"),
        ];
        
        for (keyword, feature) in unsupported {
            if source.contains(keyword) && self.strict_mode {
                return Err(ValidationError::UnsupportedFeature(feature.to_string()));
            }
        }
        
        Ok(())
    }
    
    fn check_basic_syntax(&self, source: &str) -> Result<(), ValidationError> {
        // Count braces
        let open_braces = source.matches('{').count();
        let close_braces = source.matches('}').count();
        
        if open_braces != close_braces {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: format!("Mismatched braces: {} open, {} close", open_braces, close_braces),
            });
        }
        
        // Count parentheses
        let open_parens = source.matches('(').count();
        let close_parens = source.matches(')').count();
        
        if open_parens != close_parens {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: format!("Mismatched parentheses: {} open, {} close", open_parens, close_parens),
            });
        }
        
        // Check brackets
        let open_brackets = source.matches('[').count();
        let close_brackets = source.matches(']').count();
        
        if open_brackets != close_brackets {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: format!("Mismatched brackets: {} open, {} close", open_brackets, close_brackets),
            });
        }
        
        // Check for at least one function
        if !source.contains("(") || !source.contains(")") {
            return Err(ValidationError::SyntaxError {
                line: 0,
                message: "No function found".to_string(),
            });
        }
        
        Ok(())
    }
    
    fn check_common_errors(&self, source: &str) -> Result<(), ValidationError> {
        let lines: Vec<&str> = source.lines().collect();
        
        for (line_num, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            
            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
                continue;
            }
            
            // Check for missing semicolons after statements
            if self.should_have_semicolon(trimmed) && !trimmed.ends_with(';') 
                && !trimmed.ends_with('{') && !trimmed.ends_with('}') {
                // Check if next line might be continuation
                if line_num + 1 < lines.len() {
                    let next_line = lines[line_num + 1].trim();
                    if !next_line.starts_with('{') && !next_line.is_empty() && !next_line.starts_with(')') {
                        return Err(ValidationError::SyntaxError {
                            line: line_num + 1,
                            message: format!("Possible missing semicolon: '{}'", trimmed),
                        });
                    }
                }
            }
            
            // Check for missing semicolon specifically after return
            if trimmed.contains("return") && !trimmed.ends_with(';') && !trimmed.ends_with('{') && !trimmed.ends_with('}') {
                return Err(ValidationError::SyntaxError {
                    line: line_num + 1,
                    message: format!("Missing semicolon after return statement: '{}'", trimmed),
                });
            }
            
            // Check for assignment without declaration
            if trimmed.contains('=') && !trimmed.contains("==") && !trimmed.ends_with(';') && !trimmed.starts_with("for")
    && !trimmed.starts_with("if")
    && !trimmed.starts_with("while") {
                let has_type = ["int", "float", "char", "double", "void"].iter().any(|t| trimmed.contains(t));
                if has_type {
                    return Err(ValidationError::SyntaxError {
                        line: line_num + 1,
                        message: format!("Missing semicolon after variable declaration: '{}'", trimmed),
                    });
                }
            }
        }
        
        Ok(())
    }
    
    fn check_type_mismatches(&self, source: &str) -> Result<(), ValidationError> {
        let lines: Vec<&str> = source.lines().collect();
        
        for (line_num, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            
            // Check for obvious type mismatches
            // Example: int x = "string"; or float x = 'c';
            
            // Integer assigned string
            if trimmed.contains("int") && trimmed.contains('=') && trimmed.contains('"') {
                return Err(ValidationError::SyntaxError {
                    line: line_num + 1,
                    message: format!("Type mismatch: integer variable assigned string literal: '{}'", trimmed),
                });
            }
            
            // Float/double assigned character
            if (trimmed.contains("float") || trimmed.contains("double")) 
                && trimmed.contains('=') && trimmed.contains('\'') {
                return Err(ValidationError::SyntaxError {
                    line: line_num + 1,
                    message: format!("Type mismatch: float/double assigned character literal: '{}'", trimmed),
                });
            }
            
            // Char assigned string
            if trimmed.contains("char") && !trimmed.contains("char*") 
                && trimmed.contains('=') && trimmed.contains('"') {
                return Err(ValidationError::SyntaxError {
                    line: line_num + 1,
                    message: format!("Type mismatch: char variable assigned string literal (use char* or single char): '{}'", trimmed),
                });
            }
            
            // Void variable (can't declare void variable)
            if trimmed.starts_with("void ") && trimmed.contains('=') {
                return Err(ValidationError::SyntaxError {
                    line: line_num + 1,
                    message: format!("Invalid: cannot declare void variable: '{}'", trimmed),
                });
            }
        }
        
        Ok(())
    }
    
    fn check_uninitialized_usage(&self, source: &str) -> Result<(), ValidationError> {
        use std::collections::HashSet;
        
        let lines: Vec<&str> = source.lines().collect();
        let mut declared_vars: HashSet<String> = HashSet::new();
        let mut initialized_vars: HashSet<String> = HashSet::new();
        
        for (line_num, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            
            // Track variable declarations
            let types = ["int", "float", "char", "double"];
            for vtype in &types {
                if trimmed.starts_with(vtype) && trimmed.contains(' ') {
                    // Extract variable name
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let var_name = parts[1].trim_end_matches(';').trim_end_matches('=').to_string();
                        declared_vars.insert(var_name.clone());
                        
                        // Check if initialized
                        if trimmed.contains('=') {
                            initialized_vars.insert(var_name);
                        }
                    }
                }
            }
            
            // Check usage of uninitialized variables in return
            if trimmed.contains("return") && !trimmed.contains("return;") {
                for var in &declared_vars {
                    if trimmed.contains(var) && !initialized_vars.contains(var) {
                        return Err(ValidationError::SyntaxError {
                            line: line_num + 1,
                            message: format!("Warning: returning potentially uninitialized variable '{}': '{}'", var, trimmed),
                        });
                    }
                }
            }
        }
        
        Ok(())
    }
    
    fn check_function_definitions(&self, source: &str) -> Result<(), ValidationError> {
        let lines: Vec<&str> = source.lines().collect();
        
        for (line_num, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            
            // Check for function declarations without body.
            // Uses regex to avoid false positives like: int mid = (a + b) / 2;
            // A real forward declaration must be: `type name(params);` with no `=`.
            {
                let fwd_re = regex::Regex::new(
                    r"^(int|void|float|double|char)\s+\w+\s*\([^)]*\)\s*;$"
                ).unwrap();
                if fwd_re.is_match(trimmed) && !trimmed.contains('=') {
                    return Err(ValidationError::SyntaxError {
                        line: line_num + 1,
                        message: format!("Function declaration without definition (forward declaration not supported): '{}'", trimmed),
                    });
                }
            }
            
            // FIXED: Check for missing return in non-void functions
            // Only check if it's a simple function without control flow
            if trimmed.contains("int ") && trimmed.contains('(') && trimmed.contains(')') 
                && trimmed.ends_with('{') && !source.contains("if") && !source.contains("while") && !source.contains("for") {
                // This is start of a function without control flow, check if it has return
                let mut has_return = false;
                for i in (line_num + 1)..lines.len() {
                    if lines[i].contains("return") {
                        has_return = true;
                        break;
                    }
                    if lines[i].trim() == "}" {
                        break;
                    }
                }
                
                if !has_return {
                    return Err(ValidationError::SyntaxError {
                        line: line_num + 1,
                        message: format!("Non-void function missing return statement: '{}'", trimmed),
                    });
                }
            }
        }
        
        Ok(())
    }
    
    fn should_have_semicolon(&self, line: &str) -> bool {
        // Statements that should end with semicolon
        let statement_keywords = ["return", "int", "char", "float", "double", "void"];
        
        // Check if line starts with a statement keyword
        for keyword in &statement_keywords {
            if line.starts_with(keyword) {
                return true;
            }
        }
        
        // Check for assignment pattern (variable = ...)
        if line.contains('=') && !line.contains("==") && !line.contains("<=") && !line.contains(">=") {
            return true;
        }
        
        // Check for function calls (word followed by parentheses)
        if line.contains('(') && line.contains(')') && !line.contains("if") && !line.contains("while") && !line.contains("for") {
            let has_type = statement_keywords.iter().any(|k| line.contains(k));
            if !has_type && !line.ends_with('{') {
                return true;
            }
        }
        
        false
    }
}

/// Simple C parser (MVP - pattern matching based)
pub struct SimpleParser;

impl SimpleParser {
    pub fn parse(source: &str) -> Result<CProgram> {
    let mut functions = Vec::new();

    let header_pattern = regex::Regex::new(
        r"(?m)^\s*(int|void|char|float|double)\s+(\w+)\s*\(([^)]*)\)\s*\{"
    ).unwrap();

    for cap in header_pattern.captures_iter(source) {
        let return_type = Self::parse_type(&cap[1]);
        let name = cap[2].to_string();
        let params_str = &cap[3];

        // Find where this match starts
        let header_match = cap.get(0).unwrap();
        let start_index = header_match.end() - 1; // points to '{'

        // Extract full body using brace matching
        let (body_str, _) = extract_balanced_braces(source, start_index)?;

        let parameters = Self::parse_parameters(params_str)?;
        let body = ControlFlowParser::parse_body(&body_str)?;

        functions.push(CFunction {
            name,
            return_type,
            parameters,
            body,
        });
    }

    Ok(CProgram { functions })
}
    
    pub fn parse_type(type_str: &str) -> CType {
        let s = type_str.trim();
        // Array: int[5] or int[]
        let arr_re = regex::Regex::new(r"^(int|char|float|double|void)\[(\d*)\]$").unwrap();
        if let Some(cap) = arr_re.captures(s) {
            let base = Self::parse_type(&cap[1]);
            let size = cap[2].parse::<usize>().ok();
            return CType::Array(Box::new(base), size);
        }
        // Pointer: int*
        if s.ends_with('*') {
            let inner = Self::parse_type(&s[..s.len()-1]);
            return CType::Pointer(Box::new(inner));
        }
        match s {
            "void" => CType::Void,
            "int" => CType::Int,
            "char" => CType::Char,
            "float" => CType::Float,
            "double" => CType::Double,
            other => CType::Unknown(other.to_string()),
        }
    }
    
    fn parse_parameters(params_str: &str) -> Result<Vec<CParameter>> {
        let mut parameters = Vec::new();
        
        if params_str.trim().is_empty() || params_str.trim() == "void" {
            return Ok(parameters);
        }

        // Regex to match pointer params: "int *x", "int* x", "int **pp"
        let ptr_re = regex::Regex::new(
            r"^\s*(int|char|float|double)\s*(\*+)\s*(\w+)\s*$"
        ).unwrap();
        
        for param in params_str.split(',') {
            let p = param.trim();

            // Pointer parameter: int *x, int* x, int **pp
            if let Some(cap) = ptr_re.captures(p) {
                let base_type = Self::parse_type(&cap[1]);
                let stars = cap[2].len();
                let name = cap[3].to_string();
                let mut param_type = base_type;
                for _ in 0..stars {
                    param_type = CType::Pointer(Box::new(param_type));
                }
                parameters.push(CParameter { name, param_type });
                continue;
            }

            // Normal parameter: int x, float y
            let parts: Vec<&str> = p.split_whitespace().collect();
            if parts.len() >= 2 {
                let param_type = Self::parse_type(parts[0]);
                let name = parts[1].trim_end_matches(';').to_string();
                parameters.push(CParameter { name, param_type });
            }
        }
        
        Ok(parameters)
    }
    
    fn parse_body(body_str: &str) -> Result<Vec<CStatement>> {
        let mut statements = Vec::new();
        
        // Split body into lines and process each statement
        let lines: Vec<&str> = body_str
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        
        for line in lines {
            // Skip if line doesn't end with semicolon (incomplete statement)
            if !line.ends_with(';') && !line.ends_with('}') {
                continue;
            }
            
            let line = line.trim_end_matches(';').trim();
            
            // Parse return statements
            if line.starts_with("return") {
                let return_pattern = regex::Regex::new(r"return\s+(.+)").unwrap();
                if let Some(cap) = return_pattern.captures(line) {
                    let expr = Self::parse_expression(&cap[1])?;
                    statements.push(CStatement::Return(Some(expr)));
                    continue;
                }
            }
            
            // Parse variable declarations with initialization
            let decl_pattern = regex::Regex::new(r"^(int|char|float|double)\s+(\w+)\s*=\s*(.+)$").unwrap();
            if let Some(cap) = decl_pattern.captures(line) {
                let var_type = Self::parse_type(&cap[1]);
                let name = cap[2].to_string();
                let init = Self::parse_expression(&cap[3])?;
                statements.push(CStatement::VarDecl {
                    name,
                    var_type,
                    init: Some(init),
                });
                continue;
            }
            
            // Parse variable declarations without initialization
            let decl_simple_pattern = regex::Regex::new(r"^(int|char|float|double)\s+(\w+)$").unwrap();
            if let Some(cap) = decl_simple_pattern.captures(line) {
                let var_type = Self::parse_type(&cap[1]);
                let name = cap[2].to_string();
                statements.push(CStatement::VarDecl {
                    name,
                    var_type,
                    init: None,
                });
                continue;
            }
            
            // Parse assignments (for variables already declared)
            let assign_pattern = regex::Regex::new(r"^(\w+)\s*=\s*(.+)$").unwrap();
            if let Some(cap) = assign_pattern.captures(line) {
                let target = cap[1].to_string();
                let value = Self::parse_expression(&cap[2])?;
                statements.push(CStatement::Assignment { target, value });
                continue;
            }
        }
        
        Ok(statements)
    }
    
    fn parse_expression(expr_str: &str) -> Result<CExpression> {
    let expr = expr_str.trim();

    // ===============================
    // 0️⃣ Empty
    // ===============================
    if expr.is_empty() {
        return Ok(CExpression::Null);
    }

    // ===============================
    // 0.5️⃣ Parenthesized expression: strip outer parens
    // ===============================
    if expr.starts_with('(') && expr.ends_with(')') {
        // Verify the opening '(' pairs with the LAST ')' — not e.g. (float)(a+b)
        let mut depth = 0i32;
        let mut first_close_pos = expr.len(); // position where depth first hits 0
        for (i, ch) in expr.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 { first_close_pos = i; break; }
                }
                _ => {}
            }
        }
        // Only strip outer parens when the first '(' pairs with the very last ')'
        if first_close_pos == expr.len() - 1 {
            let inner = &expr[1..expr.len()-1];
            // Skip if inner is a bare type name (cast prefix with no operand)
            let cast_types = ["int", "float", "double", "char", "long", "unsigned int", "unsigned"];
            if cast_types.iter().any(|t| inner.trim() == *t) {
                // It's a cast prefix but no operand — skip, handle below
            } else {
                return Self::parse_expression(inner);
            }
        }
    }

    // ===============================
    // 1️⃣ Type cast: (int)expr, (float)expr
    // ===============================
    // Match cast prefix: (type) followed by something
    let cast_prefix_re = regex::Regex::new(r"^\((int|float|double|char|long)\)(.+)$").unwrap();
    if let Some(cap) = cast_prefix_re.captures(expr) {
        let to_type = Self::parse_type(&cap[1]);
        let after_cast = cap[2].trim();
        let type_end = cap.get(1).unwrap().end() + 1; // position after ')'

        // Find where the immediate cast operand ends.
        // The cast binds tighter than binary ops, so (double)x * y = ((double)x) * y
        // We extract just the first "atom": a parenthesized group OR a word token.
        let cast_operand_end = if after_cast.starts_with('(') {
            // Find matching close paren
            let mut depth = 0i32;
            let mut end = 0usize;
            for (i, ch) in after_cast.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 { end = i + 1; break; }
                    }
                    _ => {}
                }
            }
            end
        } else {
            // Take up to next operator or end of string
            let mut end = after_cast.len();
            let bytes = after_cast.as_bytes();
            for i in 0..bytes.len() {
                if bytes[i] == b' ' || bytes[i] == b'+' || bytes[i] == b'-'
                    || bytes[i] == b'*' || bytes[i] == b'/' || bytes[i] == b'%'
                    || bytes[i] == b'<' || bytes[i] == b'>' || bytes[i] == b'='
                    || bytes[i] == b'&' || bytes[i] == b'|' {
                    end = i;
                    break;
                }
            }
            end
        };

        let cast_operand = &after_cast[..cast_operand_end];
        let rest = after_cast[cast_operand_end..].trim();

        let cast_inner = Self::parse_expression(cast_operand)?;
        let cast_expr = CExpression::Cast { to_type, expr: Box::new(cast_inner) };

        if rest.is_empty() {
            // Simple cast: (float)x
            return Ok(cast_expr);
        } else {
            // Cast followed by binary op: (double)x * factor
            // Re-parse as: cast_expr OP rest
            let full = format!("__CAST__ {}", rest);
            // Build BinaryOp manually by prepending cast result
            // We synthesize the full expression with a placeholder and parse the rest
            let rhs_expr = Self::parse_expression(rest.trim_start_matches(|c: char| {
                c == '*' || c == '/' || c == '+' || c == '-' || c == '%' || c == ' '
            }))?;
            let op_char = rest.chars().next().unwrap_or('+');
            let op = match op_char {
                '+' => BinOp::Add,
                '-' => BinOp::Sub,
                '*' => BinOp::Mul,
                '/' => BinOp::Div,
                '%' => BinOp::Mod,
                _ => BinOp::Add,
            };
            return Ok(CExpression::BinaryOp {
                left: Box::new(cast_expr),
                op,
                right: Box::new(rhs_expr),
            });
        }
    }

    // ===============================
    // 2️⃣ Array literal: {1, 2, 3}
    // ===============================
    if expr.starts_with('{') && expr.ends_with('}') {
        let inner = &expr[1..expr.len()-1];
        let elements: Result<Vec<CExpression>> = inner
            .split(',')
            .map(|e| Self::parse_expression(e.trim()))
            .collect();
        return Ok(CExpression::ArrayLiteral(elements?));
    }

    // ===============================
    // 3️⃣ Logical operators (lowest precedence)
    // ===============================
    if let Some(pos) = find_operator_outside_parens(expr, "||") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+2..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Or, right: Box::new(right) });
    }
    if let Some(pos) = find_operator_outside_parens(expr, "&&") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+2..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::And, right: Box::new(right) });
    }

    // ===============================
    // 4️⃣ Equality operators
    // ===============================
    if let Some(pos) = find_operator_outside_parens(expr, "==") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+2..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Equal, right: Box::new(right) });
    }
    if let Some(pos) = find_operator_outside_parens(expr, "!=") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+2..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::NotEqual, right: Box::new(right) });
    }

    // ===============================
    // 5️⃣ Relational operators
    // ===============================
    if let Some(pos) = find_operator_outside_parens(expr, "<=") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+2..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::LessEq, right: Box::new(right) });
    }
    if let Some(pos) = find_operator_outside_parens(expr, ">=") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+2..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::GreaterEq, right: Box::new(right) });
    }
    if let Some(pos) = find_operator_outside_parens(expr, "<") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+1..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Less, right: Box::new(right) });
    }
    if let Some(pos) = find_operator_outside_parens(expr, ">") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+1..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Greater, right: Box::new(right) });
    }

    // ===============================
    // 6️⃣ Assignment (single =)
    // ===============================
    if let Some(pos) = find_plain_assign_outside_parens(expr) {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+1..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Assign, right: Box::new(right) });
    }

    // ===============================
    // 7️⃣ Arithmetic: +, -, *, /, %
    // ===============================
    if let Some(pos) = find_operator_outside_parens(expr, "+") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+1..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Add, right: Box::new(right) });
    }
    // Subtraction: skip leading minus (negative literal)
    if let Some(pos) = find_binary_minus(expr) {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+1..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Sub, right: Box::new(right) });
    }
    // Binary multiply: scan for * with non-empty left side (skip unary deref at pos 0)
    {
        let bytes = expr.as_bytes();
        let mut dp = 0i32; // depth paren
        let mut db = 0i32; // depth bracket
        let mut found_mul: Option<usize> = None;
        let mut i = 0usize;
        while i < expr.len() {
            match bytes[i] {
                b'(' => { dp += 1; i += 1; continue; }
                b')' => { dp -= 1; i += 1; continue; }
                b'[' => { db += 1; i += 1; continue; }
                b']' => { db -= 1; i += 1; continue; }
                _ => {}
            }
            if dp == 0 && db == 0 && bytes[i] == b'*' && i > 0 && !expr[..i].trim().is_empty() {
                found_mul = Some(i);
                break;
            }
            i += 1;
        }
        if let Some(pos) = found_mul {
            let left = Self::parse_expression(&expr[..pos])?;
            let right = Self::parse_expression(&expr[pos+1..])?;
            return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Mul, right: Box::new(right) });
        }
    }
    if let Some(pos) = find_operator_outside_parens(expr, "/") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+1..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Div, right: Box::new(right) });
    }
    if let Some(pos) = find_operator_outside_parens(expr, "%") {
        let left = Self::parse_expression(&expr[..pos])?;
        let right = Self::parse_expression(&expr[pos+1..])?;
        return Ok(CExpression::BinaryOp { left: Box::new(left), op: BinOp::Mod, right: Box::new(right) });
    }

    // ===============================
    // 8️⃣ Unary operators
    // ===============================
    if expr.starts_with('-') {
        let operand = Self::parse_expression(&expr[1..])?;
        return Ok(CExpression::UnaryOp { op: UnaryOp::Neg, operand: Box::new(operand) });
    }
    if expr.starts_with('!') {
        let operand = Self::parse_expression(&expr[1..])?;
        return Ok(CExpression::UnaryOp { op: UnaryOp::Not, operand: Box::new(operand) });
    }
    // Unary dereference: *x, **pp
    if expr.starts_with('*') {
        let operand = Self::parse_expression(&expr[1..])?;
        return Ok(CExpression::UnaryOp { op: UnaryOp::Deref, operand: Box::new(operand) });
    }

    // ===============================
    // 9️⃣ Array access: arr[i] or arr[i][j]
    // ===============================
    let arr_access_re = regex::Regex::new(r"^(\w+)\[([^\[\]]+)\]$").unwrap();
    if let Some(cap) = arr_access_re.captures(expr) {
        let array = CExpression::Variable(cap[1].to_string());
        let index = Self::parse_expression(&cap[2])?;
        return Ok(CExpression::ArrayAccess {
            array: Box::new(array),
            index: Box::new(index),
        });
    }

    // ===============================
    // 🔟 Function call: func(args)
    // ===============================
    let call_re = regex::Regex::new(r"^(\w+)\((.+)\)$").unwrap();
    if let Some(cap) = call_re.captures(expr) {
        let func_name = cap[1].to_string();
        let args_str = &cap[2];
        let args = if args_str.trim().is_empty() {
            vec![]
        } else {
            split_args(args_str)?
        };
        return Ok(CExpression::Call { function: func_name, args });
    }
    // Function call with no args: func()
    let call_empty_re = regex::Regex::new(r"^(\w+)\(\s*\)$").unwrap();
    if let Some(cap) = call_empty_re.captures(expr) {
        return Ok(CExpression::Call { function: cap[1].to_string(), args: vec![] });
    }

    // ===============================
    // 1️⃣1️⃣ Literals
    // ===============================
    if let Ok(num) = expr.parse::<i64>() {
        return Ok(CExpression::Literal(CLiteral::Int(num)));
    }
    if let Ok(num) = expr.parse::<f64>() {
        return Ok(CExpression::Literal(CLiteral::Float(num)));
    }
    // C float literals with suffix: 1.8f, 32.0F, 3.14l, 3.14L
    if expr.ends_with(|c: char| c == 'f' || c == 'F' || c == 'l' || c == 'L') {
        let stripped = &expr[..expr.len()-1];
        if let Ok(num) = stripped.parse::<f64>() {
            return Ok(CExpression::Literal(CLiteral::Float(num)));
        }
    }
    if expr.starts_with('\'') && expr.ends_with('\'') && expr.len() >= 3 {
        let ch = expr.chars().nth(1).unwrap_or('\0');
        return Ok(CExpression::Literal(CLiteral::Char(ch)));
    }
    if expr.starts_with('"') && expr.ends_with('"') {
        let s = expr[1..expr.len()-1].to_string();
        return Ok(CExpression::Literal(CLiteral::String(s)));
    }

    // ===============================
    // 1️⃣2️⃣ Variable fallback
    // ===============================
    Ok(CExpression::Variable(expr.to_string()))
}

    /// Parse a single simple statement (used by control flow parser)
    pub fn parse_single_statement(stmt: &str) -> Result<CStatement> {
        let stmt = stmt.trim();

        // Break / Continue
        if stmt == "break" { return Ok(CStatement::Break); }
        if stmt == "continue" { return Ok(CStatement::Continue); }

        // Return
        if stmt.starts_with("return") {
            let expr = stmt.trim_start_matches("return").trim();
            if expr.is_empty() {
                return Ok(CStatement::Return(None));
            } else {
                let parsed = Self::parse_expression(expr)?;
                return Ok(CStatement::Return(Some(parsed)));
            }
        }

        // Array declaration with initializer: int arr[] = {1,2,3}  OR  int arr[5] = {1,2,3}
        let arr_decl_init = regex::Regex::new(r"^(int|char|float|double)\s+(\w+)\[(\d*)\]\s*=\s*(.+)$").unwrap();
        if let Some(cap) = arr_decl_init.captures(stmt) {
            let base_type = Self::parse_type(&cap[1]);
            let name = cap[2].to_string();
            let size_str = &cap[3];
            let init_expr = Self::parse_expression(&cap[4])?;
            let size = size_str.parse::<usize>().ok();
            return Ok(CStatement::VarDecl {
                name,
                var_type: CType::Array(Box::new(base_type), size),
                init: Some(init_expr),
            });
        }

        // Array declaration without initializer: int arr[5]
        let arr_decl = regex::Regex::new(r"^(int|char|float|double)\s+(\w+)\[(\d+)\]$").unwrap();
        if let Some(cap) = arr_decl.captures(stmt) {
            let base_type = Self::parse_type(&cap[1]);
            let name = cap[2].to_string();
            let size = cap[3].parse::<usize>().ok();
            return Ok(CStatement::VarDecl {
                name,
                var_type: CType::Array(Box::new(base_type), size),
                init: None,
            });
        }

        // Pointer declaration: int *arr or int* arr  (e.g. for matrix rows)
        let ptr_decl = regex::Regex::new(r"^(int|char|float|double)\s*\*\s*(\w+)(?:\s*=\s*(.+))?$").unwrap();
        if let Some(cap) = ptr_decl.captures(stmt) {
            let base_type = Self::parse_type(&cap[1]);
            let name = cap[2].to_string();
            let init = if let Some(init_str) = cap.get(3) {
                Some(Self::parse_expression(init_str.as_str())?)
            } else { None };
            return Ok(CStatement::VarDecl {
                name,
                var_type: CType::Pointer(Box::new(base_type)),
                init,
            });
        }

        // Variable declaration with initialization: int x = expr
        let decl_pattern = regex::Regex::new(r"^(int|char|float|double)\s+(\w+)\s*=\s*(.+)$").unwrap();
        if let Some(cap) = decl_pattern.captures(stmt) {
            return Ok(CStatement::VarDecl {
                name: cap[2].to_string(),
                var_type: Self::parse_type(&cap[1]),
                init: Some(Self::parse_expression(&cap[3])?),
            });
        }

        // Variable declaration without initialization: int x
        let decl_simple = regex::Regex::new(r"^(int|char|float|double)\s+(\w+)$").unwrap();
        if let Some(cap) = decl_simple.captures(stmt) {
            return Ok(CStatement::VarDecl {
                name: cap[2].to_string(),
                var_type: Self::parse_type(&cap[1]),
                init: None,
            });
        }

        // Indexed assignment: arr[i] = expr  OR  arr[i][j] = expr
        let idx_assign = regex::Regex::new(r"^(\w+)\[([^\]]+)\]\s*=\s*(.+)$").unwrap();
        if let Some(cap) = idx_assign.captures(stmt) {
            let array = cap[1].to_string();
            let index = Self::parse_expression(&cap[2])?;
            let value = Self::parse_expression(&cap[3])?;
            return Ok(CStatement::IndexedAssignment { array, index, value });
        }

        // Compound assignments: x += y  x -= y  x *= y  x /= y  x %= y
        let compound_re = regex::Regex::new(r"^(\w+)\s*(\+=|-=|\*=|/=|%=)\s*(.+)$").unwrap();
        if let Some(cap) = compound_re.captures(stmt) {
            let var = cap[1].to_string();
            let op_str = &cap[2];
            let rhs = Self::parse_expression(&cap[3])?;
            let op = match op_str {
                "+=" => BinOp::Add,
                "-=" => BinOp::Sub,
                "*=" => BinOp::Mul,
                "/=" => BinOp::Div,
                "%=" => BinOp::Mod,
                _ => BinOp::Add,
            };
            // Desugar: x += y  →  Assignment { x, x + y }
            let expanded = CExpression::BinaryOp {
                left: Box::new(CExpression::Variable(var.clone())),
                op,
                right: Box::new(rhs),
            };
            return Ok(CStatement::Assignment { target: var, value: expanded });
        }

        // Increment/Decrement: i++, i--, ++i, --i
        if stmt.ends_with("++") {
            let var = stmt.trim_end_matches("++").trim().to_string();
            let expanded = CExpression::BinaryOp {
                left: Box::new(CExpression::Variable(var.clone())),
                op: BinOp::Add,
                right: Box::new(CExpression::Literal(CLiteral::Int(1))),
            };
            return Ok(CStatement::Assignment { target: var, value: expanded });
        }
        if stmt.ends_with("--") {
            let var = stmt.trim_end_matches("--").trim().to_string();
            let expanded = CExpression::BinaryOp {
                left: Box::new(CExpression::Variable(var.clone())),
                op: BinOp::Sub,
                right: Box::new(CExpression::Literal(CLiteral::Int(1))),
            };
            return Ok(CStatement::Assignment { target: var, value: expanded });
        }
        if stmt.starts_with("++") {
            let var = stmt.trim_start_matches("++").trim().to_string();
            let expanded = CExpression::BinaryOp {
                left: Box::new(CExpression::Variable(var.clone())),
                op: BinOp::Add,
                right: Box::new(CExpression::Literal(CLiteral::Int(1))),
            };
            return Ok(CStatement::Assignment { target: var, value: expanded });
        }
        if stmt.starts_with("--") {
            let var = stmt.trim_start_matches("--").trim().to_string();
            let expanded = CExpression::BinaryOp {
                left: Box::new(CExpression::Variable(var.clone())),
                op: BinOp::Sub,
                right: Box::new(CExpression::Literal(CLiteral::Int(1))),
            };
            return Ok(CStatement::Assignment { target: var, value: expanded });
        }

        // Plain Assignment: x = expr
        let assign_pattern = regex::Regex::new(r"^(\w+)\s*=\s*(.+)$").unwrap();
        if let Some(cap) = assign_pattern.captures(stmt) {
            return Ok(CStatement::Assignment {
                target: cap[1].to_string(),
                value: Self::parse_expression(&cap[2])?,
            });
        }

        // Fallback → treat as expression
        Ok(CStatement::Expression(Self::parse_expression(stmt)?))
    }

    /// Parse the initialization part of a for loop
    pub fn parse_for_init(init: &str) -> Result<CStatement> {
        Self::parse_single_statement(init)
    }

    /// Parse the increment part of a for loop
    /// Parse the increment part of a for loop
    pub fn parse_for_increment(inc: &str) -> Result<CExpression> {
        let trimmed = inc.trim();

        // Helper closure to build: var = var OP 1
        macro_rules! inc_expr {
            ($var:expr, $op:expr) => {{
                let var = $var.to_string();
                let rhs = CExpression::BinaryOp {
                    left: Box::new(CExpression::Variable(var.clone())),
                    op: $op,
                    right: Box::new(CExpression::Literal(CLiteral::Int(1))),
                };
                CExpression::BinaryOp {
                    left: Box::new(CExpression::Variable(var)),
                    op: BinOp::Assign,
                    right: Box::new(rhs),
                }
            }};
        }

        // Postfix: i++
        if trimmed.ends_with("++") && !trimmed.starts_with("++") {
            let var = trimmed.trim_end_matches("++").trim();
            return Ok(inc_expr!(var, BinOp::Add));
        }
        // Postfix: i--
        if trimmed.ends_with("--") && !trimmed.starts_with("--") {
            let var = trimmed.trim_end_matches("--").trim();
            return Ok(inc_expr!(var, BinOp::Sub));
        }
        // Prefix: ++i
        if trimmed.starts_with("++") {
            let var = trimmed.trim_start_matches("++").trim();
            return Ok(inc_expr!(var, BinOp::Add));
        }
        // Prefix: --i
        if trimmed.starts_with("--") {
            let var = trimmed.trim_start_matches("--").trim();
            return Ok(inc_expr!(var, BinOp::Sub));
        }

        Self::parse_expression(trimmed)
    }
}

fn extract_balanced_braces(code: &str, start_pos: usize) -> Result<(String, usize)> {
    let mut pos = start_pos + 1;
    let mut depth = 1;
    let content_start = pos;

    while pos < code.len() && depth > 0 {
        match code.chars().nth(pos) {
            Some('{') => depth += 1,
            Some('}') => depth -= 1,
            _ => {}
        }
        pos += 1;
    }

    if depth != 0 {
        return Err(anyhow::anyhow!("Mismatched braces in function body"));
    }

    let content = code[content_start..pos - 1].to_string();
    Ok((content, pos))
}

pub fn read_c_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .context(format!("Failed to read C file: {}", path.display()))
}
// ============================================================
// Helper: find an operator at the top level (outside parens/brackets)
// ============================================================
fn find_operator_outside_parens(expr: &str, op: &str) -> Option<usize> {
    let bytes = expr.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut i = 0usize;

    while i <= expr.len().saturating_sub(op.len()) {
        match bytes[i] {
            b'(' => { depth_paren += 1; i += 1; continue; }
            b')' => { depth_paren -= 1; i += 1; continue; }
            b'[' => { depth_bracket += 1; i += 1; continue; }
            b']' => { depth_bracket -= 1; i += 1; continue; }
            _ => {}
        }
        if depth_paren == 0 && depth_bracket == 0 {
            if expr[i..].starts_with(op) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn find_plain_assign_outside_parens(expr: &str) -> Option<usize> {
    let bytes = expr.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut i = 0usize;

    while i < expr.len() {
        match bytes[i] {
            b'(' => { depth_paren += 1; i += 1; continue; }
            b')' => { depth_paren -= 1; i += 1; continue; }
            b'[' => { depth_bracket += 1; i += 1; continue; }
            b']' => { depth_bracket -= 1; i += 1; continue; }
            _ => {}
        }
        if depth_paren == 0 && depth_bracket == 0 && bytes[i] == b'=' {
            let prev = if i > 0 { bytes[i-1] } else { 0 };
            let next = if i+1 < bytes.len() { bytes[i+1] } else { 0 };
            if prev != b'!' && prev != b'<' && prev != b'>' && prev != b'=' && next != b'=' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn find_binary_minus(expr: &str) -> Option<usize> {
    let bytes = expr.as_bytes();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut i = expr.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'(' => { depth_paren -= 1; continue; }
            b')' => { depth_paren += 1; continue; }
            b'[' => { depth_bracket -= 1; continue; }
            b']' => { depth_bracket += 1; continue; }
            _ => {}
        }
        if depth_paren == 0 && depth_bracket == 0 && bytes[i] == b'-' && i > 0 {
            let prev = bytes[i-1];
            if prev.is_ascii_alphanumeric() || prev == b')' || prev == b']' || prev == b'_' {
                return Some(i);
            }
        }
    }
    None
}

fn split_args(args_str: &str) -> Result<Vec<CExpression>> {
    let mut args = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in args_str.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                let arg = args_str[start..i].trim();
                if !arg.is_empty() {
                    args.push(SimpleParser::parse_expression(arg)?);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = args_str[start..].trim();
    if !last.is_empty() {
        args.push(SimpleParser::parse_expression(last)?);
    }
    Ok(args)
}