use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Row, Table};
use ratatui::Frame;

use crate::model::{format_age, format_bytes, FileCategory};
use crate::tui::{ActiveTab, CleanupSubView, InputMode, TuiApp};

fn category_color(cat: FileCategory) -> Color {
    match cat {
        FileCategory::Video => Color::LightRed,
        FileCategory::Image => Color::LightMagenta,
        FileCategory::Audio => Color::LightCyan,
        FileCategory::Document => Color::LightBlue,
        FileCategory::Archive => Color::LightYellow,
        FileCategory::Installer => Color::LightGreen,
        FileCategory::CacheTemp => Color::DarkGray,
        FileCategory::CodeData => Color::Cyan,
        FileCategory::Other => Color::Gray,
    }
}

pub fn draw_dashboard(f: &mut Frame<'_>, app: &mut TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Top Header & Metrics
            Constraint::Length(3), // Tab Navigation Bar
            Constraint::Min(10),   // Main Active Tab Body
            Constraint::Length(3), // Bottom Status & Shortcuts
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_tab_bar(f, app, chunks[1]);

    match app.active_tab {
        ActiveTab::SmartFilter => draw_smart_filter_tab(f, app, chunks[2]),
        ActiveTab::Treemap => draw_treemap_tab(f, app, chunks[2]),
        ActiveTab::DuplicatesJunk => draw_duplicates_junk_tab(f, app, chunks[2]),
        ActiveTab::CleanupQueue => draw_cleanup_queue_tab(f, app, chunks[2]),
    }

    draw_footer(f, app, chunks[3]);

    // Modal overlays for tag adding or delete confirmation
    match app.input_mode {
        InputMode::AddingCustomTag => draw_tag_modal(f, app),
        InputMode::ConfirmDelete { permanent } => draw_confirm_delete_modal(f, app, permanent),
        _ => {}
    }
}

fn draw_header(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let total = format_bytes(app.total_bytes());
    let reclaimable = format_bytes(app.safe_reclaimable_bytes());
    let staged = format_bytes(app.staged_bytes());
    let cleaned = format_bytes(app.reclaimed_bytes);

    let line = Line::from(vec![
        Span::styled(
            " STORAGE+ ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("[{}]", app.target_kind.label()),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {} ", app.target_label)),
        Span::styled("| Total: ", Style::default().fg(Color::DarkGray)),
        Span::styled(total, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(" | Safe Junk+Dups: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            reclaimable,
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | Staged: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{} ({})", staged, app.staged_ids.len()),
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | Freed: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            cleaned,
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_tab_bar(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let tabs = [
        (ActiveTab::SmartFilter, " [1] Smart Filter & NLP Search "),
        (ActiveTab::Treemap, " [2] Space Treemap & Chart "),
        (ActiveTab::DuplicatesJunk, " [3] Duplicates & Clutter "),
        (
            ActiveTab::CleanupQueue,
            &format!(" [4] Cleanup Queue ({}) & USB Devices ", app.staged_ids.len()),
        ),
    ];

    let mut spans = Vec::new();
    for (tab, label) in tabs {
        if app.active_tab == tab {
            spans.push(Span::styled(
                label.to_string(),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                label.to_string(),
                Style::default().fg(Color::Gray),
            ));
        }
        spans.push(Span::raw("  "));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn draw_smart_filter_tab(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // NLP Search Input + Live Parsed Rule Tokens
            Constraint::Min(6),    // Matched Files Table
            Constraint::Length(4), // Highlighted File Inspector
        ])
        .split(area);

    // 1. Search Box + Live Parsed Tokens
    let is_editing = app.input_mode == InputMode::EditingQuery;
    let query_display = if app.rule.search_query.is_empty() && !is_editing {
        "Press '/' to search (e.g. \"find old unused videos over 100mb\" or \"whatsapp files > 10mb\")"
            .to_string()
    } else if is_editing {
        format!("{}█", app.rule.search_query)
    } else {
        app.rule.search_query.clone()
    };

    let mut token_spans = vec![Span::styled(
        "Parsed Rules: ",
        Style::default().fg(Color::DarkGray),
    )];
    if app.nlp_result.tokens.is_empty()
        && app.rule.categories.is_empty()
        && app.nlp_result.remaining_keywords.is_empty()
    {
        token_spans.push(Span::styled(
            "[All Files - No active filter constraints]",
            Style::default().fg(Color::Gray),
        ));
    } else {
        for tok in &app.nlp_result.tokens {
            token_spans.push(Span::styled(
                format!(" [{}] ", tok.label),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ));
            token_spans.push(Span::raw(" "));
        }
        for &cat in &app.rule.categories {
            token_spans.push(Span::styled(
                format!(" [Cat: {}] ", cat.short_label()),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD),
            ));
            token_spans.push(Span::raw(" "));
        }
        for kw in &app.nlp_result.remaining_keywords {
            token_spans.push(Span::styled(
                format!(" [Name ~ \"{}\"] ", kw),
                Style::default().fg(Color::LightYellow),
            ));
        }
    }

    let search_block = Block::default()
        .title(if is_editing {
            " Offline Natural Language Filter (TYPING - Enter/Esc to apply, Ctrl+U to clear) "
        } else {
            " Offline Natural Language Filter (100% Local Engine • Zero AI API) "
        })
        .borders(Borders::ALL)
        .border_style(if is_editing {
            Style::default().fg(Color::LightCyan)
        } else {
            Style::default().fg(Color::Blue)
        });

    let search_para = Paragraph::new(vec![
        Line::from(Span::styled(
            format!(" 🔍 {}", query_display),
            Style::default().fg(if is_editing {
                Color::LightCyan
            } else {
                Color::White
            }),
        )),
        Line::from(token_spans),
    ])
    .block(search_block);
    f.render_widget(search_para, layout[0]);

    // 2. Matched Files Table
    let table_title = format!(
        " Matched Files: {} ({}) • Sort: {} [Press 's' Sort | 'p' Preset | 'c' Category | 'r' Reset] ",
        app.matched_indices.len(),
        format_bytes(app.matched_bytes),
        app.rule.sort_by.label()
    );

    let header = Row::new(vec!["Stage", "Size", "Age", "Type", "Folder", "Name & Smart Tags"])
        .style(
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        );

    let visible_rows = layout[1].height.saturating_sub(3) as usize;
    let start_idx = if app.filter_cursor >= visible_rows && visible_rows > 0 {
        app.filter_cursor + 1 - visible_rows
    } else {
        0
    };

    let rows: Vec<Row<'_>> = app
        .matched_indices
        .iter()
        .enumerate()
        .skip(start_idx)
        .take(visible_rows.max(1))
        .map(|(row_i, &entry_idx)| {
            let e = &app.entries[entry_idx];
            let is_staged = app.staged_ids.contains(&e.id);
            let is_cursor = row_i == app.filter_cursor;

            let mark = if e.whitelisted {
                " [SAFE]"
            } else if is_staged {
                " [✓]   "
            } else {
                " [ ]   "
            };

            let tags_str: String = e
                .auto_tags
                .iter()
                .map(|t| t.tag_str())
                .chain(e.custom_tags.iter().map(|ct| ct.as_str()))
                .collect::<Vec<_>>()
                .join(" ");

            let name_col = if tags_str.is_empty() {
                e.name.clone()
            } else {
                format!("{}  {}", e.name, tags_str)
            };

            let style = if is_cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD)
            } else if is_staged {
                Style::default().fg(Color::LightRed)
            } else if e.whitelisted {
                Style::default().fg(Color::LightGreen)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                mark.to_string(),
                format_bytes(e.size_bytes),
                format_age(e.days_old(app.now_unix)),
                e.category.short_label().to_string(),
                e.parent_folder.clone(),
                name_col,
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(22),
        Constraint::Min(25),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .title(table_title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(table, layout[1]);

    // 3. Bottom File Inspector
    let inspector_lines = if let Some(e) = app.highlighted_filter_entry() {
        let auto_desc: Vec<String> = e
            .auto_tags
            .iter()
            .map(|t| format!("{} ({})", t.tag_str(), t.description()))
            .collect();
        let hash_str = e.content_hash.as_deref().unwrap_or("unhashed (unique size)");
        vec![
            Line::from(vec![
                Span::styled(" Path: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&e.path, Style::default().fg(Color::White)),
                Span::styled("  | Hash: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    &hash_str[..hash_str.len().min(16)],
                    Style::default().fg(Color::Yellow),
                ),
            ]),
            Line::from(vec![
                Span::styled(" Auto-Tags: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if auto_desc.is_empty() {
                        "None".to_string()
                    } else {
                        auto_desc.join(" • ")
                    },
                    Style::default().fg(Color::LightMagenta),
                ),
            ]),
        ]
    } else {
        vec![Line::from(" No matching files selected.")]
    };

    let inspector = Paragraph::new(inspector_lines).block(
        Block::default()
            .title(" File Inspector [Press 'w' Protect/Whitelist | 't' Add Custom Tag | 'Space' Stage | 'a' Stage All] ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(inspector, layout[2]);
}

fn draw_treemap_tab(f: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(area);

    let canvas_block = Block::default()
        .title(format!(
            " Interactive 2D Space Treemap ({}) [Press 'g' Group Mode | 'Enter' Drill Down | 'Backspace' Reset Zoom] ",
            if let Some(cat) = app.drilldown_category {
                format!("Zoomed: {}", cat.label())
            } else if let Some(ref fld) = app.drilldown_folder {
                format!("Zoomed: {}", fld)
            } else {
                app.treemap_mode.label().to_string()
            }
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let inner = canvas_block.inner(cols[0]);
    f.render_widget(canvas_block, cols[0]);

    app.update_treemap_rects(inner.width, inner.height);

    for (idx, rect) in app.last_treemap_rects.iter().enumerate() {
        if rect.w < 2 || rect.h < 2 {
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
        let is_cursor = idx == app.treemap_cursor;
        let is_staged = rect
            .entry_id
            .map(|id| app.staged_ids.contains(&id))
            .unwrap_or(false);

        let border_color = if is_cursor {
            Color::White
        } else if is_staged {
            Color::LightRed
        } else if let Some(cat) = rect.category {
            category_color(cat)
        } else {
            Color::Cyan
        };

        let title = format!(" {} ({:.0}%) ", rect.label, rect.percentage);
        let b = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if is_cursor {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(border_color)
            });

        let text = Paragraph::new(vec![Line::from(Span::styled(
            &rect.sublabel,
            Style::default().fg(if is_cursor {
                Color::Yellow
            } else {
                Color::Gray
            }),
        ))])
        .block(b);
        f.render_widget(text, r_area);
    }

    // Right column: Legend & Block List
    let items: Vec<ListItem<'_>> = app
        .last_treemap_rects
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let selected = i == app.treemap_cursor;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(
                format!(
                    "{:>5.1}% | {:>9} | {}",
                    r.percentage,
                    format_bytes(r.size_bytes),
                    r.label
                ),
                style,
            )))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Treemap Blocks (↑/↓ to select) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(list, cols[1]);
}

fn draw_duplicates_junk_tab(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(6)])
        .split(area);

    let sub_header = Line::from(vec![
        Span::styled(
            if app.cleanup_subview == CleanupSubView::JunkBuckets {
                " [Clutter & Junk Buckets] "
            } else {
                "  Clutter & Junk Buckets  "
            },
            if app.cleanup_subview == CleanupSubView::JunkBuckets {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
        Span::raw("   "),
        Span::styled(
            if app.cleanup_subview == CleanupSubView::DuplicateClusters {
                " [Duplicate & Burst Clusters] "
            } else {
                "  Duplicate & Burst Clusters  "
            },
            if app.cleanup_subview == CleanupSubView::DuplicateClusters {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
        Span::raw("    Press 'v' to switch mode | Press 'x' for 1-Tap Smart Auto-Select | 'Space' to toggle group"),
    ]);

    f.render_widget(
        Paragraph::new(sub_header).block(Block::default().borders(Borders::ALL)),
        layout[0],
    );

    match app.cleanup_subview {
        CleanupSubView::JunkBuckets => {
            let items: Vec<ListItem<'_>> = app
                .junk_buckets
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let is_cur = i == app.junk_cursor;
                    let staged_count = b
                        .entry_ids
                        .iter()
                        .filter(|id| app.staged_ids.contains(id))
                        .count();
                    let style = if is_cur {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::LightCyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(vec![
                        Line::from(Span::styled(
                            format!(
                                " [{} / {} staged] {:<34} {:>10}   {}",
                                staged_count,
                                b.entry_ids.len(),
                                b.title,
                                format_bytes(b.total_bytes),
                                b.safety.badge()
                            ),
                            style,
                        )),
                        Line::from(Span::styled(
                            format!("     └─ {}", b.subtitle),
                            Style::default().fg(Color::DarkGray),
                        )),
                    ])
                })
                .collect();

            let list = List::new(items).block(
                Block::default()
                    .title(" Detected Clutter Buckets ")
                    .borders(Borders::ALL),
            );
            f.render_widget(list, layout[1]);
        }
        CleanupSubView::DuplicateClusters => {
            let items: Vec<ListItem<'_>> = app
                .duplicate_clusters
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let is_cur = i == app.dup_cursor;
                    let redundant_staged = c
                        .entry_ids
                        .iter()
                        .filter(|&&id| id != c.keep_id && app.staged_ids.contains(&id))
                        .count();
                    let style = if is_cur {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    let paths_preview: Vec<String> = c
                        .entry_ids
                        .iter()
                        .filter_map(|&id| app.entries.iter().find(|e| e.id == id))
                        .map(|e| {
                            if e.id == c.keep_id {
                                format!("KEEP: {}", e.path)
                            } else if app.staged_ids.contains(&e.id) {
                                format!("STAGE[✓]: {}", e.path)
                            } else {
                                format!("COPY[ ]: {}", e.path)
                            }
                        })
                        .collect();

                    ListItem::new(vec![
                        Line::from(Span::styled(
                            format!(
                                " [{} copies staged] {:<32} Wasted: {:>10}  [{}]",
                                redundant_staged,
                                c.title,
                                format_bytes(c.wasted_bytes),
                                c.reason.label()
                            ),
                            style,
                        )),
                        Line::from(Span::styled(
                            format!("     └─ {}", paths_preview.join("  |  ")),
                            Style::default().fg(Color::DarkGray),
                        )),
                    ])
                })
                .collect();

            let list = List::new(items).block(
                Block::default()
                    .title(format!(
                        " Duplicate & Burst Groups ({}) — Press 'x' to Auto-Stage All Redundant Copies ",
                        app.duplicate_clusters.len()
                    ))
                    .borders(Borders::ALL),
            );
            f.render_widget(list, layout[1]);
        }
    }
}

fn draw_cleanup_queue_tab(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let staged_entries: Vec<_> = app
        .entries
        .iter()
        .filter(|e| app.staged_ids.contains(&e.id))
        .collect();

    let items: Vec<ListItem<'_>> = staged_entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let style = if i == app.queue_cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightRed)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(
                format!("{:>10}  |  {}", format_bytes(e.size_bytes), e.path),
                style,
            )))
        })
        .collect();

    let queue_list = List::new(items).block(
        Block::default()
            .title(format!(
                " Staged Cleanup Basket: {} files ({}) [Press 'd' Clean (Recycle/Safe) | 'D' Permanent | 'c' Clear] ",
                staged_entries.len(),
                format_bytes(app.staged_bytes())
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightRed)),
    );
    f.render_widget(queue_list, cols[0]);

    let mut dev_lines = vec![Line::from(Span::styled(
        " Connected Android USB Devices:",
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    ))];

    if app.android_devices.is_empty() {
        dev_lines.push(Line::from("  (No Android USB device detected)"));
        dev_lines.push(Line::from(""));
        dev_lines.push(Line::from("  • For MTP: Plug in USB & choose"));
        dev_lines.push(Line::from("    'File Transfer (MTP)' on phone."));
        dev_lines.push(Line::from("  • For ADB: Enable USB Debugging"));
        dev_lines.push(Line::from("    or run `splus adb-setup`."));
    } else {
        for dev in &app.android_devices {
            dev_lines.push(Line::from(format!(
                "  • {} [{}] ({})",
                dev.name,
                dev.mode.label(),
                dev.status
            )));
        }
    }

    let dev_panel = Paragraph::new(dev_lines).block(
        Block::default()
            .title(" Android USB Targets (ADB + MTP) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(dev_panel, cols[1]);
}

fn draw_footer(f: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            " STATUS ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {} ", app.status_banner)),
        Span::styled(
            " | Keys: [/] NLP Search  [Tab/1-4] Views  [Space] Stage  [p] Presets  [d] Clean  [q] Quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_tag_modal(f: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect(50, 20, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(" Add Custom Tag (Tip: 'keep' also Whitelists file) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan));
    let p = Paragraph::new(vec![
        Line::from(""),
        Line::from(format!("  Tag: #{}█", app.tag_input_buffer)),
        Line::from(""),
        Line::from("  Press Enter to save, Esc to cancel."),
    ])
    .block(block);
    f.render_widget(p, area);
}

fn draw_confirm_delete_modal(f: &mut Frame<'_>, app: &TuiApp, permanent: bool) {
    let area = centered_rect(60, 25, f.area());
    f.render_widget(Clear, area);
    let mode_label = if permanent {
        "PERMANENT DELETE"
    } else {
        "SAFE CLEANUP (Recycle Bin on Windows / Delete on Android)"
    };
    let block = Block::default()
        .title(format!(" Confirm {} ", mode_label))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightRed));
    let p = Paragraph::new(vec![
        Line::from(""),
        Line::from(format!(
            "  Clean {} staged files and reclaim {}?",
            app.staged_ids.len(),
            format_bytes(app.staged_bytes())
        )),
        Line::from(""),
        Line::from("  Press [Y] or [Enter] to confirm, any other key to cancel."),
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
