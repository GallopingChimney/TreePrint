#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod search;

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use std::{fs, path::PathBuf};

fn main() -> eframe::Result {
    eframe::run_native(
        "Tree Print",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(App::default()))),
    )
}

fn reveal_in_explorer(path: &Path) {
    let _ = Command::new("explorer")
        .arg("/select,")
        .arg(path)
        .spawn();
}

// -- Shared helpers --

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

fn format_size_padded(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    match bytes {
        b if b >= GB => format!("{:>6.1} GB", b as f64 / GB as f64),
        b if b >= MB => format!("{:>6.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:>6.1} KB", b as f64 / KB as f64),
        b => format!("{:>6} B ", b),
    }
}

/// Green(0) → Yellow(mid) → Red(upper), log + power-curve scale
fn size_color(bytes: u64, upper: u64) -> egui::Color32 {
    if bytes == 0 || upper == 0 {
        return egui::Color32::from_rgb(80, 200, 80);
    }
    // Normalized log ratio with power curve — keeps small files green/yellow
    // even when the upper bound is very high
    let ratio = (bytes as f64).ln() / (upper as f64).ln();
    let t = ratio.clamp(0.0, 1.0).powf(2.5) as f32;

    if t <= 0.5 {
        // Green → Yellow
        let s = t * 2.0;
        egui::Color32::from_rgb(
            (80.0 + 175.0 * s) as u8,
            (200.0 + 20.0 * s) as u8,
            (80.0 - 80.0 * s) as u8,
        )
    } else {
        // Yellow → Red
        let s = (t - 0.5) * 2.0;
        egui::Color32::from_rgb(
            255,
            (220.0 - 220.0 * s) as u8,
            0,
        )
    }
}

fn format_time(t: &SystemTime) -> String {
    let dur = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;

    // Minimal datetime formatting without chrono
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;

    // Days since 1970-01-01
    let mut y = 1970i32;
    let mut remaining = days;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let months_days: &[i64] = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 0;
    for (i, &d) in months_days.iter().enumerate() {
        if remaining < d { mo = i + 1; break; }
        remaining -= d;
    }
    let day = remaining + 1;

    format!("{y:04}-{mo:02}-{day:02} {h:02}:{m:02}")
}

fn line_count(path: &Path) -> Option<usize> {
    let content = fs::read(path).ok()?;
    if content.iter().take(512).any(|&b| b == 0) {
        return None;
    }
    Some(content.iter().filter(|&&b| b == b'\n').count())
}

// -- Filters --

#[derive(Clone)]
struct Filters {
    hide_hidden: bool,
    hide_git: bool,
    hide_node_modules: bool,
    hide_target: bool,
    hide_build: bool,
    hide_obj_files: bool,
    hide_lock_files: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            hide_hidden: true,
            hide_git: true,
            hide_node_modules: true,
            hide_target: true,
            hide_build: false,
            hide_obj_files: false,
            hide_lock_files: false,
        }
    }
}

impl Filters {
    fn should_skip(&self, name: &str, is_dir: bool) -> bool {
        if self.hide_hidden && name.starts_with('.') {
            return true;
        }
        if is_dir {
            if self.hide_git && name == ".git" { return true; }
            if self.hide_node_modules && name == "node_modules" { return true; }
            if self.hide_target && name == "target" { return true; }
            if self.hide_build && (name == "build" || name == "dist" || name == "out") {
                return true;
            }
        } else {
            if self.hide_obj_files
                && matches!(
                    Path::new(name).extension().and_then(|e| e.to_str()),
                    Some("o" | "obj" | "pdb" | "exe" | "dll" | "so" | "dylib")
                )
            {
                return true;
            }
            if self.hide_lock_files
                && matches!(
                    name,
                    "Cargo.lock"
                        | "package-lock.json"
                        | "yarn.lock"
                        | "pnpm-lock.yaml"
                        | "bun.lockb"
                )
            {
                return true;
            }
        }
        false
    }
}

// -- Tree state --

struct TreeLine {
    text: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
}

struct TreeState {
    lines: Vec<TreeLine>,
    root_display: String,
    files: usize,
    dirs: usize,
    total_bytes: u64,
    handle: Option<TreeHandle>,
    done: bool,
    dirty: bool,
}

struct TreeHandle {
    rx: Receiver<TreeLine>,
    cancel: Arc<AtomicBool>,
}

impl Default for TreeState {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            root_display: String::new(),
            files: 0,
            dirs: 0,
            total_bytes: 0,
            handle: None,
            done: true,
            dirty: true,
        }
    }
}

impl TreeState {
    fn start(&mut self, dir: &Path, filters: &Filters, show_lines: bool) {
        if let Some(h) = &self.handle {
            h.cancel.store(true, Ordering::Relaxed);
        }
        self.lines.clear();
        self.root_display = dir.display().to_string();
        self.files = 0;
        self.dirs = 0;
        self.total_bytes = 0;
        self.done = false;
        self.dirty = false;

        if !dir.is_dir() {
            self.root_display = "Not a valid directory".into();
            self.done = true;
            return;
        }

        let (tx, rx) = unbounded::<TreeLine>();
        let cancel = Arc::new(AtomicBool::new(false));
        let dir = dir.to_path_buf();
        let filters = filters.clone();
        let cancel_clone = cancel.clone();

        std::thread::spawn(move || {
            build_tree(&dir, "", &tx, &filters, show_lines, &cancel_clone);
        });

        self.handle = Some(TreeHandle { rx, cancel });
    }

    fn poll(&mut self) {
        let Some(h) = &self.handle else { return };
        for _ in 0..5000 {
            match h.rx.try_recv() {
                Ok(line) => {
                    if line.is_dir {
                        self.dirs += 1;
                    } else {
                        self.files += 1;
                        self.total_bytes += line.size;
                    }
                    self.lines.push(line);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.done = true;
                    self.handle = None;
                    break;
                }
            }
        }
    }

    fn to_copyable_text(&self) -> String {
        let mut out = format!("{}\n", self.root_display);
        for line in &self.lines {
            out.push_str(&line.text);
        }
        out
    }
}

// -- Search state --

struct SearchState {
    query: String,
    handle: Option<search::SearchHandle>,
    results: Vec<search::SearchResult>,
    done: bool,
    started_at: Option<Instant>,
    elapsed_ms: f64,
    case_sensitive: bool,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            handle: None,
            results: Vec::new(),
            done: true,
            started_at: None,
            elapsed_ms: 0.0,
            case_sensitive: false,
        }
    }
}

impl SearchState {
    fn start(&mut self, root: &Path) {
        if let Some(h) = &self.handle {
            h.cancel.store(true, Ordering::Relaxed);
        }
        self.results.clear();
        self.done = false;
        self.started_at = Some(Instant::now());
        self.elapsed_ms = 0.0;
        self.handle = Some(search::search(root, &self.query, self.case_sensitive));
    }

    fn poll(&mut self) {
        if let Some(h) = &self.handle {
            search::drain_results(h, &mut self.results);
            if let Some(t) = self.started_at {
                self.elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
            }
            if search::is_done(h) {
                self.done = true;
                self.handle = None;
            }
        }
    }
}

// -- App --

#[derive(PartialEq)]
enum Tab { Tree, Search }

struct App {
    dir: String,
    filters: Filters,
    show_lines: bool,
    tab: Tab,
    tree: TreeState,
    search: SearchState,
    size_upper_mb: f64, // upper bound in MB for size color ramp
}

impl Default for App {
    fn default() -> Self {
        let dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut app = Self {
            dir,
            filters: Filters::default(),
            show_lines: true,
            tab: Tab::Tree,
            tree: TreeState::default(),
            search: SearchState::default(),
            size_upper_mb: 2048.0, // 2 GB default
        };
        app.tree.start(Path::new(&app.dir), &app.filters, app.show_lines);
        app
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.tree.done {
            self.tree.poll();
            ctx.request_repaint();
        }
        if !self.search.done {
            self.search.poll();
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut dir_changed = false;
            ui.horizontal(|ui| {
                ui.label("Directory:");
                if ui.text_edit_singleline(&mut self.dir).lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    dir_changed = true;
                }
                if ui.button("Browse…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.dir = path.to_string_lossy().into_owned();
                        dir_changed = true;
                    }
                }
            });

            if dir_changed {
                if self.tab == Tab::Tree {
                    self.tree.start(Path::new(&self.dir), &self.filters, self.show_lines);
                } else {
                    self.tree.dirty = true;
                }
            }

            ui.horizontal(|ui| {
                if ui.selectable_label(self.tab == Tab::Tree, "Tree").clicked() && self.tab != Tab::Tree {
                    self.tab = Tab::Tree;
                    if self.tree.dirty {
                        self.tree.start(Path::new(&self.dir), &self.filters, self.show_lines);
                    }
                }
                if ui.selectable_label(self.tab == Tab::Search, "Search").clicked() {
                    self.tab = Tab::Search;
                }
            });

            ui.separator();

            match self.tab {
                Tab::Tree => self.draw_tree(ui),
                Tab::Search => self.draw_search(ui),
            }
        });
    }
}

impl App {
    fn draw_tree(&mut self, ui: &mut egui::Ui) {
        let f = &mut self.filters;
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Hide:");
            changed |= ui.checkbox(&mut f.hide_hidden, "dotfiles").changed();
            changed |= ui.checkbox(&mut f.hide_git, ".git").changed();
            changed |= ui.checkbox(&mut f.hide_node_modules, "node_modules").changed();
            changed |= ui.checkbox(&mut f.hide_target, "target").changed();
            changed |= ui.checkbox(&mut f.hide_build, "build/dist/out").changed();
            changed |= ui.checkbox(&mut f.hide_obj_files, "binaries").changed();
            changed |= ui.checkbox(&mut f.hide_lock_files, "lockfiles").changed();
            ui.separator();
            changed |= ui.checkbox(&mut self.show_lines, "line counts").changed();
        });
        if changed {
            self.tree.start(Path::new(&self.dir), &self.filters, self.show_lines);
        }

        ui.horizontal(|ui| {
            ui.label(format!(
                "{} files, {} folders, {}",
                self.tree.files, self.tree.dirs, format_size(self.tree.total_bytes),
            ));
            if !self.tree.done {
                ui.spinner();
            }
            ui.separator();
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(self.tree.to_copyable_text());
            }
        });

        ui.separator();

        let hover_bg = egui::Color32::from_rgb(100, 160, 255).gamma_multiply(0.15);
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
        let total = self.tree.lines.len() + 1; // +1 for root label
        egui::ScrollArea::vertical().show_rows(ui, row_h, total, |ui, row_range| {
            ui.set_min_width(ui.available_width());
            for i in row_range {
                if i == 0 {
                    ui.label(egui::RichText::new(&self.tree.root_display).monospace());
                    continue;
                }
                let line = &self.tree.lines[i - 1];
                let text = line.text.trim_end_matches('\n');
                let outer = ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), row_h),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(text).monospace())
                            .selectable(true)
                            .sense(egui::Sense::click()))
                    },
                );
                let resp = outer.response;
                if resp.hovered() {
                    ui.painter().rect_filled(resp.rect, 0.0, hover_bg);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    reveal_in_explorer(&line.path);
                }
            }
        });
    }

    fn draw_search(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label("Find:");
            changed |= ui.text_edit_singleline(&mut self.search.query).changed();
            changed |= ui.checkbox(&mut self.search.case_sensitive, "Aa").changed();
            ui.separator();
            ui.label("Size Colors:");
            ui.add(egui::Slider::new(&mut self.size_upper_mb, (1.0 / 1024.0)..=4096.0)
                .logarithmic(true)
                .custom_formatter(|v, _| {
                    if v >= 1024.0 { format!("{:.1} GB", v / 1024.0) }
                    else if v >= 1.0 { format!("{:.0} MB", v) }
                    else { format!("{:.0} KB", v * 1024.0) }
                }));
        });

        if changed {
            if self.search.query.is_empty() {
                if let Some(h) = &self.search.handle {
                    h.cancel.store(true, Ordering::Relaxed);
                }
                self.search.results.clear();
                self.search.done = true;
                self.search.handle = None;
            } else {
                self.search.start(Path::new(&self.dir));
            }
        }

        ui.horizontal(|ui| {
            let count = self.search.results.len();
            if self.search.done {
                ui.label(format!("{count} results in {:.1}ms", self.search.elapsed_ms));
            } else {
                ui.label(format!("{count} results… {:.0}ms", self.search.elapsed_ms));
                ui.spinner();
            }
            if !self.search.results.is_empty() {
                ui.separator();
                if ui.button("Copy paths").clicked() {
                    let text: String = self.search.results.iter()
                        .map(|r| r.path.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(text);
                }
            }
        });

        ui.separator();

        let root = PathBuf::from(&self.dir);
        let query = self.search.query.clone();
        let case_sensitive = self.search.case_sensitive;
        let is_glob = search::is_glob(&query);
        let size_upper = (self.size_upper_mb * 1024.0 * 1024.0) as u64;
        let highlight_color = egui::Color32::from_rgb(86, 156, 214);
        let hover_bg = egui::Color32::from_rgb(100, 160, 255).gamma_multiply(0.15);
        let mono_id = egui::TextStyle::Monospace.resolve(ui.style());
        let text_color = ui.visuals().text_color();
        let dim_color = ui.visuals().weak_text_color();

        let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
        let total = self.search.results.len();
        egui::ScrollArea::vertical().show_rows(ui, row_h, total, |ui, row_range| {
            ui.set_min_width(ui.available_width());
            let mono = egui::TextFormat {
                font_id: mono_id.clone(),
                color: text_color,
                ..Default::default()
            };

            for i in row_range {
                let result = &self.search.results[i];
                let display = result.path.strip_prefix(&root)
                    .unwrap_or(&result.path)
                    .to_string_lossy();
                let label = if result.is_dir {
                    format!("{display}/")
                } else {
                    display.into_owned()
                };

                let resp = ui.horizontal(|ui| {
                    // Path — left-aligned
                    let mut job = egui::text::LayoutJob::default();
                    if is_glob {
                        job.append(&label, 0.0, mono.clone());
                    } else {
                        append_highlighted(&mut job, &label, &query, case_sensitive, &mono, highlight_color);
                    }
                    ui.add(egui::Label::new(job).selectable(true));

                    // Size + Date — right-aligned to window edge
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(accessed) = &result.accessed {
                            ui.add(egui::Label::new(egui::RichText::new(format_time(accessed))
                                .monospace()
                                .color(dim_color)));
                        }
                        if !result.is_dir {
                            ui.add(egui::Label::new(egui::RichText::new(format_size_padded(result.size))
                                .monospace()
                                .color(size_color(result.size, size_upper))));
                        }
                    });
                }).response;

                if resp.hovered() {
                    ui.painter().rect_filled(resp.rect, 0.0, hover_bg);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.interact(egui::Sense::click()).clicked() {
                    reveal_in_explorer(&result.path);
                }
            }
        });
    }
}

fn append_highlighted(
    job: &mut egui::text::LayoutJob,
    text: &str,
    query: &str,
    case_sensitive: bool,
    base: &egui::TextFormat,
    color: egui::Color32,
) {
    let highlighted = egui::TextFormat { color, ..base.clone() };
    let haystack = if case_sensitive { text.to_string() } else { text.to_lowercase() };
    let needle = if case_sensitive { query.to_string() } else { query.to_lowercase() };

    let mut pos = 0;
    while let Some(idx) = haystack[pos..].find(&needle) {
        let start = pos + idx;
        if start > pos {
            job.append(&text[pos..start], 0.0, base.clone());
        }
        job.append(&text[start..start + needle.len()], 0.0, highlighted.clone());
        pos = start + needle.len();
    }
    if pos < text.len() {
        job.append(&text[pos..], 0.0, base.clone());
    }
}

// -- Background tree builder --

fn build_tree(dir: &Path, prefix: &str, tx: &crossbeam_channel::Sender<TreeLine>, filters: &Filters, show_lines: bool, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) { return; }

    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    let entries: Vec<_> = entries
        .into_iter()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            !filters.should_skip(&name, e.path().is_dir())
        })
        .collect();

    let count = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) { return; }

        let last = i == count - 1;
        let connector = if last { "└── " } else { "├── " };
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.path().is_dir();
        let path = entry.path();

        if is_dir {
            let _ = tx.send(TreeLine {
                text: format!("{prefix}{connector}{name}/\n"),
                path,
                is_dir: true,
                size: 0,
            });
            let ext = if last { "    " } else { "│   " };
            build_tree(&entry.path(), &format!("{prefix}{ext}"), tx, filters, show_lines, cancel);
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let text = if show_lines {
                if let Some(lines) = line_count(&entry.path()) {
                    format!("{prefix}{connector}{name} ({lines} lines)\n")
                } else {
                    format!("{prefix}{connector}{name}\n")
                }
            } else {
                format!("{prefix}{connector}{name}\n")
            };
            let _ = tx.send(TreeLine { text, path, is_dir: false, size });
        }
    }
}
