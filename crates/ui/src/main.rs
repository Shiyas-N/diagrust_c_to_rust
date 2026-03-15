//! DIAGRUST GUI — eframe/egui frontend
//!
//! Features:
//!   • C source editor (type or open file)
//!   • Generated Rust viewer with syntax-highlighted copy
//!   • Run C code (via gcc if available)
//!   • Run translated Rust code
//!   • Translation report panel
//!   • Download generated Rust file

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::sync::{Arc, Mutex};
use std::thread;
use std::process::Command;

// Suppress flash of console window for each subprocess on Windows.
// On other platforms this is a no-op.
trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}
impl NoWindow for Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Translation result (shared across threads)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct TranslationResult {
    rust_code: String,
    report_text: String,
    validation_report: String,   // validation panel content
    error: Option<String>,
    c_output: Option<String>,
    rust_output: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// App state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Tab { Output, Report, Validation }

struct DiagRustApp {
    // Editor
    c_source: String,
    // Results (thread-safe)
    result: Arc<Mutex<Option<TranslationResult>>>,
    // UI state
    translating: bool,
    running_c: bool,
    running_rust: bool,
    validating: bool,
    strict_validate: bool,
    active_tab: Tab,
    // Status bar
    status: String,
    // Save path override
    save_path: String,
    // Max iterations
    max_iter: usize,
}

impl Default for DiagRustApp {
    fn default() -> Self {
        Self {
            c_source: DEFAULT_C.to_string(),
            result: Arc::new(Mutex::new(None)),
            translating: false,
            running_c: false,
            running_rust: false,
            validating: false,
            strict_validate: false,
            active_tab: Tab::Output,
            status: "Ready".to_string(),
            save_path: "output.rs".to_string(),
            max_iter: 10,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Translation worker (runs in background thread)
// ─────────────────────────────────────────────────────────────────────────────

fn translate_in_thread(c_source: String, max_iter: usize, result_slot: Arc<Mutex<Option<TranslationResult>>>) {
    thread::spawn(move || {
        let res = do_translate(&c_source, max_iter);
        *result_slot.lock().unwrap() = Some(res);
    });
}

/// Run C validation in a background thread, writing result into `validation_report`.
fn validate_c_in_thread(c_source: String, strict: bool, result_slot: Arc<Mutex<Option<TranslationResult>>>) {
    thread::spawn(move || {
        let report = do_validate_c(&c_source, strict);
        // Merge into existing result, or create a stub if none exists yet
        let mut guard = result_slot.lock().unwrap();
        if let Some(ref mut res) = *guard {
            res.validation_report = report;
        } else {
            *guard = Some(TranslationResult {
                validation_report: report,
                ..Default::default()
            });
        }
    });
}

fn do_validate_c(c_source: &str, strict: bool) -> String {

    let validator = input_validation::InputValidator::new(strict);
    match validator.validate_source(c_source, "input.c") {
        Ok(result) => format_validation_report(&result),
        Err(e)     => format!("Validation error: {}\n", e),
    }
}

fn format_validation_report(result: &input_validation::ValidatedSource) -> String {
    use input_validation::Severity;

    let mut buf = String::new();
    buf.push_str("═══════════════════════════════════════════════════\n");
    buf.push_str("          DIAGRUST C VALIDATION REPORT\n");
    buf.push_str("═══════════════════════════════════════════════════\n\n");

    let errors   = result.diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = result.diagnostics.iter().filter(|d| d.severity == Severity::Warning).count();
    let infos    = result.diagnostics.iter().filter(|d| d.severity == Severity::Info).count();

    let status = if errors > 0 {
        format!("✗  {} error(s) — fix before transpiling", errors)
    } else if warnings > 0 {
        format!("⚠  {} warning(s) — may affect output", warnings)
    } else {
        "✅  C source looks good — ready to transpile".to_string()
    };

    buf.push_str(&format!("Status:      {}\n", status));
    buf.push_str(&format!("Lines:       {}\n", result.line_count));
    buf.push_str(&format!("Complexity:  {}/100\n", result.complexity));
    buf.push_str(&format!("Errors:      {}  |  Warnings: {}  |  Info: {}\n\n", errors, warnings, infos));

    if !result.diagnostics.is_empty() {
        buf.push_str("───────────────────────────────────────────────────\n");
        buf.push_str("DIAGNOSTICS\n");
        buf.push_str("───────────────────────────────────────────────────\n");

        for d in &result.diagnostics {
            let icon = match d.severity {
                Severity::Error   => "✗ ERROR  ",
                Severity::Warning => "⚠ WARN   ",
                Severity::Info    => "ℹ INFO   ",
            };
            let line_str = if d.line > 0 { format!("line {:3} │ ", d.line) } else { "         │ ".to_string() };
            buf.push_str(&format!("  {}{}  {}\n", icon, line_str, d.message));
            if let Some(ref ctx) = d.context {
                buf.push_str(&format!("              │   ╰─ {}\n", ctx));
            }
            if !d.hint.is_empty() {
                buf.push_str(&format!("              │   💡 {}\n", d.hint));
            }
        }
    }

    if !result.features.is_empty() {
        buf.push_str("\n───────────────────────────────────────────────────\n");
        buf.push_str("FEATURES DETECTED\n");
        buf.push_str("───────────────────────────────────────────────────\n");
        let feat_list: Vec<_> = result.features.iter().collect();
        let row_count = (feat_list.len() + 2) / 3;
        for row in 0..row_count {
            let mut line_s = String::from("  ");
            for col in 0..3 {
                if let Some(f) = feat_list.get(row + col * row_count) {
                    line_s.push_str(&format!("  {:20}", f));
                }
            }
            buf.push_str(&line_s.trim_end());
            buf.push('\n');
        }
    }

    if !result.defines.is_empty() {
        buf.push_str("\n───────────────────────────────────────────────────\n");
        buf.push_str("PREPROCESSOR DEFINES\n");
        buf.push_str("───────────────────────────────────────────────────\n");
        for (k, v) in &result.defines {
            if v.is_empty() {
                buf.push_str(&format!("  #define {}\n", k));
            } else {
                buf.push_str(&format!("  #define {} = {}\n", k, v));
            }
        }
    }

    buf
}

fn do_translate(c_source: &str, max_iter: usize) -> TranslationResult {
    use semantic::SemanticAnalyser;
    use ir::IrBuilder;
    use codegen::CodeAssembler;
    use compiler_loop::CompilerLoop;

    // 1. Parse C source
    let program = match ast_input::ClangParser::parse(c_source) {
        Ok(p) => p,
        Err(e) => return TranslationResult { error: Some(format!("Parse error: {}", e)), ..Default::default() },
    };

    // 2. Semantic analysis
    let semantic = match SemanticAnalyser::analyse(&program) {
        Ok(s) => s,
        Err(e) => return TranslationResult { error: Some(format!("Semantic error: {}", e)), ..Default::default() },
    };

    // 3. IR building
    let ir = match IrBuilder::build(&program, &semantic) {
        Ok(i) => i,
        Err(e) => return TranslationResult { error: Some(format!("IR error: {}", e)), ..Default::default() },
    };

    // 4. Code generation
    let initial_rust = match CodeAssembler::generate(&ir) {
        Ok(r) => r,
        Err(e) => return TranslationResult { error: Some(format!("Codegen error: {}", e)), ..Default::default() },
    };

    // 5. Compiler-in-the-loop repair
    let loop_dir = std::env::temp_dir().join(format!("diagrust_loop_{}", std::process::id()));
    let cl = CompilerLoop::with_temp_dir(max_iter, loop_dir);
    let sem_summary = build_semantic_summary(&semantic);

    let (outcome, report) = match cl.run(
        &initial_rust, "input.c", "output.rs",
        c_source, sem_summary, vec![],
    ) {
        Ok(r) => r,
        Err(e) => return TranslationResult {
            rust_code: initial_rust,
            report_text: format!("Compiler loop error: {}", e),
            ..Default::default()
        },
    };

    let final_code = match &outcome {
        compiler_loop::LoopOutcome::Success { final_code, .. } => final_code.clone(),
        compiler_loop::LoopOutcome::MaxIterationsReached { best_code, .. } => best_code.clone(),
        compiler_loop::LoopOutcome::ConvergenceFailure { code, .. } => code.clone(),
        compiler_loop::LoopOutcome::NoCompiler { code } => code.clone(),
    };

    TranslationResult {
        rust_code: final_code,
        report_text: format_report(&report, &outcome),
        ..Default::default()
    }
}


fn build_semantic_summary(sem: &semantic::SemanticModel) -> reporting::SemanticSummary {
    use reporting::SemanticSummary;
    let mut summary = SemanticSummary::default();
    summary.functions_analysed = sem.functions.len();
    for fs in &sem.functions {
        for (name, kind) in &fs.ownership_map {
            summary.inferred_ownership.push((name.clone(), format!("{:?}", kind)));
        }
        if fs.memory_patterns.iter().any(|p| p.is_pointer) { summary.has_pointers = true; }
        if fs.memory_patterns.iter().any(|p| p.is_array) { summary.array_variables += 1; }
        summary.pointer_variables += fs.memory_patterns.iter().filter(|p| p.is_pointer).count();
        summary.def_use_pairs += fs.def_use_chains.values().map(|c| c.uses.len()).sum::<usize>();
        summary.has_loops = summary.has_loops || fs.cfg.has_loops;
    }
    summary
}

fn format_report(report: &reporting::TranslationReport, outcome: &compiler_loop::LoopOutcome) -> String {
    let m = &report.metrics;
    let status = match outcome {
        compiler_loop::LoopOutcome::Success { iterations, .. } =>
            format!("✅ Compilation SUCCESS on iteration {}", iterations),
        compiler_loop::LoopOutcome::MaxIterationsReached { last_error_count, iterations, .. } =>
            format!("⚠  Max iterations ({}) reached — {} error(s) remain", iterations, last_error_count),
        compiler_loop::LoopOutcome::ConvergenceFailure { message, .. } =>
            format!("✗ Convergence failure: {}", message),
        compiler_loop::LoopOutcome::NoCompiler { .. } =>
            "⚠  rustc not available — code generated without validation".to_string(),
    };

    let mut buf = String::new();
    buf.push_str("═══════════════════════════════════════════════════\n");
    buf.push_str("          DIAGRUST TRANSLATION REPORT\n");
    buf.push_str("═══════════════════════════════════════════════════\n\n");
    buf.push_str(&format!("Status:              {}\n", status));
    buf.push_str(&format!("Iterations used:     {} / {}\n", m.iterations_used, m.max_iterations));
    buf.push_str(&format!("Errors remaining:    {}\n", m.error_count));
    buf.push_str(&format!("Warnings remaining:  {}\n", m.warning_count));
    buf.push_str(&format!("Lines generated:     {}\n", m.lines_generated));
    buf.push_str(&format!("Unsafe blocks:       {}\n", m.unsafe_block_count));
    buf.push_str(&format!("Idiomaticity score:  {}/100\n", m.idiomatic_score));
    buf.push_str(&format!("Repairs applied:     {}\n", m.repairs_applied));

    if !report.repair_log.is_empty() {
        buf.push_str("\n───────────────────────────────────────────────────\n");
        buf.push_str("REPAIR LOG\n");
        buf.push_str("───────────────────────────────────────────────────\n");
        for entry in &report.repair_log {
            buf.push_str(&format!("  [iter {}] {:20} {}\n",
                entry.iteration, entry.rule_name, entry.description));
        }
    }

    let sem = &report.semantic_summary;
    buf.push_str("\n───────────────────────────────────────────────────\n");
    buf.push_str("SEMANTIC SUMMARY\n");
    buf.push_str("───────────────────────────────────────────────────\n");
    buf.push_str(&format!("  Functions analysed: {}\n", sem.functions_analysed));
    buf.push_str(&format!("  Pointer variables:  {}\n", sem.pointer_variables));
    buf.push_str(&format!("  Array variables:    {}\n", sem.array_variables));
    buf.push_str(&format!("  Def-use pairs:      {}\n", sem.def_use_pairs));
    buf.push_str(&format!("  Has pointers:       {}\n", sem.has_pointers));

    if !sem.inferred_ownership.is_empty() {
        buf.push_str("\n  Inferred ownership:\n");
        for (var, kind) in sem.inferred_ownership.iter().take(20) {
            buf.push_str(&format!("    {:20} → {}\n", var, kind));
        }
    }

    buf
}

// ─────────────────────────────────────────────────────────────────────────────
// Run helpers (compile + execute)
// ─────────────────────────────────────────────────────────────────────────────

fn run_c_code(c_source: &str, result_slot: Arc<Mutex<Option<TranslationResult>>>) {
    let c_src = c_source.to_string();
    thread::spawn(move || {
        let output = do_run_c(&c_src);
        if let Ok(mut guard) = result_slot.lock() {
            if let Some(res) = guard.as_mut() {
                res.c_output = Some(output);
            }
        }
    });
}

fn do_run_c(source: &str) -> String {
    let tmp = std::env::temp_dir().join(format!("diagrust_c_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let src = tmp.join("main.c");
    let bin = tmp.join("main_c");
    if std::fs::write(&src, source).is_err() { return "Failed to write C source".into(); }
    let compile = Command::new("gcc").no_window()
        .arg(&src).arg("-o").arg(&bin).arg("-lm")
        .output();
    match compile {
        Err(_) => "gcc not found — cannot run C code".into(),
        Ok(out) if !out.status.success() => {
            format!("Compile error:\n{}", String::from_utf8_lossy(&out.stderr))
        }
        Ok(_) => {
            match Command::new(&bin).no_window().output() {
                Err(e) => format!("Run error: {}", e),
                Ok(out) => {
                    let _ = std::fs::remove_dir_all(&tmp);
                    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
                }
            }
        }
    }
}

fn run_rust_code(rust_source: &str, result_slot: Arc<Mutex<Option<TranslationResult>>>) {
    let rust_src = rust_source.to_string();
    thread::spawn(move || {
        let output = do_run_rust(&rust_src);
        if let Ok(mut guard) = result_slot.lock() {
            if let Some(res) = guard.as_mut() {
                res.rust_output = Some(output);
            }
        }
    });
}

fn do_run_rust(source: &str) -> String {
    let tmp = std::env::temp_dir().join(format!("diagrust_rs_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let src = tmp.join("main.rs");
    let bin = tmp.join("main_rs");
    if std::fs::write(&src, source).is_err() { return "Failed to write Rust source".into(); }
    let compile = Command::new("rustc").no_window()
        .arg("--edition=2021").arg(&src).arg("-o").arg(&bin)
        .output();
    match compile {
        Err(_) => "rustc not found".into(),
        Ok(out) if !out.status.success() => {
            format!("Compile error:\n{}", String::from_utf8_lossy(&out.stderr))
        }
        Ok(_) => {
            match Command::new(&bin).no_window().output() {
                Err(e) => format!("Run error: {}", e),
                Ok(out) => {
                    let _ = std::fs::remove_dir_all(&tmp);
                    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// egui App implementation
// ─────────────────────────────────────────────────────────────────────────────

impl eframe::App for DiagRustApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll background thread result
        if self.translating {
            if let Ok(guard) = self.result.try_lock() {
                if guard.is_some() {
                    self.translating = false;
                    let res = guard.as_ref().unwrap();
                    if let Some(ref e) = res.error {
                        self.status = format!("Error: {}", e);
                    } else {
                        self.status = "Translation complete".to_string();
                    }
                    ctx.request_repaint();
                    drop(guard);
                }
            }
        }

        // Poll validation thread
        if self.validating {
            if let Ok(guard) = self.result.try_lock() {
                if guard.as_ref().map(|r| !r.validation_report.is_empty()).unwrap_or(false) {
                    self.validating = false;
                    self.status = "Validation complete".to_string();
                    ctx.request_repaint();
                }
                drop(guard);
            }
        }

        // ── Top toolbar ─────────────────────────────────────────────────────
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🦀 DIAGRUST");
                ui.separator();

                // Translate button
                let translate_label = if self.translating {
                    "⏳ Translating…"
                } else {
                    "⚙ Translate C → Rust"
                };
                if ui.add_enabled(!self.translating, egui::Button::new(translate_label)).clicked() {
                    self.translating = true;
                    self.status = "Translating…".to_string();
                    *self.result.lock().unwrap() = None;
                    translate_in_thread(self.c_source.clone(), self.max_iter, Arc::clone(&self.result));
                }

                ui.separator();

                // Validate C button
                let validate_label = if self.validating { "⏳ Validating…" } else { "🔍 Validate C" };
                if ui.add_enabled(!self.validating && !self.translating, egui::Button::new(validate_label))
                    .on_hover_text("Check C source for errors and warnings before transpiling")
                    .clicked()
                {
                    self.validating = true;
                    self.status = "Validating C source…".to_string();
                    // Clear previous validation report
                    if let Ok(mut g) = self.result.try_lock() {
                        if let Some(ref mut r) = *g { r.validation_report.clear(); }
                    }
                    validate_c_in_thread(self.c_source.clone(), self.strict_validate, Arc::clone(&self.result));
                    self.active_tab = Tab::Validation;
                };
                // ui.checkbox(&mut self.strict_validate, "Strict")
                //     .on_hover_text("Enable strict C validation (type mismatches, dangerous patterns)");

                ui.separator();

                // Run C button
                let can_run_c = !self.running_c && self.result.lock().unwrap().is_some();
                if ui.add_enabled(can_run_c, egui::Button::new("▶ Run C")).on_hover_text("Compile and run the C code with gcc").clicked() {
                    self.running_c = true;
                    self.status = "Running C…".to_string();
                    run_c_code(&self.c_source, Arc::clone(&self.result));
                    self.active_tab = Tab::Output;
                }

                // Run Rust button
                let rust_available = self.result.lock().unwrap().as_ref().map(|r| !r.rust_code.is_empty()).unwrap_or(false);
                let can_run_rust = !self.running_rust && rust_available;
                if ui.add_enabled(can_run_rust, egui::Button::new("▶ Run Rust")).on_hover_text("Compile and run the generated Rust code").clicked() {
                    self.running_rust = true;
                    self.status = "Running Rust…".to_string();
                    let rust_code = self.result.lock().unwrap().as_ref().map(|r| r.rust_code.clone()).unwrap_or_default();
                    run_rust_code(&rust_code, Arc::clone(&self.result));
                    self.active_tab = Tab::Output;
                }

                ui.separator();

                // Download button
                if ui.add_enabled(rust_available, egui::Button::new("💾 Save Rust")).clicked() {
                    if let Some(res) = &*self.result.lock().unwrap() {
                        if let Err(e) = std::fs::write(&self.save_path, &res.rust_code) {
                            self.status = format!("Save failed: {}", e);
                        } else {
                            self.status = format!("Saved → {}", self.save_path);
                        }
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut self.save_path).desired_width(140.0));

                ui.separator();

                // Open file
                if ui.button("📂 Open .c").clicked() {
                    if let Some(path) = rfd_pick_file() {
                        match std::fs::read_to_string(&path) {
                            Ok(src) => {
                                self.c_source = src;
                                self.status = format!("Opened: {}", path);
                            }
                            Err(e) => self.status = format!("Read error: {}", e),
                        }
                    }
                }

                ui.separator();
                ui.label("Max iterations:");
                ui.add(egui::DragValue::new(&mut self.max_iter).clamp_range(1..=50usize).speed(1));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(&self.status).color(status_color(ctx, &self.status)));
                });
            });
        });

        // ── Bottom panel: Output / Report / Validation tabs ──────────────────
        egui::TopBottomPanel::bottom("bottom_panel").resizable(true).min_height(120.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_tab, Tab::Output, "📺 Run Output");
                ui.selectable_value(&mut self.active_tab, Tab::Report, "📋 Translation Report");
                ui.selectable_value(&mut self.active_tab, Tab::Validation, "🔍 C Validation");
                if self.validating {
                    ui.spinner();
                }
            });
            ui.separator();

            let guard = self.result.lock().unwrap();
            match self.active_tab {
                Tab::Output => {
                    let text = if let Some(res) = &*guard {
                        let c_out = res.c_output.as_deref().unwrap_or("");
                        let r_out = res.rust_output.as_deref().unwrap_or("");
                        if c_out.is_empty() && r_out.is_empty() {
                            "No output yet. Press ▶ Run C or ▶ Run Rust.".to_string()
                        } else {
                            let mut s = String::new();
                            if !c_out.is_empty() {
                                s.push_str("── C Output ─────────────────────────\n");
                                s.push_str(c_out);
                                s.push('\n');
                            }
                            if !r_out.is_empty() {
                                s.push_str("── Rust Output ──────────────────────\n");
                                s.push_str(r_out);
                            }
                            s
                        }
                    } else {
                        "Translate first, then run.".to_string()
                    };
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut text.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                        );
                    });
                }
                Tab::Report => {
                    let text = if let Some(res) = &*guard {
                        if res.report_text.is_empty() { "No report yet.".to_string() }
                        else { res.report_text.clone() }
                    } else {
                        "Translate first to see the report.".to_string()
                    };
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut text.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                        );
                    });
                }
                Tab::Validation => {
                    let text = if self.validating {
                        "⏳ Running C validation (gcc -fsyntax-only)…".to_string()
                    } else if let Some(res) = &*guard {
                        if res.validation_report.is_empty() {
                            "Press 🔍 Validate C in the toolbar to check your C source.\n\
                             Validation runs gcc/clang -fsyntax-only plus static analysis.".to_string()
                        } else {
                            res.validation_report.clone()
                        }
                    } else {
                        "Press 🔍 Validate C in the toolbar to check your C source.\n\
                         Validation runs gcc/clang -fsyntax-only plus static analysis.".to_string()
                    };
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut text.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                        );
                    });
                }
            }
        });

        // ── Central area: split editor | generated Rust ──────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            let available = ui.available_size();
            let half_w = (available.x - 12.0) / 2.0;

            ui.horizontal_top(|ui| {
                // Left: C editor
                ui.vertical(|ui| {
                    ui.set_width(half_w);
                    ui.label(egui::RichText::new("C Source").strong().size(14.0));
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_source("c_editor")
                        .max_height(available.y - 60.0)
                        .show(ui, |ui: &mut egui::Ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.c_source)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(35)
                                    .hint_text("Paste or type your C code here…")
                                    .code_editor()
                            );
                        });
                });

                ui.separator();

                // Right: Generated Rust
                ui.vertical(|ui| {
                    ui.set_width(half_w);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Generated Rust").strong().size(14.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Copy button
                            let guard2 = self.result.lock().unwrap();
                            let has_code = guard2.as_ref().map(|r| !r.rust_code.is_empty()).unwrap_or(false);
                            if ui.add_enabled(has_code, egui::Button::new("📋 Copy")).clicked() {
                                if let Some(res) = &*guard2 {
                                    ui.output_mut(|o| o.copied_text = res.rust_code.clone());
                                    drop(guard2);
                                    self.status = "Copied to clipboard".to_string();
                                }
                            }
                        });
                    });
                    ui.separator();

                    let guard2 = self.result.lock().unwrap();
                    let rust_text = if let Some(res) = &*guard2 {
                        if let Some(ref e) = res.error {
                            format!("// Error during translation:\n// {}", e)
                        } else if res.rust_code.is_empty() {
                            "// No output yet — press ⚙ Translate C → Rust".to_string()
                        } else {
                            res.rust_code.clone()
                        }
                    } else if self.translating {
                        "// Translating…".to_string()
                    } else {
                        "// No output yet — press ⚙ Translate C → Rust".to_string()
                    };
                    drop(guard2);

                    egui::ScrollArea::vertical()
                        .id_source("rust_viewer")
                        .max_height(available.y - 60.0)
                        .show(ui, |ui: &mut egui::Ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut rust_text.as_str())
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(35)
                                    .code_editor()
                            );
                        });
                });
            });
        });

        // Repaint while busy
        if self.translating || self.running_c || self.running_rust {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            // Reset running flags when outputs appear
            if self.running_c {
                if let Ok(g) = self.result.try_lock() {
                    if g.as_ref().map(|r| r.c_output.is_some()).unwrap_or(false) {
                        self.running_c = false;
                        self.status = "C run complete".to_string();
                    }
                }
            }
            if self.running_rust {
                if let Ok(g) = self.result.try_lock() {
                    if g.as_ref().map(|r| r.rust_output.is_some()).unwrap_or(false) {
                        self.running_rust = false;
                        self.status = "Rust run complete".to_string();
                    }
                }
            }
        }
    }
}

fn status_color(ctx: &egui::Context, status: &str) -> egui::Color32 {
    if status.contains("Error") || status.contains("error") {
        egui::Color32::LIGHT_RED
    } else if status.contains("success") || status.contains("complete") || status.contains("Saved") || status.contains("Copied") {
        if ctx.style().visuals.dark_mode { egui::Color32::LIGHT_GREEN } else { egui::Color32::DARK_GREEN }
    } else if status.contains("…") || status.contains("ing") {
        egui::Color32::YELLOW
    } else {
        ctx.style().visuals.text_color()
    }
}

/// Minimal file-picker fallback (no rfd dependency — use a text input instead).
/// Returns the path the user typed / opened.
fn rfd_pick_file() -> Option<String> {
    // Use a simple file dialog via command line on Linux/macOS; fallback to None
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("zenity")
            .args(["--file-selection", "--file-filter=C files (*.c) | *.c"])
            .output().ok()?;
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() { return Some(path); }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("osascript")
            .args(["-e", "tell application \"Finder\" to POSIX path of (choose file of type {\"c\"})"])
            .output().ok()?;
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() { return Some(path); }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        // PowerShell file dialog
        let script = r#"Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Filter = 'C files|*.c'; if($d.ShowDialog() -eq 'OK'){Write-Output $d.FileName}"#;
        let out = std::process::Command::new("powershell")
            .args(["-Command", script]).output().ok()?;
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() { return Some(path); }
        }
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DIAGRUST — C to Rust Transpiler")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DIAGRUST",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::<DiagRustApp>::default()
        }),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Default C example
// ─────────────────────────────────────────────────────────────────────────────

const DEFAULT_C: &str = r#"#include <stdio.h>

void bubble_sort(int arr[], int n) {
    int i, j, temp;
    for (i = 0; i < n - 1; i++) {
        for (j = 0; j < n - i - 1; j++) {
            if (arr[j] > arr[j + 1]) {
                temp = arr[j];
                arr[j] = arr[j + 1];
                arr[j + 1] = temp;
            }
        }
    }
}

int main() {
    int arr[] = {64, 34, 25, 12, 22, 11, 90};
    int n = 7;
    bubble_sort(arr, n);
    printf("Sorted: ");
    for (int i = 0; i < n; i++) {
        printf("%d ", arr[i]);
    }
    printf("\n");
    return 0;
}
"#;
