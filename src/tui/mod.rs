pub mod ui;

use std::collections::HashSet;
use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::engine::autotag::enrich_entries_with_autotags;
use crate::engine::duplicates::{
    auto_select_redundant_ids, detect_duplicate_clusters,
};
use crate::engine::filter::{evaluate_smart_filter, BUILTIN_PRESETS};
use crate::engine::junk::scan_junk_buckets;
use crate::engine::treemap::{build_terminal_treemap, TreemapMode};
use crate::model::{
    format_bytes, AndroidDevice, DuplicateCluster, FileCategory, JunkBucket, ParsedNlpResult,
    SafetyLevel, SmartFilterRule, StorageEntry, StorageTargetKind, TreemapRect,
};
use crate::scanner::{delete_storage_entry, discover_all_android_devices};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    SmartFilter,
    Treemap,
    DuplicatesJunk,
    CleanupQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    EditingQuery,
    AddingCustomTag,
    ConfirmDelete { permanent: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupSubView {
    JunkBuckets,
    DuplicateClusters,
}

pub struct TuiApp {
    pub target_label: String,
    pub target_kind: StorageTargetKind,
    pub entries: Vec<StorageEntry>,
    pub now_unix: i64,

    pub active_tab: ActiveTab,
    pub input_mode: InputMode,
    pub tag_input_buffer: String,

    pub rule: SmartFilterRule,
    pub preset_index: Option<usize>,
    pub category_cycle_index: Option<usize>,

    // Cached computed views
    pub matched_indices: Vec<usize>,
    pub matched_bytes: u64,
    pub nlp_result: ParsedNlpResult,
    pub duplicate_clusters: Vec<DuplicateCluster>,
    pub junk_buckets: Vec<JunkBucket>,

    // Selection & cursor state
    pub filter_cursor: usize,
    pub treemap_mode: TreemapMode,
    pub treemap_cursor: usize,
    pub drilldown_category: Option<FileCategory>,
    pub drilldown_folder: Option<String>,
    pub last_treemap_rects: Vec<TreemapRect>,

    pub cleanup_subview: CleanupSubView,
    pub junk_cursor: usize,
    pub dup_cursor: usize,
    pub queue_cursor: usize,

    pub staged_ids: HashSet<usize>,
    pub reclaimed_bytes: u64,
    pub status_banner: String,
    pub android_devices: Vec<AndroidDevice>,
}

impl TuiApp {
    pub fn new(
        target_label: String,
        target_kind: StorageTargetKind,
        entries: Vec<StorageEntry>,
        initial_query: Option<String>,
    ) -> Self {
        let now_unix = chrono::Utc::now().timestamp();
        let mut rule = SmartFilterRule::default();
        if let Some(q) = initial_query {
            rule.search_query = q;
        }

        let mut app = Self {
            target_label,
            target_kind,
            entries,
            now_unix,
            active_tab: ActiveTab::SmartFilter,
            input_mode: InputMode::Normal,
            tag_input_buffer: String::new(),
            rule,
            preset_index: None,
            category_cycle_index: None,
            matched_indices: Vec::new(),
            matched_bytes: 0,
            nlp_result: ParsedNlpResult::default(),
            duplicate_clusters: Vec::new(),
            junk_buckets: Vec::new(),
            filter_cursor: 0,
            treemap_mode: TreemapMode::Category,
            treemap_cursor: 0,
            drilldown_category: None,
            drilldown_folder: None,
            last_treemap_rects: Vec::new(),
            cleanup_subview: CleanupSubView::JunkBuckets,
            junk_cursor: 0,
            dup_cursor: 0,
            queue_cursor: 0,
            staged_ids: HashSet::new(),
            reclaimed_bytes: 0,
            status_banner: String::from(
                "Press '/' to type a Natural Language filter, 'p' for Presets, 'Space' to stage files, 'Tab' to switch views.",
            ),
            android_devices: discover_all_android_devices(),
        };
        app.recompute_all();
        app
    }

    pub fn recompute_all(&mut self) {
        let (matched, bytes, nlp) =
            evaluate_smart_filter(&self.entries, &self.rule, self.now_unix);
        self.matched_indices = matched;
        self.matched_bytes = bytes;
        self.nlp_result = nlp;
        self.duplicate_clusters = detect_duplicate_clusters(&self.entries);
        self.junk_buckets = scan_junk_buckets(&self.entries, self.now_unix);

        if self.filter_cursor >= self.matched_indices.len() && !self.matched_indices.is_empty() {
            self.filter_cursor = self.matched_indices.len() - 1;
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size_bytes).sum()
    }

    pub fn staged_bytes(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| self.staged_ids.contains(&e.id))
            .map(|e| e.size_bytes)
            .sum()
    }

    pub fn safe_reclaimable_bytes(&self) -> u64 {
        let junk_safe: u64 = self
            .junk_buckets
            .iter()
            .filter(|b| b.safety == SafetyLevel::Safe)
            .map(|b| b.total_bytes)
            .sum();
        let dup_waste: u64 = self
            .duplicate_clusters
            .iter()
            .map(|c| c.wasted_bytes)
            .sum();
        junk_safe + dup_waste
    }

    pub fn highlighted_filter_entry(&self) -> Option<&StorageEntry> {
        let idx = *self.matched_indices.get(self.filter_cursor)?;
        self.entries.get(idx)
    }

    pub fn active_treemap_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if let Some(cat) = self.drilldown_category {
                    if e.category != cat {
                        return false;
                    }
                }
                if let Some(ref folder) = self.drilldown_folder {
                    if &e.parent_folder != folder {
                        return false;
                    }
                }
                true
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn update_treemap_rects(&mut self, width: u16, height: u16) {
        let active = self.active_treemap_indices();
        let effective_mode =
            if self.drilldown_category.is_some() || self.drilldown_folder.is_some() {
                TreemapMode::Files
            } else {
                self.treemap_mode
            };
        self.last_treemap_rects =
            build_terminal_treemap(&self.entries, &active, effective_mode, width, height, 12);
        if self.treemap_cursor >= self.last_treemap_rects.len()
            && !self.last_treemap_rects.is_empty()
        {
            self.treemap_cursor = self.last_treemap_rects.len() - 1;
        }
    }

    pub fn toggle_stage_entry_id(&mut self, id: usize) {
        if let Some(e) = self.entries.iter().find(|x| x.id == id) {
            if e.whitelisted {
                self.status_banner =
                    format!("'{}' is Whitelisted (protected). Press 'w' to unprotect first.", e.name);
                return;
            }
        }
        if self.staged_ids.contains(&id) {
            self.staged_ids.remove(&id);
        } else {
            self.staged_ids.insert(id);
        }
    }

    pub fn execute_staged_cleanup(&mut self, permanent: bool) {
        if self.staged_ids.is_empty() {
            self.status_banner = String::from("No files staged for cleanup.");
            return;
        }

        let to_delete: Vec<StorageEntry> = self
            .entries
            .iter()
            .filter(|e| self.staged_ids.contains(&e.id) && !e.whitelisted)
            .cloned()
            .collect();

        let mut deleted_ids = HashSet::new();
        let mut freed_bytes = 0u64;
        let mut errors = 0usize;

        for item in &to_delete {
            match delete_storage_entry(item, permanent) {
                Ok(()) => {
                    deleted_ids.insert(item.id);
                    freed_bytes += item.size_bytes;
                }
                Err(_) => {
                    errors += 1;
                }
            }
        }

        self.entries.retain(|e| !deleted_ids.contains(&e.id));
        for (new_idx, e) in self.entries.iter_mut().enumerate() {
            e.id = new_idx;
        }
        enrich_entries_with_autotags(&mut self.entries, self.now_unix);
        self.staged_ids.clear();
        self.reclaimed_bytes += freed_bytes;
        self.recompute_all();

        let mode_str = if permanent {
            "Permanently deleted"
        } else {
            "Moved to Recycle Bin / cleaned"
        };
        if errors == 0 {
            self.status_banner = format!(
                "{} {} files ({} reclaimed)!",
                mode_str,
                deleted_ids.len(),
                format_bytes(freed_bytes)
            );
        } else {
            self.status_banner = format!(
                "{} {} files ({}), {} failed (permissions/in-use).",
                mode_str,
                deleted_ids.len(),
                format_bytes(freed_bytes),
                errors
            );
        }
    }
}

pub fn run_interactive_tui(
    target_label: String,
    target_kind: StorageTargetKind,
    entries: Vec<StorageEntry>,
    initial_query: Option<String>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(target_label, target_kind, entries, initial_query);
    let res = run_event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw_dashboard(f, app))?;

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match app.input_mode {
                    InputMode::EditingQuery => match key.code {
                        KeyCode::Enter | KeyCode::Esc => {
                            app.input_mode = InputMode::Normal;
                            app.status_banner = format!(
                                "NLP Filter active: {} files matched ({})",
                                app.matched_indices.len(),
                                format_bytes(app.matched_bytes)
                            );
                        }
                        KeyCode::Backspace => {
                            app.rule.search_query.pop();
                            app.preset_index = None;
                            app.recompute_all();
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.rule.search_query.clear();
                            app.preset_index = None;
                            app.recompute_all();
                        }
                        KeyCode::Char(c) => {
                            app.rule.search_query.push(c);
                            app.preset_index = None;
                            app.recompute_all();
                        }
                        _ => {}
                    },

                    InputMode::AddingCustomTag => match key.code {
                        KeyCode::Esc => {
                            app.tag_input_buffer.clear();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Enter => {
                            let tag = app.tag_input_buffer.trim().trim_start_matches('#').to_string();
                            if !tag.is_empty() {
                                if let Some(&idx) = app.matched_indices.get(app.filter_cursor) {
                                    let entry = &mut app.entries[idx];
                                    if !entry.custom_tags.contains(&tag) {
                                        entry.custom_tags.push(tag.clone());
                                    }
                                    if tag.eq_ignore_ascii_case("keep") {
                                        entry.whitelisted = true;
                                        app.staged_ids.remove(&entry.id);
                                    }
                                    app.status_banner =
                                        format!("Added custom tag #{} to {}", tag, entry.name);
                                }
                            }
                            app.tag_input_buffer.clear();
                            app.input_mode = InputMode::Normal;
                            app.recompute_all();
                        }
                        KeyCode::Backspace => {
                            app.tag_input_buffer.pop();
                        }
                        KeyCode::Char(c) => {
                            app.tag_input_buffer.push(c);
                        }
                        _ => {}
                    },

                    InputMode::ConfirmDelete { permanent } => match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                            app.input_mode = InputMode::Normal;
                            app.execute_staged_cleanup(permanent);
                        }
                        _ => {
                            app.input_mode = InputMode::Normal;
                            app.status_banner = String::from("Cleanup cancelled.");
                        }
                    },

                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(())
                        }
                        KeyCode::Char('1') => app.active_tab = ActiveTab::SmartFilter,
                        KeyCode::Char('2') => app.active_tab = ActiveTab::Treemap,
                        KeyCode::Char('3') => app.active_tab = ActiveTab::DuplicatesJunk,
                        KeyCode::Char('4') => app.active_tab = ActiveTab::CleanupQueue,
                        KeyCode::Tab => {
                            app.active_tab = match app.active_tab {
                                ActiveTab::SmartFilter => ActiveTab::Treemap,
                                ActiveTab::Treemap => ActiveTab::DuplicatesJunk,
                                ActiveTab::DuplicatesJunk => ActiveTab::CleanupQueue,
                                ActiveTab::CleanupQueue => ActiveTab::SmartFilter,
                            };
                        }
                        KeyCode::Char('/') => {
                            app.active_tab = ActiveTab::SmartFilter;
                            app.input_mode = InputMode::EditingQuery;
                            app.status_banner = String::from(
                                "Type natural language query (e.g. 'old videos over 100mb') and press Enter.",
                            );
                        }
                        KeyCode::Char('d') => {
                            if !app.staged_ids.is_empty() {
                                app.input_mode = InputMode::ConfirmDelete { permanent: false };
                            } else {
                                app.status_banner =
                                    String::from("Stage files with Space or 'x' before cleaning.");
                            }
                        }
                        KeyCode::Char('D') => {
                            if !app.staged_ids.is_empty() {
                                app.input_mode = InputMode::ConfirmDelete { permanent: true };
                            }
                        }
                        _ => handle_tab_specific_keys(app, key.code),
                    },
                }
            }
        }
    }
}

fn handle_tab_specific_keys(app: &mut TuiApp, code: KeyCode) {
    match app.active_tab {
        ActiveTab::SmartFilter => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                if app.filter_cursor + 1 < app.matched_indices.len() {
                    app.filter_cursor += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.filter_cursor = app.filter_cursor.saturating_sub(1);
            }
            KeyCode::PageDown => {
                app.filter_cursor = (app.filter_cursor + 10)
                    .min(app.matched_indices.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                app.filter_cursor = app.filter_cursor.saturating_sub(10);
            }
            KeyCode::Char(' ') => {
                if let Some(&idx) = app.matched_indices.get(app.filter_cursor) {
                    let id = app.entries[idx].id;
                    app.toggle_stage_entry_id(id);
                }
            }
            KeyCode::Char('a') => {
                let selectable: Vec<usize> = app
                    .matched_indices
                    .iter()
                    .filter(|&&i| !app.entries[i].whitelisted)
                    .map(|&i| app.entries[i].id)
                    .collect();
                let all_selected =
                    !selectable.is_empty() && selectable.iter().all(|id| app.staged_ids.contains(id));
                if all_selected {
                    for id in selectable {
                        app.staged_ids.remove(&id);
                    }
                    app.status_banner = String::from("Unstaged all matched files.");
                } else {
                    let count = selectable.len();
                    for id in selectable {
                        app.staged_ids.insert(id);
                    }
                    app.status_banner = format!("Staged all {} matched files for cleanup.", count);
                }
            }
            KeyCode::Char('w') => {
                if let Some(&idx) = app.matched_indices.get(app.filter_cursor) {
                    let entry = &mut app.entries[idx];
                    entry.whitelisted = !entry.whitelisted;
                    let id = entry.id;
                    let name = entry.name.clone();
                    let protected = entry.whitelisted;
                    if protected {
                        app.staged_ids.remove(&id);
                        app.status_banner = format!("Protected '{}' on Safe List.", name);
                    } else {
                        app.status_banner = format!("Removed '{}' from Safe List.", name);
                    }
                }
            }
            KeyCode::Char('t') => {
                if !app.matched_indices.is_empty() {
                    app.input_mode = InputMode::AddingCustomTag;
                    app.tag_input_buffer.clear();
                }
            }
            KeyCode::Char('p') => {
                let next_p = match app.preset_index {
                    None => 0,
                    Some(i) => (i + 1) % BUILTIN_PRESETS.len(),
                };
                app.preset_index = Some(next_p);
                let preset = &BUILTIN_PRESETS[next_p];
                app.rule = SmartFilterRule {
                    search_query: preset.nl_query.to_string(),
                    ..Default::default()
                };
                app.filter_cursor = 0;
                app.recompute_all();
                app.status_banner = format!("Preset [{}]: {}", preset.name, preset.description);
            }
            KeyCode::Char('c') => {
                let next_c = match app.category_cycle_index {
                    None => Some(0),
                    Some(i) if i + 1 < FileCategory::ALL.len() => Some(i + 1),
                    Some(_) => None,
                };
                app.category_cycle_index = next_c;
                app.rule.categories = match next_c {
                    Some(i) => vec![FileCategory::ALL[i]],
                    None => Vec::new(),
                };
                app.filter_cursor = 0;
                app.recompute_all();
            }
            KeyCode::Char('s') => {
                app.rule.sort_by = app.rule.sort_by.next();
                app.recompute_all();
                app.status_banner = format!("Sorted by: {}", app.rule.sort_by.label());
            }
            KeyCode::Char('r') => {
                app.rule = SmartFilterRule::default();
                app.preset_index = None;
                app.category_cycle_index = None;
                app.filter_cursor = 0;
                app.recompute_all();
                app.status_banner = String::from("Cleared all smart filters.");
            }
            _ => {}
        },

        ActiveTab::Treemap => match code {
            KeyCode::Char('g') => {
                app.drilldown_category = None;
                app.drilldown_folder = None;
                app.treemap_mode = app.treemap_mode.next();
                app.treemap_cursor = 0;
                app.status_banner = format!("Treemap grouping: {}", app.treemap_mode.label());
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('l') => {
                if app.treemap_cursor + 1 < app.last_treemap_rects.len() {
                    app.treemap_cursor += 1;
                }
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('h') => {
                app.treemap_cursor = app.treemap_cursor.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(rect) = app.last_treemap_rects.get(app.treemap_cursor).cloned() {
                    if let Some(entry_id) = rect.entry_id {
                        app.toggle_stage_entry_id(entry_id);
                    } else if let Some(cat) = rect.category {
                        app.drilldown_category = Some(cat);
                        app.treemap_cursor = 0;
                        app.status_banner = format!("Drilled down into category: {}", cat.label());
                    } else if let Some(folder) = rect.folder {
                        app.drilldown_folder = Some(folder.clone());
                        app.treemap_cursor = 0;
                        app.status_banner = format!("Drilled down into folder: {}", folder);
                    }
                }
            }
            KeyCode::Backspace | KeyCode::Esc => {
                app.drilldown_category = None;
                app.drilldown_folder = None;
                app.treemap_cursor = 0;
                app.status_banner = String::from("Reset treemap zoom to full storage.");
            }
            KeyCode::Char(' ') => {
                if let Some(rect) = app.last_treemap_rects.get(app.treemap_cursor).cloned() {
                    if let Some(entry_id) = rect.entry_id {
                        app.toggle_stage_entry_id(entry_id);
                    }
                }
            }
            _ => {}
        },

        ActiveTab::DuplicatesJunk => match code {
            KeyCode::Char('v') => {
                app.cleanup_subview = match app.cleanup_subview {
                    CleanupSubView::JunkBuckets => CleanupSubView::DuplicateClusters,
                    CleanupSubView::DuplicateClusters => CleanupSubView::JunkBuckets,
                };
            }
            KeyCode::Down | KeyCode::Char('j') => match app.cleanup_subview {
                CleanupSubView::JunkBuckets => {
                    if app.junk_cursor + 1 < app.junk_buckets.len() {
                        app.junk_cursor += 1;
                    }
                }
                CleanupSubView::DuplicateClusters => {
                    if app.dup_cursor + 1 < app.duplicate_clusters.len() {
                        app.dup_cursor += 1;
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => match app.cleanup_subview {
                CleanupSubView::JunkBuckets => {
                    app.junk_cursor = app.junk_cursor.saturating_sub(1);
                }
                CleanupSubView::DuplicateClusters => {
                    app.dup_cursor = app.dup_cursor.saturating_sub(1);
                }
            },
            KeyCode::Char(' ') => match app.cleanup_subview {
                CleanupSubView::JunkBuckets => {
                    if let Some(bucket) = app.junk_buckets.get(app.junk_cursor) {
                        let ids = bucket.entry_ids.clone();
                        let all_in = ids.iter().all(|id| app.staged_ids.contains(id));
                        if all_in {
                            for id in ids {
                                app.staged_ids.remove(&id);
                            }
                        } else {
                            for id in ids {
                                app.staged_ids.insert(id);
                            }
                        }
                    }
                }
                CleanupSubView::DuplicateClusters => {
                    if let Some(cluster) = app.duplicate_clusters.get(app.dup_cursor) {
                        let redundant: Vec<usize> = cluster
                            .entry_ids
                            .iter()
                            .copied()
                            .filter(|&id| id != cluster.keep_id)
                            .collect();
                        let all_in = redundant.iter().all(|id| app.staged_ids.contains(id));
                        if all_in {
                            for id in redundant {
                                app.staged_ids.remove(&id);
                            }
                        } else {
                            for id in redundant {
                                app.staged_ids.insert(id);
                            }
                        }
                    }
                }
            },
            KeyCode::Char('x') => match app.cleanup_subview {
                CleanupSubView::JunkBuckets => {
                    let safe_ids: Vec<usize> = app
                        .junk_buckets
                        .iter()
                        .filter(|b| b.safety == SafetyLevel::Safe)
                        .flat_map(|b| b.entry_ids.iter().copied())
                        .collect();
                    let count = safe_ids.len();
                    for id in safe_ids {
                        app.staged_ids.insert(id);
                    }
                    app.status_banner = format!("Auto-staged {} Safe Junk files!", count);
                }
                CleanupSubView::DuplicateClusters => {
                    let redundant =
                        auto_select_redundant_ids(&app.duplicate_clusters, &app.entries);
                    let count = redundant.len();
                    for id in redundant {
                        app.staged_ids.insert(id);
                    }
                    app.status_banner = format!(
                        "Auto-staged {} redundant duplicate copies (keeping 1 original per group)!",
                        count
                    );
                }
            },
            _ => {}
        },

        ActiveTab::CleanupQueue => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                if app.queue_cursor + 1 < app.staged_ids.len() {
                    app.queue_cursor += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.queue_cursor = app.queue_cursor.saturating_sub(1);
            }
            KeyCode::Char('c') => {
                app.staged_ids.clear();
                app.queue_cursor = 0;
                app.status_banner = String::from("Cleared staged cleanup basket.");
            }
            _ => {}
        },
    }
}
