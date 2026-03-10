// ast_input/src/validator.rs
//
// Lightweight structural validation for C source code.

use crate::types::ValidationError;

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
