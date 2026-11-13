use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::model::{format_age, format_bytes, AutoTag, FileCategory, RecentWindow};
use crate::tui::{FocusPane, ModalState, SidebarItem, TuiApp, QUICK_CARDS};

const BORDER_IDLE: Color = Color::Rgb(52, 56, 64);
const BORDER_FOCUS: Color = Color::Rgb(115, 125, 140);
const TEXT_MAIN: Color = Color::Rgb(236, 239, 244);
const TEXT_MUTED: Color = Color::Rgb(120, 128, 140);
const TEXT_DIM: Color = Color::Rgb(85, 92, 105);
const ACCENT_MINT: Color = Color::Rgb(78, 205, 164);
const ACCENT_CORAL: Color = Color::Rgb(242, 109, 109);
const ACCENT_AMBER: Color = Color::Rgb(240, 185, 75);
const ACCENT_BLUE: Color = Color::Rgb(105, 165, 245);
const ACCENT_PURPLE: Color = Color::Rgb(178, 132, 240);

fn tag_color(tag: AutoTag) -> Color {
    match tag {
        AutoTag::Duplicate | AutoTag::Bloated => ACCENT_CORAL,
        AutoTag::Forgotten | AutoTag::BurstPhoto => ACCENT_AMBER,
        AutoTag::CacheTemp | AutoTag::Installer | AutoTag::DevArtifact | AutoTag::EmptyFile => {
            ACCENT_MINT
        }
        AutoTag::Screenshot | AutoTag::ScreenRecording | AutoTag::MessengerMedia => TEXT_MUTED,
    }
}

fn card_accent(cat: FileCategory) -> Color {
    match cat {
        FileCategory::Video => ACCENT_CORAL,
        FileCategory::Image => ACCENT_AMBER,
        FileCategory::Installer => ACCENT_MINT,
        FileCategory::Document => ACCENT_BLUE,
        FileCategory::CacheTemp => ACCENT_PURPLE,
        FileCategory::Audio => ACCENT_PURPLE,
        FileCategory::Archive => ACCENT_AMBER,
        FileCategory::CodeData => ACCENT_BLUE,
        FileCategory::Other => TEXT_MUTED,
    }
}

pub fn draw_dashboard(f: &mut Frame<'_>, app: &mut TuiApp) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Minimal 1-line top bar + padding
            Constraint::Min(10),   // 2-Pane workspace (Sidebar + Main View)
            Constraint::Length(1), // Minimal 1-line footer
        ])
        .split(f.area());

    draw_top_bar(f, app, rows[0]);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(36)])
        .split(rows[1]);

    draw_sidebar(f, app, panes[0]);

    if app.active_sidebar == SidebarItem::VisualMap {
        draw_visual_map_pane(f, app, panes[1]);
    } else {
        draw_file_list_pane(f, app, panes[1]);
    }

    draw_footer_bar(f, app, rows[2]);

    // Centered Modals (Quick Actions palette or Confirm Clean)
    match app.modal {
        ModalState::QuickActions {
            action_cursor,
            card_cursor,
        } => draw_quick_actions_modal(f, app, action_cursor, card_cursor),
        ModalState::ConfirmClean { permanent } => draw_confirm_clean_modal(f, app, permanent),
        ModalState::None => {}
    }
}

fn draw_top_bar(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(36),
            Constraint::Percentage(28),
            Constraint::Percentage(36),
        ])
        .split(area);

    let short_target = app
        .target_label
        .rsplit(['\\', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(&app.target_label);

    let left = Line::from(vec![
        Span::styled(
            "  splus ",
            Style::default().fg(TEXT_MAIN).add_modifier(Modifier::BOLD),
        ),
        Span::styled("• ", Style::default().fg(TEXT_DIM)),
        Span::styled(short_target, Style::default().fg(TEXT_MUTED)),
    ]);
    f.render_widget(Paragraph::new(left), cols[0]);

    let center = Line::from(vec![Span::styled(
        " ⌕ Quick Actions / ",
        Style::default().fg(TEXT_MAIN).bg(Color::Rgb(36, 39, 46)),
    )]);
    f.render_widget(
        Paragraph::new(center).alignment(Alignment::Center),
        cols[1],
    );

    let right_spans = if app.staged_ids.is_empty() {
        vec![
            Span::styled(
                format!("{} total  ", format_bytes(app.total_bytes())),
                Style::default().fg(TEXT_MUTED),
            ),
        ]
    } else {
        vec![
            Span::styled(
                format!(
                    "{} selected ({}) ",
                    app.staged_ids.len(),
                    format_bytes(app.staged_bytes())
                ),
                Style::default()
                    .fg(ACCENT_CORAL)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("↵ Clean  ", Style::default().fg(TEXT_MAIN)),
        ]
    };
    f.render_widget(
        Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right),
        cols[2],
    );
}

fn draw_sidebar(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let is_focused = app.focus == FocusPane::Sidebar && app.modal == ModalState::None;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if is_focused {
            BORDER_FOCUS
        } else {
            BORDER_IDLE
        }));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Views",
        Style::default().fg(TEXT_DIM),
    )));

    for (idx, &item) in SidebarItem::ALL.iter().enumerate() {
        if idx == 2 {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Quick Filters",
                Style::default().fg(TEXT_DIM),
            )));
        }

        let is_active = app.active_sidebar == item;
        let is_cursor = app.sidebar_cursor == idx && is_focused;
        let dot = if is_active { " ● " } else { "   " };

        let size_str = if item == SidebarItem::VisualMap {
            String::new()
        } else {
            format_bytes(app.sidebar_item_bytes(item))
        };

        let label = if item == SidebarItem::RecentlyAdded {
            format!("Recent ({})", app.recent_window.short_label())
        } else {
            item.label().to_string()
        };
        let pad_len = (inner.width as usize)
            .saturating_sub(dot.chars().count() + label.len() + size_str.len() + 1);
        let spaces = " ".repeat(pad_len);

        let row_style = if is_cursor {
            Style::default()
                .fg(TEXT_MAIN)
                .bg(Color::Rgb(42, 46, 54))
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default().fg(TEXT_MAIN).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT_MUTED)
        };

        lines.push(Line::from(vec![
            Span::styled(
                dot,
                if is_active {
                    Style::default().fg(ACCENT_MINT)
                } else {
                    row_style
                },
            ),
            Span::styled(label, row_style),
            Span::styled(spaces, row_style),
            Span::styled(
                format!("{} ", size_str),
                if is_cursor {
                    row_style
                } else {
                    Style::default().fg(TEXT_DIM)
                },
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Devices",
        Style::default().fg(TEXT_DIM),
    )));
    lines.push(Line::from(vec![
        Span::styled(" ● ", Style::default().fg(ACCENT_MINT)),
        Span::styled(app.target_kind.label(), Style::default().fg(TEXT_MAIN)),
    ]));
    for dev in app.android_devices.iter().take(2) {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(&dev.name, Style::default().fg(TEXT_MUTED)),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_file_list_pane(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let is_focused = app.focus == FocusPane::Main && app.modal == ModalState::None;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if is_focused {
            BORDER_FOCUS
        } else {
            BORDER_IDLE
        }));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Clean prompt header bar
            Constraint::Min(4),    // Spacious 2-line file list
        ])
        .split(inner);

    // 1. Clean Prompt Header Bar
    let right_meta = format!(
        "{} items ({}) • {} ",
        app.matched_indices.len(),
        format_bytes(app.matched_bytes),
        app.rule.sort_by.label()
    );

    let prompt_line = if app.active_sidebar == SidebarItem::RecentlyAdded
        && app.rule.search_query.is_empty()
    {
        let mut left_spans = vec![Span::styled(
            " > Recently Added  ",
            Style::default().fg(TEXT_MAIN).add_modifier(Modifier::BOLD),
        )];
        let windows = [
            (RecentWindow::Day, " 24h "),
            (RecentWindow::Week, " 7d "),
            (RecentWindow::Month, " 30d "),
        ];
        for (w, lbl) in windows {
            if app.recent_window == w {
                left_spans.push(Span::styled(
                    lbl,
                    Style::default()
                        .fg(Color::Black)
                        .bg(ACCENT_MINT)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                left_spans.push(Span::styled(lbl, Style::default().fg(TEXT_MUTED)));
            }
            left_spans.push(Span::raw(" "));
        }
        left_spans.push(Span::styled("(t switch)", Style::default().fg(TEXT_DIM)));

        let left_len: usize = left_spans.iter().map(|s| s.content.chars().count()).sum();
        let pad = (chunks[0].width as usize).saturating_sub(left_len + right_meta.chars().count());
        left_spans.push(Span::raw(" ".repeat(pad)));
        left_spans.push(Span::styled(right_meta, Style::default().fg(TEXT_MUTED)));
        Line::from(left_spans)
    } else {
        let title_text = if !app.rule.search_query.is_empty() {
            format!(" > {}", app.rule.search_query)
        } else {
            format!(" > {}", app.active_sidebar.label())
        };
        let pad = (chunks[0].width as usize)
            .saturating_sub(title_text.chars().count() + right_meta.chars().count());
        Line::from(vec![
            Span::styled(
                title_text,
                Style::default().fg(TEXT_MAIN).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(pad)),
            Span::styled(right_meta, Style::default().fg(TEXT_MUTED)),
        ])
    };
    f.render_widget(
        Paragraph::new(prompt_line).style(Style::default().bg(Color::Rgb(28, 30, 36))),
        chunks[0],
    );

    // 2. Spacious 2-line File List
    if app.matched_indices.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "   No files match this filter.",
                Style::default().fg(TEXT_MUTED),
            )),
            Line::from(Span::styled(
                "   Press '/' to search or choose another filter on the left.",
                Style::default().fg(TEXT_DIM),
            )),
        ]);
        f.render_widget(empty, chunks[1]);
        return;
    }

    let items_per_screen = (chunks[1].height as usize / 2).max(1);
    let start_idx = if app.list_cursor >= items_per_screen {
        app.list_cursor + 1 - items_per_screen
    } else {
        0
    };

    let list_items: Vec<ListItem<'_>> = app
        .matched_indices
        .iter()
        .enumerate()
        .skip(start_idx)
        .take(items_per_screen)
        .map(|(row_i, &entry_idx)| {
            let e = &app.entries[entry_idx];
            let is_cursor = row_i == app.list_cursor && is_focused;
            let is_staged = app.staged_ids.contains(&e.id);

            let icon = if e.whitelisted {
                Span::styled(" ★  ", Style::default().fg(ACCENT_AMBER))
            } else if is_staged {
                Span::styled(" ●  ", Style::default().fg(ACCENT_CORAL))
            } else {
                Span::styled(" ○  ", Style::default().fg(TEXT_DIM))
            };

            let size_str = format!("{} ", format_bytes(e.size_bytes));
            let used_w = 4 + e.name.chars().count() + size_str.len();
            let pad_w = (chunks[1].width as usize).saturating_sub(used_w);

            let name_style = if is_cursor {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if is_staged {
                Style::default().fg(ACCENT_CORAL)
            } else {
                Style::default().fg(TEXT_MAIN)
            };

            let row_bg = if is_cursor {
                Style::default().bg(Color::Rgb(36, 40, 48))
            } else {
                Style::default()
            };

            let line1 = Line::from(vec![
                icon,
                Span::styled(&e.name, name_style),
                Span::raw(" ".repeat(pad_w)),
                Span::styled(
                    size_str,
                    Style::default()
                        .fg(if is_staged { ACCENT_CORAL } else { TEXT_MAIN })
                        .add_modifier(Modifier::BOLD),
                ),
            ]);

            let mut line2_spans = vec![Span::styled(
                format!(
                    "    {} • {}",
                    e.parent_folder,
                    format_age(e.days_old(app.now_unix))
                ),
                Style::default().fg(TEXT_DIM),
            )];

            for (ti, &tag) in e.auto_tags.iter().take(2).enumerate() {
                let sep = if ti == 0 { " • " } else { " · " };
                line2_spans.push(Span::styled(sep, Style::default().fg(TEXT_DIM)));
                line2_spans.push(Span::styled(
                    tag.tag_str(),
                    Style::default().fg(tag_color(tag)),
                ));
            }

            let line2 = Line::from(line2_spans);

            ListItem::new(vec![line1, line2]).style(row_bg)
        })
        .collect();

    f.render_widget(List::new(list_items), chunks[1]);
}

fn draw_visual_map_pane(f: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let is_focused = app.focus == FocusPane::Main && app.modal == ModalState::None;
    let title = if let Some(cat) = app.drilldown_category {
        format!(" Visual Map • {} (Backspace to zoom out) ", cat.label())
    } else if let Some(ref fld) = app.drilldown_folder {
        format!(" Visual Map • {} (Backspace to zoom out) ", fld)
    } else {
        format!(
            " Visual Map • {} (Press 'g' to switch view, Enter to zoom in) ",
            app.treemap_mode.label()
        )
    };

    let block = Block::default()
        .title(Span::styled(title, Style::default().fg(TEXT_MUTED)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if is_focused {
            BORDER_FOCUS
        } else {
            BORDER_IDLE
        }));

    let inner = block.inner(area);
    f.render_widget(block, area);

    app.update_treemap_rects(inner.width, inner.height);

    for (idx, rect) in app.last_treemap_rects.iter().enumerate() {
        if rect.w < 3 || rect.h < 2 {
            continue;
        }
        let rx = inner.x.saturating_add(rect.x);
        let ry = inner.y.saturating_add(rect.y);
        let rw = rect.w.min(inner.width.saturating_sub(rect.x));
        let rh = rect.h.min(inner.height.saturating_sub(rect.y));
        if rw == 0 || rh == 0 {
            continue;
        }

        let r_area = Rect::new(rx, ry, rw, rh);
        let is_cursor = idx == app.treemap_cursor && is_focused;
        let is_staged = rect
            .entry_id
            .map(|id| app.staged_ids.contains(&id))
            .unwrap_or(false);

        let accent = if is_staged {
            ACCENT_CORAL
        } else if let Some(cat) = rect.category {
            card_accent(cat)
        } else {
            ACCENT_BLUE
        };

        let b = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(if is_cursor {
                Style::default()
                    .fg(TEXT_MAIN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(accent)
            });

        let content = Paragraph::new(vec![
            Line::from(Span::styled(
                format!(" {}", rect.label),
                Style::default()
                    .fg(if is_cursor { Color::White } else { TEXT_MAIN })
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!(" {} ({:.0}%)", format_bytes(rect.size_bytes), rect.percentage),
                Style::default().fg(TEXT_MUTED),
            )),
        ])
        .block(b);

        f.render_widget(content, r_area);
    }
}

fn draw_footer_bar(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);

    let left_text = if let Some(ref msg) = app.notice {
        Span::styled(format!("  {}", msg), Style::default().fg(ACCENT_MINT))
    } else if !app.staged_ids.is_empty() {
        Span::styled(
            format!(
                "  {} selected ({}) — Press d or ↵ to clean",
                app.staged_ids.len(),
                format_bytes(app.staged_bytes())
            ),
            Style::default().fg(ACCENT_CORAL),
        )
    } else {
        Span::styled(
            format!("  {} files", app.entries.len()),
            Style::default().fg(TEXT_DIM),
        )
    };
    f.render_widget(Paragraph::new(Line::from(left_text)), cols[0]);

    let right_hints = Line::from(vec![
        Span::styled("Tab ", Style::default().fg(TEXT_MUTED)),
        Span::styled("Sidebar   ", Style::default().fg(TEXT_DIM)),
        Span::styled("/ ", Style::default().fg(TEXT_MUTED)),
        Span::styled("Quick actions   ", Style::default().fg(TEXT_DIM)),
        Span::styled("Space ", Style::default().fg(TEXT_MUTED)),
        Span::styled("Select   ", Style::default().fg(TEXT_DIM)),
        Span::styled("s ", Style::default().fg(TEXT_MUTED)),
        Span::styled("Sort   ", Style::default().fg(TEXT_DIM)),
        Span::styled("q ", Style::default().fg(TEXT_MUTED)),
        Span::styled("Quit  ", Style::default().fg(TEXT_DIM)),
    ]);
    f.render_widget(
        Paragraph::new(right_hints).alignment(Alignment::Right),
        cols[1],
    );
}

/// Centered Quick Actions & Search Modal (modeled directly on Screenshot #1).
fn draw_quick_actions_modal(
    f: &mut Frame<'_>,
    app: &TuiApp,
    action_cursor: usize,
    card_cursor: Option<usize>,
) {
    let area = centered_rect(64, 72, f.area());
    f.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER_FOCUS))
        .style(Style::default().bg(Color::Rgb(16, 17, 20)));

    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Search input bar
            Constraint::Length(1), // "Categories" label
            Constraint::Length(4), // 5 horizontal category cards
            Constraint::Length(1), // "Quick actions" label
            Constraint::Min(8),    // Quick actions list
            Constraint::Length(1), // Bottom hint bar
        ])
        .split(inner);

    // 1. Search Input Box
    let query_str = if app.rule.search_query.is_empty() {
        "Search files or type a filter (e.g. old videos > 50mb)...".to_string()
    } else {
        format!("{}█", app.rule.search_query)
    };
    let search_style = if app.rule.search_query.is_empty() {
        Style::default().fg(TEXT_DIM)
    } else {
        Style::default().fg(TEXT_MAIN)
    };

    let search_box = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER_IDLE));

    let right_pills = " ↵ Run   ESC Close ";
    let pad = (sections[0].width as usize)
        .saturating_sub(query_str.chars().count() + right_pills.chars().count() + 6);
    let search_line = Line::from(vec![
        Span::styled(" ⌕ ", Style::default().fg(TEXT_MUTED)),
        Span::styled(query_str, search_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(right_pills, Style::default().fg(TEXT_DIM)),
    ]);
    f.render_widget(Paragraph::new(search_line).block(search_box), sections[0]);

    // 2. Categories Header
    f.render_widget(
        Paragraph::new(Span::styled(
            " Categories (←/→ to pick)",
            Style::default().fg(TEXT_DIM),
        )),
        sections[1],
    );

    // 3. 5 Horizontal Category Cards (just like "Board 1..5" in Screenshot #1)
    let card_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ])
        .split(sections[2]);

    for (i, &(cat, title)) in QUICK_CARDS.iter().enumerate() {
        let is_selected = card_cursor == Some(i);
        let accent = card_accent(cat);
        let bytes = app.category_bytes(cat);

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if is_selected {
                accent
            } else {
                BORDER_IDLE
            }));

        let card_text = Paragraph::new(vec![
            Line::from(Span::styled(
                title,
                Style::default()
                    .fg(accent)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format_bytes(bytes),
                Style::default().fg(TEXT_MUTED),
            )),
        ])
        .alignment(Alignment::Center)
        .block(card_block);

        f.render_widget(card_text, card_cols[i]);
    }

    // 4. Quick Actions Header
    f.render_widget(
        Paragraph::new(Span::styled(
            " Quick actions",
            Style::default().fg(TEXT_DIM),
        )),
        sections[3],
    );

    // 5. Quick Actions List
    let dup_wasted: u64 = app.duplicate_clusters.iter().map(|c| c.wasted_bytes).sum();
    let actions = [
        (
            "⌕  Apply search filter",
            if app.rule.search_query.is_empty() {
                "Show all files in workspace".to_string()
            } else {
                format!(
                    "Filter by \"{}\" ({} matches)",
                    app.rule.search_query,
                    app.matched_indices.len()
                )
            },
        ),
        (
            "✦  Select extra duplicates",
            format!(
                "Keep original file, mark extra copies ({})",
                format_bytes(dup_wasted)
            ),
        ),
        (
            "✦  Select safe junk",
            "Mark temporary files, cache, and leftover installers".to_string(),
        ),
        (
            "↕  Switch sort order",
            format!("Currently sorted by: {}", app.rule.sort_by.label()),
        ),
        (
            "○  Clear selection",
            format!("Unselect all {} marked files", app.staged_ids.len()),
        ),
    ];

    let action_items: Vec<ListItem<'_>> = actions
        .iter()
        .enumerate()
        .map(|(i, (title, sub))| {
            let active = card_cursor.is_none() && action_cursor == i;
            let bg = if active {
                Style::default().bg(Color::Rgb(32, 36, 44))
            } else {
                Style::default()
            };
            ListItem::new(vec![
                Line::from(Span::styled(
                    format!(" {}", title),
                    Style::default()
                        .fg(if active { Color::White } else { TEXT_MAIN })
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("    {}", sub),
                    Style::default().fg(TEXT_MUTED),
                )),
            ])
            .style(bg)
        })
        .collect();

    f.render_widget(List::new(action_items), sections[4]);

    // 6. Bottom Hint
    let footer = Line::from(vec![
        Span::styled(
            " Type to filter naturally (e.g. \"old videos over 100mb\")",
            Style::default().fg(TEXT_DIM),
        ),
        Span::raw("      "),
        Span::styled("↑↓ Navigate", Style::default().fg(TEXT_MUTED)),
    ]);
    f.render_widget(Paragraph::new(footer), sections[5]);
}

fn draw_confirm_clean_modal(f: &mut Frame<'_>, app: &TuiApp, permanent: bool) {
    let area = centered_rect(46, 24, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT_CORAL))
        .style(Style::default().bg(Color::Rgb(18, 19, 23)));

    let mode_desc = if permanent {
        "Permanently delete selected files?"
    } else {
        "Move selected files to Trash / clean?"
    };

    let p = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", mode_desc),
            Style::default().fg(TEXT_MAIN).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "  {} files • {}",
                app.staged_ids.len(),
                format_bytes(app.staged_bytes())
            ),
            Style::default().fg(ACCENT_CORAL),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [↵ Confirm]", Style::default().fg(TEXT_MAIN)),
            Span::raw("    "),
            Span::styled("[Esc Cancel]", Style::default().fg(TEXT_MUTED)),
        ]),
    ])
    .block(block);

    f.render_widget(p, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
