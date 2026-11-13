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
use crate::engine::duplicates::{auto_select_redundant_ids, detect_duplicate_clusters};
use crate::engine::filter::evaluate_smart_filter;
use crate::engine::junk::scan_junk_buckets;
use crate::engine::treemap::{build_terminal_treemap, TreemapMode};
use crate::model::{
    format_bytes, AndroidDevice, AutoTag, DuplicateCluster, FileCategory, JunkBucket,
    ParsedNlpResult, SafetyLevel, SmartFilterRule, StorageEntry, StorageTargetKind, TreemapRect,
};
use crate::scanner::{delete_storage_entry, discover_all_android_devices};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Sidebar,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    AllFiles,
    VisualMap,
    Duplicates,
    TempAndJunk,
    OldAndUnused,
    LargeFiles,
    Installers,
    ChatMedia,
    Screenshots,
}

impl SidebarItem {
    pub const ALL: [SidebarItem; 9] = [
        Self::AllFiles,
        Self::VisualMap,
        Self::Duplicates,
        Self::TempAndJunk,
        Self::OldAndUnused,
        Self::LargeFiles,
        Self::Installers,
        Self::ChatMedia,
        Self::Screenshots,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Self::AllFiles => "All Files",
            Self::VisualMap => "Visual Map",
            Self::Duplicates => "Duplicates",
            Self::TempAndJunk => "Temp & Junk",
            Self::OldAndUnused => "Old & Unused",
            Self::LargeFiles => "Large (>100 MB)",
            Self::Installers => "Apps & Setup",
            Self::ChatMedia => "Chat Media",
            Self::Screenshots => "Screenshots",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalState {
    None,
    QuickActions {
        action_cursor: usize,
        card_cursor: Option<usize>,
    },
    ConfirmClean {
        permanent: bool,
    },
}

pub const QUICK_CARDS: [(FileCategory, &str); 5] = [
    (FileCategory::Video, "Videos"),
    (FileCategory::Image, "Photos"),
    (FileCategory::Installer, "Apps"),
    (FileCategory::Document, "Docs"),
    (FileCategory::CacheTemp, "Temp"),
];

pub struct TuiApp {
    pub target_label: String,
    pub target_kind: StorageTargetKind,
    pub entries: Vec<StorageEntry>,
    pub now_unix: i64,

    pub focus: FocusPane,
    pub sidebar_cursor: usize,
    pub active_sidebar: SidebarItem,
    pub modal: ModalState,

    pub rule: SmartFilterRule,

    // Cached computed state
    pub matched_indices: Vec<usize>,
    pub matched_bytes: u64,
    pub nlp_result: ParsedNlpResult,
    pub duplicate_clusters: Vec<DuplicateCluster>,
    pub junk_buckets: Vec<JunkBucket>,

    // Main list & Visual Map cursors
    pub list_cursor: usize,
    pub treemap_mode: TreemapMode,
    pub treemap_cursor: usize,
    pub drilldown_category: Option<FileCategory>,
    pub drilldown_folder: Option<String>,
    pub last_treemap_rects: Vec<TreemapRect>,

    pub staged_ids: HashSet<usize>,
    pub reclaimed_bytes: u64,
    pub notice: Option<String>,
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
            focus: FocusPane::Main,
            sidebar_cursor: 0,
            active_sidebar: SidebarItem::AllFiles,
            modal: ModalState::None,
            rule,
            matched_indices: Vec::new(),
            matched_bytes: 0,
            nlp_result: ParsedNlpResult::default(),
            duplicate_clusters: Vec::new(),
            junk_buckets: Vec::new(),
            list_cursor: 0,
            treemap_mode: TreemapMode::Category,
            treemap_cursor: 0,
            drilldown_category: None,
            drilldown_folder: None,
            last_treemap_rects: Vec::new(),
            staged_ids: HashSet::new(),
            reclaimed_bytes: 0,
            notice: None,
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

        if self.list_cursor >= self.matched_indices.len() && !self.matched_indices.is_empty() {
            self.list_cursor = self.matched_indices.len() - 1;
        }
    }

    pub fn apply_sidebar_item(&mut self, item: SidebarItem) {
        self.active_sidebar = item;
        let sort = self.rule.sort_by;
        let mut next_rule = SmartFilterRule {
            sort_by: sort,
            ..Default::default()
        };

        match item {
            SidebarItem::AllFiles | SidebarItem::VisualMap => {}
            SidebarItem::Duplicates => {
                next_rule.only_duplicates = true;
            }
            SidebarItem::TempAndJunk => {
                next_rule.only_junk = true;
            }
            SidebarItem::OldAndUnused => {
                next_rule.min_age_days = Some(120);
            }
            SidebarItem::LargeFiles => {
                next_rule.min_size_bytes = Some(100 * 1024 * 1024);
            }
            SidebarItem::Installers => {
                next_rule.categories = vec![FileCategory::Installer];
            }
            SidebarItem::ChatMedia => {
                next_rule.auto_tags = vec![AutoTag::MessengerMedia];
            }
            SidebarItem::Screenshots => {
                next_rule.auto_tags = vec![AutoTag::Screenshot, AutoTag::ScreenRecording];
            }
        }

        self.rule = next_rule;
        self.list_cursor = 0;
        self.recompute_all();
    }

    pub fn sidebar_item_bytes(&self, item: SidebarItem) -> u64 {
        match item {
            SidebarItem::AllFiles | SidebarItem::VisualMap => self.total_bytes(),
            SidebarItem::Duplicates => self
                .duplicate_clusters
                .iter()
                .map(|c| c.wasted_bytes)
                .sum(),
            SidebarItem::TempAndJunk => self
                .entries
                .iter()
                .filter(|e| {
                    e.category == FileCategory::CacheTemp
                        || e.category == FileCategory::Installer
                        || e.auto_tags.contains(&AutoTag::CacheTemp)
                        || e.auto_tags.contains(&AutoTag::Installer)
                        || e.auto_tags.contains(&AutoTag::DevArtifact)
                })
                .map(|e| e.size_bytes)
                .sum(),
            SidebarItem::OldAndUnused => self
                .entries
                .iter()
                .filter(|e| e.days_old(self.now_unix) >= 120)
                .map(|e| e.size_bytes)
                .sum(),
            SidebarItem::LargeFiles => self
                .entries
                .iter()
                .filter(|e| e.size_bytes >= 100 * 1024 * 1024)
                .map(|e| e.size_bytes)
                .sum(),
            SidebarItem::Installers => self
                .entries
                .iter()
                .filter(|e| {
                    e.category == FileCategory::Installer
                        || e.auto_tags.contains(&AutoTag::Installer)
                })
                .map(|e| e.size_bytes)
                .sum(),
            SidebarItem::ChatMedia => self
                .entries
                .iter()
                .filter(|e| e.auto_tags.contains(&AutoTag::MessengerMedia))
                .map(|e| e.size_bytes)
                .sum(),
            SidebarItem::Screenshots => self
                .entries
                .iter()
                .filter(|e| {
                    e.auto_tags.contains(&AutoTag::Screenshot)
                        || e.auto_tags.contains(&AutoTag::ScreenRecording)
                })
                .map(|e| e.size_bytes)
                .sum(),
        }
    }

    pub fn category_bytes(&self, cat: FileCategory) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.category == cat)
            .map(|e| e.size_bytes)
            .sum()
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
            build_terminal_treemap(&self.entries, &active, effective_mode, width, height, 10);
        if self.treemap_cursor >= self.last_treemap_rects.len()
            && !self.last_treemap_rects.is_empty()
        {
            self.treemap_cursor = self.last_treemap_rects.len() - 1;
        }
    }

    pub fn toggle_stage_entry_id(&mut self, id: usize) {
        if let Some(e) = self.entries.iter().find(|x| x.id == id) {
            if e.whitelisted {
                self.notice = Some(format!("'{}' is marked Keep safe ★", e.name));
                return;
            }
        }
        if self.staged_ids.contains(&id) {
            self.staged_ids.remove(&id);
        } else {
            self.staged_ids.insert(id);
        }
    }

    pub fn select_all_duplicates(&mut self) {
        let ids = auto_select_redundant_ids(&self.duplicate_clusters, &self.entries);
        let count = ids.len();
        for id in ids {
            self.staged_ids.insert(id);
        }
        self.notice = Some(format!("Selected {} duplicate copies", count));
    }

    pub fn select_safe_junk(&mut self) {
        let safe_ids: Vec<usize> = self
            .junk_buckets
            .iter()
            .filter(|b| b.safety == SafetyLevel::Safe)
            .flat_map(|b| b.entry_ids.iter().copied())
            .collect();
        let count = safe_ids.len();
        for id in safe_ids {
            self.staged_ids.insert(id);
        }
        self.notice = Some(format!("Selected {} safe junk files", count));
    }

    pub fn execute_staged_cleanup(&mut self, permanent: bool) {
        if self.staged_ids.is_empty() {
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

        for item in &to_delete {
            if delete_storage_entry(item, permanent).is_ok() {
                deleted_ids.insert(item.id);
                freed_bytes += item.size_bytes;
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

        self.notice = Some(format!(
            "Cleaned {} files ({})",
            deleted_ids.len(),
            format_bytes(freed_bytes)
        ));
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

                match app.modal {
                    ModalState::QuickActions {
                        mut action_cursor,
                        mut card_cursor,
                    } => match key.code {
                        KeyCode::Esc => {
                            app.modal = ModalState::None;
                        }
                        KeyCode::Down => {
                            card_cursor = None;
                            action_cursor = (action_cursor + 1).min(4);
                            app.modal = ModalState::QuickActions {
                                action_cursor,
                                card_cursor,
                            };
                        }
                        KeyCode::Up => {
                            if action_cursor == 0 {
                                card_cursor = Some(card_cursor.unwrap_or(0));
                            } else {
                                action_cursor -= 1;
                            }
                            app.modal = ModalState::QuickActions {
                                action_cursor,
                                card_cursor,
                            };
                        }
                        KeyCode::Right => {
                            let next = match card_cursor {
                                Some(c) => (c + 1) % QUICK_CARDS.len(),
                                None => 0,
                            };
                            app.modal = ModalState::QuickActions {
                                action_cursor,
                                card_cursor: Some(next),
                            };
                        }
                        KeyCode::Left => {
                            let prev = match card_cursor {
                                Some(0) | None => QUICK_CARDS.len() - 1,
                                Some(c) => c - 1,
                            };
                            app.modal = ModalState::QuickActions {
                                action_cursor,
                                card_cursor: Some(prev),
                            };
                        }
                        KeyCode::Backspace => {
                            app.rule.search_query.pop();
                            app.active_sidebar = SidebarItem::AllFiles;
                            app.recompute_all();
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.rule.search_query.clear();
                            app.recompute_all();
                        }
                        KeyCode::Enter => {
                            if let Some(c_idx) = card_cursor {
                                let (cat, _) = QUICK_CARDS[c_idx];
                                app.active_sidebar = SidebarItem::AllFiles;
                                app.rule = SmartFilterRule {
                                    categories: vec![cat],
                                    sort_by: app.rule.sort_by,
                                    ..Default::default()
                                };
                                app.list_cursor = 0;
                                app.recompute_all();
                            } else {
                                match action_cursor {
                                    0 => {
                                        // Apply current typed search filter and focus list
                                        app.active_sidebar = SidebarItem::AllFiles;
                                        app.focus = FocusPane::Main;
                                        app.recompute_all();
                                    }
                                    1 => app.select_all_duplicates(),
                                    2 => app.select_safe_junk(),
                                    3 => {
                                        app.rule.sort_by = app.rule.sort_by.next();
                                        app.recompute_all();
                                    }
                                    4 => {
                                        app.staged_ids.clear();
                                        app.notice = Some("Cleared selection".to_string());
                                    }
                                    _ => {}
                                }
                            }
                            app.modal = ModalState::None;
                        }
                        KeyCode::Char(c) => {
                            card_cursor = None;
                            action_cursor = 0;
                            app.rule.search_query.push(c);
                            app.active_sidebar = SidebarItem::AllFiles;
                            app.recompute_all();
                            app.modal = ModalState::QuickActions {
                                action_cursor,
                                card_cursor,
                            };
                        }
                        _ => {}
                    },

                    ModalState::ConfirmClean { permanent } => match key.code {
                        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            app.modal = ModalState::None;
                            app.execute_staged_cleanup(permanent);
                        }
                        _ => {
                            app.modal = ModalState::None;
                        }
                    },

                    ModalState::None => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(())
                        }
                        KeyCode::Char('/') => {
                            app.modal = ModalState::QuickActions {
                                action_cursor: 0,
                                card_cursor: None,
                            };
                        }
                        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.modal = ModalState::QuickActions {
                                action_cursor: 0,
                                card_cursor: None,
                            };
                        }
                        KeyCode::Tab => {
                            app.focus = match app.focus {
                                FocusPane::Sidebar => FocusPane::Main,
                                FocusPane::Main => FocusPane::Sidebar,
                            };
                        }
                        KeyCode::Char('d') => {
                            if !app.staged_ids.is_empty() {
                                app.modal = ModalState::ConfirmClean { permanent: false };
                            }
                        }
                        KeyCode::Char('D') => {
                            if !app.staged_ids.is_empty() {
                                app.modal = ModalState::ConfirmClean { permanent: true };
                            }
                        }
                        _ => handle_pane_keys(app, key.code),
                    },
                }
            }
        }
    }
}

fn handle_pane_keys(app: &mut TuiApp, code: KeyCode) {
    match app.focus {
        FocusPane::Sidebar => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                if app.sidebar_cursor + 1 < SidebarItem::ALL.len() {
                    app.sidebar_cursor += 1;
                    app.apply_sidebar_item(SidebarItem::ALL[app.sidebar_cursor]);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.sidebar_cursor > 0 {
                    app.sidebar_cursor -= 1;
                    app.apply_sidebar_item(SidebarItem::ALL[app.sidebar_cursor]);
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                app.apply_sidebar_item(SidebarItem::ALL[app.sidebar_cursor]);
                app.focus = FocusPane::Main;
            }
            _ => {}
        },

        FocusPane::Main => {
            if app.active_sidebar == SidebarItem::VisualMap {
                match code {
                    KeyCode::Left | KeyCode::Char('h') if app.treemap_cursor == 0 => {
                        app.focus = FocusPane::Sidebar;
                    }
                    KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => {
                        if app.treemap_cursor + 1 < app.last_treemap_rects.len() {
                            app.treemap_cursor += 1;
                        }
                    }
                    KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => {
                        app.treemap_cursor = app.treemap_cursor.saturating_sub(1);
                    }
                    KeyCode::Char('g') => {
                        app.drilldown_category = None;
                        app.drilldown_folder = None;
                        app.treemap_mode = app.treemap_mode.next();
                        app.treemap_cursor = 0;
                    }
                    KeyCode::Enter => {
                        if let Some(rect) = app.last_treemap_rects.get(app.treemap_cursor).cloned()
                        {
                            if let Some(entry_id) = rect.entry_id {
                                app.toggle_stage_entry_id(entry_id);
                            } else if let Some(cat) = rect.category {
                                app.drilldown_category = Some(cat);
                                app.treemap_cursor = 0;
                            } else if let Some(folder) = rect.folder {
                                app.drilldown_folder = Some(folder);
                                app.treemap_cursor = 0;
                            }
                        }
                    }
                    KeyCode::Backspace | KeyCode::Esc => {
                        if app.drilldown_category.is_some() || app.drilldown_folder.is_some() {
                            app.drilldown_category = None;
                            app.drilldown_folder = None;
                            app.treemap_cursor = 0;
                        } else {
                            app.focus = FocusPane::Sidebar;
                        }
                    }
                    KeyCode::Char(' ') => {
                        if let Some(rect) = app.last_treemap_rects.get(app.treemap_cursor).cloned()
                        {
                            if let Some(entry_id) = rect.entry_id {
                                app.toggle_stage_entry_id(entry_id);
                            }
                        }
                    }
                    _ => {}
                }
            } else {
                match code {
                    KeyCode::Left | KeyCode::Char('h') => {
                        app.focus = FocusPane::Sidebar;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if app.list_cursor + 1 < app.matched_indices.len() {
                            app.list_cursor += 1;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.list_cursor = app.list_cursor.saturating_sub(1);
                    }
                    KeyCode::PageDown => {
                        app.list_cursor = (app.list_cursor + 8)
                            .min(app.matched_indices.len().saturating_sub(1));
                    }
                    KeyCode::PageUp => {
                        app.list_cursor = app.list_cursor.saturating_sub(8);
                    }
                    KeyCode::Char(' ') => {
                        if let Some(&idx) = app.matched_indices.get(app.list_cursor) {
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
                        let all_in = !selectable.is_empty()
                            && selectable.iter().all(|id| app.staged_ids.contains(id));
                        if all_in {
                            for id in selectable {
                                app.staged_ids.remove(&id);
                            }
                        } else {
                            for id in selectable {
                                app.staged_ids.insert(id);
                            }
                        }
                    }
                    KeyCode::Char('w') => {
                        if let Some(&idx) = app.matched_indices.get(app.list_cursor) {
                            let entry = &mut app.entries[idx];
                            entry.whitelisted = !entry.whitelisted;
                            if entry.whitelisted {
                                app.staged_ids.remove(&entry.id);
                            }
                        }
                    }
                    KeyCode::Char('s') => {
                        app.rule.sort_by = app.rule.sort_by.next();
                        app.recompute_all();
                    }
                    KeyCode::Enter => {
                        if !app.staged_ids.is_empty() {
                            app.modal = ModalState::ConfirmClean { permanent: false };
                        } else if let Some(&idx) = app.matched_indices.get(app.list_cursor) {
                            let id = app.entries[idx].id;
                            app.toggle_stage_entry_id(id);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
