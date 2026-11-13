use std::collections::HashMap;

use crate::model::{format_bytes, FileCategory, StorageEntry, TreemapRect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreemapMode {
    Category,
    Folder,
    Files,
}

impl TreemapMode {
    pub fn next(&self) -> Self {
        match self {
            Self::Category => Self::Folder,
            Self::Folder => Self::Files,
            Self::Files => Self::Category,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Category => "By Category",
            Self::Folder => "By Folder",
            Self::Files => "Top Files",
        }
    }
}

#[derive(Debug, Clone)]
struct RawNode {
    label: String,
    sublabel: String,
    size_bytes: u64,
    category: Option<FileCategory>,
    folder: Option<String>,
    entry_id: Option<usize>,
    count: usize,
}

fn partition_rects(
    nodes: &[RawNode],
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    total_bytes: u64,
    out: &mut Vec<TreemapRect>,
) {
    if nodes.is_empty() || w == 0 || h == 0 {
        return;
    }

    if nodes.len() == 1 || (w < 6 && h < 3) {
        let n = &nodes[0];
        let sum_bytes: u64 = nodes.iter().map(|i| i.size_bytes).sum();
        let pct = if total_bytes > 0 {
            (sum_bytes as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };
        out.push(TreemapRect {
            label: if nodes.len() == 1 {
                n.label.clone()
            } else {
                format!("{} (+{})", n.label, nodes.len() - 1)
            },
            sublabel: if nodes.len() == 1 {
                n.sublabel.clone()
            } else {
                format_bytes(sum_bytes)
            },
            size_bytes: sum_bytes,
            percentage: pct,
            x,
            y,
            w,
            h,
            category: n.category,
            folder: n.folder.clone(),
            entry_id: n.entry_id,
            count: nodes.iter().map(|i| i.count).sum(),
        });
        return;
    }

    let sum: u64 = nodes.iter().map(|n| n.size_bytes).sum();
    if sum == 0 {
        return;
    }

    let half = sum / 2;
    let mut running = 0u64;
    let mut split_idx = 1usize;
    for (i, n) in nodes.iter().enumerate().take(nodes.len() - 1) {
        running += n.size_bytes;
        split_idx = i + 1;
        if running >= half {
            break;
        }
    }

    let left = &nodes[..split_idx];
    let right = &nodes[split_idx..];
    let left_sum: u64 = left.iter().map(|n| n.size_bytes).sum();
    let ratio = (left_sum as f64 / sum as f64).clamp(0.15, 0.85);

    // Note: terminal characters are roughly twice as tall as they are wide,
    // so compare `w` against `h * 2` for balanced visual aspect ratios!
    if w >= h.saturating_mul(2) {
        let left_w = ((w as f64) * ratio).round() as u16;
        let left_w = left_w.clamp(1, w.saturating_sub(1));
        partition_rects(left, x, y, left_w, h, total_bytes, out);
        partition_rects(right, x + left_w, y, w - left_w, h, total_bytes, out);
    } else {
        let top_h = ((h as f64) * ratio).round() as u16;
        let top_h = top_h.clamp(1, h.saturating_sub(1));
        partition_rects(left, x, y, w, top_h, total_bytes, out);
        partition_rects(right, x, y + top_h, w, h - top_h, total_bytes, out);
    }
}

pub fn build_terminal_treemap(
    entries: &[StorageEntry],
    active_indices: &[usize],
    mode: TreemapMode,
    width: u16,
    height: u16,
    max_nodes: usize,
) -> Vec<TreemapRect> {
    let total_bytes: u64 = active_indices.iter().map(|&i| entries[i].size_bytes).sum();
    if total_bytes == 0 || width == 0 || height == 0 {
        return Vec::new();
    }

    let mut raw: Vec<RawNode> = Vec::new();

    match mode {
        TreemapMode::Category => {
            let mut map: HashMap<FileCategory, (u64, usize)> = HashMap::new();
            for &idx in active_indices {
                let e = &entries[idx];
                let entry = map.entry(e.category).or_insert((0, 0));
                entry.0 += e.size_bytes;
                entry.1 += 1;
            }
            for (cat, (bytes, count)) in map {
                if bytes == 0 {
                    continue;
                }
                raw.push(RawNode {
                    label: cat.label().to_string(),
                    sublabel: format!("{} ({} files)", format_bytes(bytes), count),
                    size_bytes: bytes,
                    category: Some(cat),
                    folder: None,
                    entry_id: None,
                    count,
                });
            }
        }
        TreemapMode::Folder => {
            let mut map: HashMap<String, (u64, usize)> = HashMap::new();
            for &idx in active_indices {
                let e = &entries[idx];
                let entry = map.entry(e.parent_folder.clone()).or_insert((0, 0));
                entry.0 += e.size_bytes;
                entry.1 += 1;
            }
            for (folder, (bytes, count)) in map {
                if bytes == 0 {
                    continue;
                }
                raw.push(RawNode {
                    label: folder.clone(),
                    sublabel: format!("{} ({} files)", format_bytes(bytes), count),
                    size_bytes: bytes,
                    category: None,
                    folder: Some(folder),
                    entry_id: None,
                    count,
                });
            }
        }
        TreemapMode::Files => {
            let mut sorted = active_indices.to_vec();
            sorted.sort_by(|&a, &b| entries[b].size_bytes.cmp(&entries[a].size_bytes));
            for &idx in sorted.iter().take(max_nodes) {
                let e = &entries[idx];
                if e.size_bytes == 0 {
                    continue;
                }
                raw.push(RawNode {
                    label: e.name.clone(),
                    sublabel: format!("{} • {}", format_bytes(e.size_bytes), e.parent_folder),
                    size_bytes: e.size_bytes,
                    category: Some(e.category),
                    folder: Some(e.parent_folder.clone()),
                    entry_id: Some(e.id),
                    count: 1,
                });
            }
        }
    }

    raw.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    if raw.len() > max_nodes {
        raw.truncate(max_nodes);
    }

    let mut out = Vec::new();
    partition_rects(&raw, 0, 0, width, height, total_bytes, &mut out);
    out
}

/// Renders a standalone ASCII/Unicode 2D Treemap box for CLI output (`splus scan --treemap`).
pub fn render_ascii_treemap(rects: &[TreemapRect], width: u16, height: u16) -> String {
    if rects.is_empty() || width == 0 || height == 0 {
        return String::from("(No non-empty files to render in treemap)\n");
    }

    let w = width as usize;
    let h = height as usize;
    let mut grid = vec![vec![' '; w]; h];

    for rect in rects {
        let x0 = (rect.x as usize).min(w.saturating_sub(1));
        let y0 = (rect.y as usize).min(h.saturating_sub(1));
        let x1 = ((rect.x + rect.w).saturating_sub(1) as usize).min(w.saturating_sub(1));
        let y1 = ((rect.y + rect.h).saturating_sub(1) as usize).min(h.saturating_sub(1));

        if x1 <= x0 || y1 <= y0 {
            continue;
        }

        // Draw box borders
        for x in x0..=x1 {
            grid[y0][x] = '─';
            grid[y1][x] = '─';
        }
        for row in grid.iter_mut().take(y1 + 1).skip(y0) {
            row[x0] = '│';
            row[x1] = '│';
        }
        grid[y0][x0] = '┌';
        grid[y0][x1] = '┐';
        grid[y1][x0] = '└';
        grid[y1][x1] = '┘';

        let inner_w = x1.saturating_sub(x0 + 1);
        let inner_h = y1.saturating_sub(y0 + 1);

        if inner_w >= 4 && inner_h >= 1 {
            let line1 = format!("{} ({:.0}%)", rect.label, rect.percentage);
            write_str_to_grid(&mut grid[y0 + 1], x0 + 1, inner_w, &line1);
        }
        if inner_w >= 4 && inner_h >= 2 {
            write_str_to_grid(&mut grid[y0 + 2], x0 + 1, inner_w, &rect.sublabel);
        }
    }

    let mut out = String::new();
    for row in grid {
        let s: String = row.into_iter().collect();
        out.push_str(&s);
        out.push('\n');
    }
    out
}

fn write_str_to_grid(row: &mut [char], start_x: usize, max_len: usize, text: &str) {
    for (i, ch) in text.chars().take(max_len).enumerate() {
        if start_x + i < row.len() {
            row[start_x + i] = ch;
        }
    }
}
