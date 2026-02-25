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
    Unknown(String),
}

#[derive(Debug, Clone)]
pub enum CStatement {
    Return(Option<CExpression>),
    VarDecl { name: String, var_type: CType, init: Option<CExpression> },
    Assignment { target: String, value: CExpression },
    Expression(CExpression),
    If { condition: CExpression, then_body: Vec<CStatement>, else_body: Option<Vec<CStatement>> },
    While { condition: CExpression, body: Vec<CStatement> },
    For { init: Box<CStatement>, condition: CExpression, increment: CExpression, body: Vec<CStatement> },
}

#[derive(Debug, Clone)]
pub enum CExpression {
    Literal(CLiteral),
    Variable(String),
    BinaryOp { left: Box<CExpression>, op: BinOp, right: Box<CExpression> },
    UnaryOp { op: UnaryOp, operand: Box<CExpression> },
    Call { function: String, args: Vec<CExpression> },
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
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            
            // Check for missing semicolons after statements
            // Look for lines that should end with ; but don't
            if self.should_have_semicolon(trimmed) && !trimmed.ends_with(';') && !trimmed.ends_with('{') && !trimmed.ends_with('}') {
                // Check if next line might be continuation
                if line_num + 1 < lines.len() {
                    let next_line = lines[line_num + 1].trim();
                    // If next line doesn't start with these, likely missing semicolon
                    if !next_line.starts_with('{') && !next_line.is_empty() {
                        return Err(ValidationError::SyntaxError {
                            line: line_num + 1,
                            message: format!("Possible missing semicolon: '{}'", trimmed),
                        });
                    }
                }
            }
            
            // Check for missing closing brace in context
            if trimmed.contains("return") && !trimmed.ends_with(';') && !trimmed.ends_with('}') {
                return Err(ValidationError::SyntaxError {
                    line: line_num + 1,
                    message: format!("Missing semicolon after return statement: '{}'", trimmed),
                });
            }
        }
        
        Ok(())
    }
    
    fn should_have_semicolon(&self, line: &str) -> bool {
        // Statements that should end with semicolon
        let statement_keywords = ["return", "int", "char", "float", "double", "void"];
        
        // Check if line starts with a statement keyword or looks like an assignment
        for keyword in &statement_keywords {
            if line.starts_with(keyword) {
                return true;
            }
        }
        
        // Check for assignment pattern (variable = ...)
        if line.contains('=') && !line.contains("==") {
            return true;
        }
        
        // Check for function calls (word followed by parentheses)
        if line.contains('(') && line.contains(')') && !line.contains("if") && !line.contains("while") && !line.contains("for") {
            // Could be function declaration or call
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
        
        // Very simple parser for MVP - handles basic function patterns
        let function_pattern = regex::Regex::new(
            r"(?m)^\s*(int|void|char|float|double)\s+(\w+)\s*\(([^)]*)\)\s*\{([^}]*)\}"
        ).unwrap();
        
        for cap in function_pattern.captures_iter(source) {
            let return_type = Self::parse_type(&cap[1]);
            let name = cap[2].to_string();
            let params_str = &cap[3];
            let body_str = &cap[4];
            
            let parameters = Self::parse_parameters(params_str)?;
            let body = Self::parse_body(body_str)?;
            
            functions.push(CFunction {
                name,
                return_type,
                parameters,
                body,
            });
        }
        
        Ok(CProgram { functions })
    }
    
    fn parse_type(type_str: &str) -> CType {
        match type_str.trim() {
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
        
        for param in params_str.split(',') {
            let parts: Vec<&str> = param.trim().split_whitespace().collect();
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
        
        // Integer literal
        if let Ok(num) = expr.parse::<i64>() {
            return Ok(CExpression::Literal(CLiteral::Int(num)));
        }
        
        // Binary operations
        if expr.contains('+') {
            let parts: Vec<&str> = expr.splitn(2, '+').collect();
            if parts.len() == 2 {
                let left = Self::parse_expression(parts[0])?;
                let right = Self::parse_expression(parts[1])?;
                return Ok(CExpression::BinaryOp {
                    left: Box::new(left),
                    op: BinOp::Add,
                    right: Box::new(right),
                });
            }
        }
        
        if expr.contains('-') && !expr.starts_with('-') {
            let parts: Vec<&str> = expr.splitn(2, '-').collect();
            if parts.len() == 2 {
                let left = Self::parse_expression(parts[0])?;
                let right = Self::parse_expression(parts[1])?;
                return Ok(CExpression::BinaryOp {
                    left: Box::new(left),
                    op: BinOp::Sub,
                    right: Box::new(right),
                });
            }
        }
        
        if expr.contains('*') {
            let parts: Vec<&str> = expr.splitn(2, '*').collect();
            if parts.len() == 2 {
                let left = Self::parse_expression(parts[0])?;
                let right = Self::parse_expression(parts[1])?;
                return Ok(CExpression::BinaryOp {
                    left: Box::new(left),
                    op: BinOp::Mul,
                    right: Box::new(right),
                });
            }
        }
        
        // Variable
        if expr.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Ok(CExpression::Variable(expr.to_string()));
        }
        
        // Default: treat as variable
        Ok(CExpression::Variable(expr.to_string()))
    }
}

pub fn read_c_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .context(format!("Failed to read C file: {}", path.display()))
}