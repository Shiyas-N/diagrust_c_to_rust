//! # Reporting Module (Process 0.5.3 – Output and Reporting)
//!
//! Generates the Translation Report returned to the user (SRS §4.6.9).
//! Computes quality metrics: unsafe block count, compilation success,
//! iteration count, repair summary.

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Metrics
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TranslationMetrics {
    pub compilation_success: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub iterations_used: usize,
    pub max_iterations: usize,
    pub unsafe_block_count: usize,
    pub repairs_applied: usize,
    pub functions_translated: usize,
    pub lines_generated: usize,
    pub idiomatic_score: u8, // 0-100
}

impl TranslationMetrics {
    /// Count `unsafe` occurrences in the generated code.
    pub fn count_unsafe(code: &str) -> usize {
        code.matches("unsafe").count()
    }

    /// Rough idiomaticity score (higher = better).
    pub fn compute_idiomatic_score(code: &str, unsafe_count: usize) -> u8 {
        let mut score: i32 = 100;
        score -= (unsafe_count as i32) * 15;
        if code.contains("*mut") { score -= 10; }
        if code.contains("*const") { score -= 10; }
        if code.contains("Box<") || code.contains("Vec<") || code.contains("Option<") {
            score += 10;
        }
        if code.contains("match ") { score += 5; }
        score.clamp(0, 100) as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Translation report
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TranslationReport {
    pub input_file: String,
    pub output_file: String,
    pub original_c_code: String,
    pub final_rust_code: String,
    pub metrics: TranslationMetrics,
    pub repair_log: Vec<RepairLogEntry>,
    pub semantic_summary: SemanticSummary,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RepairLogEntry {
    pub iteration: usize,
    pub rule_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct SemanticSummary {
    pub functions_analysed: usize,
    pub pointer_variables: usize,
    pub array_variables: usize,
    pub inferred_ownership: Vec<(String, String)>, // (var, ownership_kind)
    pub def_use_pairs: usize,
    pub has_control_flow: bool,
    pub has_loops: bool,
    pub has_branches: bool,
    pub has_pointers: bool,
}

impl TranslationReport {
    /// Render a human-readable report string.
    pub fn render(&self) -> String {
        let m = &self.metrics;
        let bar = "═".repeat(60);
        let mut out = String::new();

        out.push_str(&format!("\n{}\n", bar));
        out.push_str("  DIAGRUST — Translation Report\n");
        out.push_str(&format!("{}\n\n", bar));

        out.push_str(&format!("  Input  : {}\n", self.input_file));
        out.push_str(&format!("  Output : {}\n\n", self.output_file));

        out.push_str("  ┌─ Compilation ──────────────────────────────────────\n");
        out.push_str(&format!(
            "  │  Status     : {}\n",
            if m.compilation_success { "✓ SUCCESS" } else { "✗ FAILED" }
        ));
        out.push_str(&format!("  │  Errors     : {}\n", m.error_count));
        out.push_str(&format!("  │  Warnings   : {}\n", m.warning_count));
        out.push_str(&format!("  │  Iterations : {} / {}\n", m.iterations_used, m.max_iterations));
        out.push_str("  │\n");
        out.push_str("  ├─ Code Quality ─────────────────────────────────────\n");
        out.push_str(&format!("  │  Functions  : {}\n", m.functions_translated));
        out.push_str(&format!("  │  Lines      : {}\n", m.lines_generated));
        out.push_str(&format!("  │  Unsafe blk : {}\n", m.unsafe_block_count));
        out.push_str(&format!("  │  Idiomatic  : {}%\n", m.idiomatic_score));
        out.push_str("  │\n");
        out.push_str("  ├─ Semantic Analysis ────────────────────────────────\n");
        let s = &self.semantic_summary;
        out.push_str(&format!("  │  Functions  : {}\n", s.functions_analysed));
        out.push_str(&format!("  │  Pointers   : {}\n", s.pointer_variables));
        out.push_str(&format!("  │  Arrays     : {}\n", s.array_variables));
        out.push_str(&format!("  │  Def-Use pairs : {}\n", s.def_use_pairs));
        out.push_str(&format!("  │  Has loops  : {}\n", s.has_loops));
        out.push_str("  │\n");

        if !self.repair_log.is_empty() {
            out.push_str("  ├─ Repairs Applied ──────────────────────────────────\n");
            for entry in &self.repair_log {
                out.push_str(&format!(
                    "  │  [Iter {}] {}: {}\n",
                    entry.iteration, entry.rule_name, entry.description
                ));
            }
            out.push_str("  │\n");
        }

        if !self.warnings.is_empty() {
            out.push_str("  ├─ Warnings ─────────────────────────────────────────\n");
            for w in &self.warnings {
                out.push_str(&format!("  │  ⚠  {}\n", w));
            }
            out.push_str("  │\n");
        }

        out.push_str(&format!("{}\n", bar));
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Report builder
// ─────────────────────────────────────────────────────────────────────────────

pub struct ReportBuilder;

impl ReportBuilder {
    pub fn build(
        input_file: &str,
        output_file: &str,
        original_c: &str,
        final_rust: &str,
        iterations: usize,
        max_iterations: usize,
        compilation_success: bool,
        error_count: usize,
        warning_count: usize,
        repair_log: Vec<RepairLogEntry>,
        semantic_summary: SemanticSummary,
        warnings: Vec<String>,
    ) -> TranslationReport {
        let unsafe_count = TranslationMetrics::count_unsafe(final_rust);
        let idiomatic_score = TranslationMetrics::compute_idiomatic_score(final_rust, unsafe_count);
        let functions_translated = final_rust.matches("pub fn ").count();
        let lines_generated = final_rust.lines().count();

        let metrics = TranslationMetrics {
            compilation_success,
            error_count,
            warning_count,
            iterations_used: iterations,
            max_iterations,
            unsafe_block_count: unsafe_count,
            repairs_applied: repair_log.len(),
            functions_translated,
            lines_generated,
            idiomatic_score,
        };

        TranslationReport {
            input_file: input_file.to_string(),
            output_file: output_file.to_string(),
            original_c_code: original_c.to_string(),
            final_rust_code: final_rust.to_string(),
            metrics,
            repair_log,
            semantic_summary,
            warnings,
        }
    }
}
