// crates/ast_input/src/control_flow.rs
// Dedicated module for parsing control flow structures

use anyhow::{Result, Context};
use crate::{CStatement, CExpression, SimpleParser};

pub struct ControlFlowParser;

impl ControlFlowParser {
    /// Main entry point: parse a function body and extract all statements
    pub fn parse_body(body: &str) -> Result<Vec<CStatement>> {
        let mut statements = Vec::new();
        let body_trimmed = body.trim();
        
        if body_trimmed.is_empty() {
            return Ok(statements);
        }
        
        let mut pos = 0;
        let bytes = body_trimmed.as_bytes();
        
        while pos < bytes.len() {
            // Skip whitespace
            pos = Self::skip_whitespace(body_trimmed, pos);
            
            if pos >= bytes.len() {
                break;
            }
            
            let remaining = &body_trimmed[pos..];
            
            // Try to parse control structures first
            if remaining.starts_with("if ") || remaining.starts_with("if(") {
                match Self::parse_if_statement(remaining) {
                    Ok((stmt, consumed)) => {
                        statements.push(stmt);
                        pos += consumed;
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to parse if statement: {}", e);
                        pos += 1; // Skip and continue
                        continue;
                    }
                }
            }
            
            if remaining.starts_with("while ") || remaining.starts_with("while(") {
                match Self::parse_while_loop(remaining) {
                    Ok((stmt, consumed)) => {
                        statements.push(stmt);
                        pos += consumed;
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to parse while loop: {}", e);
                        pos += 1;
                        continue;
                    }
                }
            }
            
            if remaining.starts_with("for ") || remaining.starts_with("for(") {
                match Self::parse_for_loop(remaining) {
                    Ok((stmt, consumed)) => {
                        statements.push(stmt);
                        pos += consumed;
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to parse for loop: {}", e);
                        pos += 1;
                        continue;
                    }
                }
            }
            
            // Try to parse a simple statement (until semicolon)
            if let Some(semi_pos) = remaining.find(';') {
                let stmt_text = remaining[..semi_pos].trim();
                
                if !stmt_text.is_empty() {
                    match SimpleParser::parse_single_statement(stmt_text) {
                        Ok(stmt) => statements.push(stmt),
                        Err(_) => {} // Skip unparseable statements
                    }
                }
                
                pos += semi_pos + 1;
            } else {
                // No semicolon, might be end of block or syntax error
                break;
            }
        }
        
        Ok(statements)
    }
    
    /// Parse: if (condition) { body } [else { body }]
    fn parse_if_statement(code: &str) -> Result<(CStatement, usize)> {
        let mut pos = 0;
        
        // Skip "if"
        if code.starts_with("if") {
            pos = 2;
        } else {
            return Err(anyhow::anyhow!("Expected 'if'"));
        }
        
        // Skip whitespace
        pos = Self::skip_whitespace(code, pos);
        
        // Parse condition: (...)
        if !Self::char_at(code, pos).map_or(false, |c| c == '(') {
            return Err(anyhow::anyhow!("Expected '(' after 'if'"));
        }
        
        let (condition_str, cond_end) = Self::extract_parentheses(code, pos)?;
        let condition = SimpleParser::parse_expression(&condition_str)?;
        pos = cond_end;
        
        // Skip whitespace
        pos = Self::skip_whitespace(code, pos);
        
        // Parse then block: { ... }
        if !Self::char_at(code, pos).map_or(false, |c| c == '{') {
            return Err(anyhow::anyhow!("Expected '{{' after if condition"));
        }
        
        let (then_body_str, then_end) = Self::extract_braces(code, pos)?;
        let then_body = Self::parse_body(&then_body_str)?;
        pos = then_end;
        
        // Skip whitespace
        pos = Self::skip_whitespace(code, pos);
        
        // Check for else
        let else_body = if code[pos..].starts_with("else") {
            pos += 4;
            pos = Self::skip_whitespace(code, pos);
            
            if Self::char_at(code, pos).map_or(false, |c| c == '{') {
                let (else_body_str, else_end) = Self::extract_braces(code, pos)?;
                let else_stmts = Self::parse_body(&else_body_str)?;
                pos = else_end;
                Some(else_stmts)
            } else {
                None
            }
        } else {
            None
        };
        
        Ok((CStatement::If { condition, then_body, else_body }, pos))
    }
    
    /// Parse: while (condition) { body }
    fn parse_while_loop(code: &str) -> Result<(CStatement, usize)> {
        let mut pos = 0;
        
        // Skip "while"
        if code.starts_with("while") {
            pos = 5;
        } else {
            return Err(anyhow::anyhow!("Expected 'while'"));
        }
        
        // Skip whitespace
        pos = Self::skip_whitespace(code, pos);
        
        // Parse condition: (...)
        if !Self::char_at(code, pos).map_or(false, |c| c == '(') {
            return Err(anyhow::anyhow!("Expected '(' after 'while'"));
        }
        
        let (condition_str, cond_end) = Self::extract_parentheses(code, pos)?;
        let condition = SimpleParser::parse_expression(&condition_str)?;
        pos = cond_end;
        
        // Skip whitespace
        pos = Self::skip_whitespace(code, pos);
        
        // Parse body: { ... }
        if !Self::char_at(code, pos).map_or(false, |c| c == '{') {
            return Err(anyhow::anyhow!("Expected '{{' after while condition"));
        }
        
        let (body_str, body_end) = Self::extract_braces(code, pos)?;
        let body = Self::parse_body(&body_str)?;
        pos = body_end;
        
        Ok((CStatement::While { condition, body }, pos))
    }
    
    /// Parse: for (init; condition; increment) { body }
    fn parse_for_loop(code: &str) -> Result<(CStatement, usize)> {
        let mut pos = 0;
        
        // Skip "for"
        if code.starts_with("for") {
            pos = 3;
        } else {
            return Err(anyhow::anyhow!("Expected 'for'"));
        }
        
        // Skip whitespace
        pos = Self::skip_whitespace(code, pos);
        
        // Parse (init; condition; increment)
        if !Self::char_at(code, pos).map_or(false, |c| c == '(') {
            return Err(anyhow::anyhow!("Expected '(' after 'for'"));
        }
        
        let (parts_str, parts_end) = Self::extract_parentheses(code, pos)?;
        pos = parts_end;
        
        // Split by semicolons
        let parts: Vec<&str> = parts_str.split(';').collect();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("For loop must have 3 parts separated by semicolons"));
        }
        
        let init = SimpleParser::parse_for_init(parts[0].trim())?;
        let condition = SimpleParser::parse_expression(parts[1].trim())?;
        let increment = SimpleParser::parse_for_increment(parts[2].trim())?;
        
        // Skip whitespace
        pos = Self::skip_whitespace(code, pos);
        
        // Parse body: { ... }
        if !Self::char_at(code, pos).map_or(false, |c| c == '{') {
            return Err(anyhow::anyhow!("Expected '{{' after for header"));
        }
        
        let (body_str, body_end) = Self::extract_braces(code, pos)?;
        let body = Self::parse_body(&body_str)?;
        pos = body_end;
        
        Ok((CStatement::For {
            init: Box::new(init),
            condition,
            increment,
            body,
        }, pos))
    }
    
    // === Helper Functions ===
    
    fn skip_whitespace(s: &str, mut pos: usize) -> usize {
        while pos < s.len() {
            match s.chars().nth(pos) {
                Some(c) if c.is_whitespace() => pos += 1,
                _ => break,
            }
        }
        pos
    }
    
    fn char_at(s: &str, pos: usize) -> Option<char> {
        s.chars().nth(pos)
    }
    
    /// Extract content between matching parentheses: (...)
    /// Returns (content, position_after_closing_paren)
    fn extract_parentheses(code: &str, start_pos: usize) -> Result<(String, usize)> {
        if Self::char_at(code, start_pos) != Some('(') {
            return Err(anyhow::anyhow!("Expected '(' at position {}", start_pos));
        }
        
        let mut pos = start_pos + 1;
        let mut depth = 1;
        let content_start = pos;
        
        while pos < code.len() && depth > 0 {
            match Self::char_at(code, pos) {
                Some('(') => depth += 1,
                Some(')') => depth -= 1,
                _ => {}
            }
            pos += 1;
        }
        
        if depth != 0 {
            return Err(anyhow::anyhow!("Mismatched parentheses starting at {}", start_pos));
        }
        
        let content = code[content_start..pos-1].to_string();
        Ok((content, pos))
    }
    
    /// Extract content between matching braces: {...}
    /// Returns (content, position_after_closing_brace)
    fn extract_braces(code: &str, start_pos: usize) -> Result<(String, usize)> {
        if Self::char_at(code, start_pos) != Some('{') {
            return Err(anyhow::anyhow!("Expected '{{' at position {}", start_pos));
        }
        
        let mut pos = start_pos + 1;
        let mut depth = 1;
        let content_start = pos;
        
        while pos < code.len() && depth > 0 {
            match Self::char_at(code, pos) {
                Some('{') => depth += 1,
                Some('}') => depth -= 1,
                _ => {}
            }
            pos += 1;
        }
        
        if depth != 0 {
            return Err(anyhow::anyhow!("Mismatched braces starting at {}", start_pos));
        }
        
        let content = code[content_start..pos-1].to_string();
        Ok((content, pos))
    }
}