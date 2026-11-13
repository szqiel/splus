use std::collections::{HashMap, HashSet};

use regex::Regex;

use crate::model::{AutoTag, FileCategory, StorageEntry};

const MB: u64 = 1024 * 1024;

/// Normalizes a filename to detect renamed copies like "report (1).pdf", "video_copy.mp4", or "photo - Copy (2).jpg".
pub fn normalize_base_filename(name: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(idx) if idx > 0 => (&name[..idx], &name[idx..]),
        _ => (name, ""),
    };

    let lower_stem = stem.to_ascii_lowercase();
    let lower_ext = ext.to_ascii_lowercase();

    // Strip common duplicate suffixes: " (1)", " - copy", "_copy", "_1"
    let re = Regex::new(r"(?i)(\s*\(\d+\)|\s*[-_]\s*copy(?:\s*\(\d+\)|\s*\d+)?|[-_]copy\d*)$")
        .expect("valid regex");
    let cleaned = re.replace_all(lower_stem.trim(), "").trim().to_string();

    format!("{}{}", cleaned, lower_ext)
}

/// Computes single-file heuristic auto-tags completely offline using deterministic rules.
pub fn compute_single_file_tags(entry: &StorageEntry, now_unix: i64) -> Vec<AutoTag> {
    let mut tags = Vec::new();
    let lower_name = entry.name.to_ascii_lowercase();
    let lower_path = entry.path.replace('\\', "/").to_ascii_lowercase();
    let lower_folder = entry.parent_folder.replace('\\', "/").to_ascii_lowercase();
    let ext = entry.extension.to_ascii_lowercase();
    let days_old = entry.days_old(now_unix);

    // 1. Empty file
    if entry.size_bytes == 0 {
        tags.push(AutoTag::EmptyFile);
    }

    // 2. Dev / build artifact detection (Windows + cross-platform dev folders)
    if lower_path.contains("/node_modules/")
        || lower_path.contains("/target/debug/")
        || lower_path.contains("/target/release/")
        || lower_path.contains("/.gradle/")
        || lower_path.contains("/__pycache__/")
        || lower_path.contains("/.venv/")
        || lower_path.contains("/.next/")
        || lower_path.contains("/dist/")
        || lower_path.contains("/build/intermediates/")
    {
        tags.push(AutoTag::DevArtifact);
    }

    // 3. Screenshot detection
    if entry.category == FileCategory::Image
        && (lower_name.starts_with("screenshot")
            || lower_name.starts_with("screen shot")
            || lower_name.contains("scrnshot")
            || lower_folder.contains("screenshot"))
    {
        tags.push(AutoTag::Screenshot);
    }

    // 4. Screen recording detection
    if entry.category == FileCategory::Video
        && (lower_name.contains("screen_recording")
            || lower_name.contains("screenrecording")
            || lower_name.starts_with("svid_")
            || lower_name.starts_with("rpreplay_")
            || lower_folder.contains("screenrecord")
            || lower_folder.contains("screenshot")
            || lower_folder.contains("captures"))
    {
        tags.push(AutoTag::ScreenRecording);
    }

    // 5. Messenger / chat media detection
    if lower_name.contains("-wa0")
        || lower_name.starts_with("whatsapp")
        || lower_name.starts_with("telegram_")
        || lower_path.contains("/whatsapp/")
        || lower_path.contains("/telegram")
        || lower_path.contains("/wechat/")
        || lower_path.contains("/discord/")
    {
        tags.push(AutoTag::MessengerMedia);
    }

    // 6. Installer / package detection
    if entry.category == FileCategory::Installer
        || matches!(
            ext.as_str(),
            "apk" | "xapk" | "apkm" | "msi" | "exe" | "dmg" | "deb"
        )
        || (entry.category == FileCategory::Archive
            && (lower_folder.contains("download") || lower_name.contains("setup") || lower_name.contains("installer")))
    {
        tags.push(AutoTag::Installer);
    }

    // 7. Cache & temporary junk detection
    if entry.category == FileCategory::CacheTemp
        || matches!(
            ext.as_str(),
            "tmp" | "temp" | "log" | "bak" | "thumbdata" | "nomedia" | "dmp"
        )
        || lower_path.contains("/cache/")
        || lower_path.contains("/.thumbnails")
        || lower_path.contains("/temp/")
        || lower_name.starts_with(".thumbdata")
        || lower_name == "thumbs.db"
        || lower_name == ".ds_store"
    {
        tags.push(AutoTag::CacheTemp);
    }

    // 8. Bloated size threshold for category
    let bloated_threshold = match entry.category {
        FileCategory::Video => 250 * MB,
        FileCategory::Image => 15 * MB,
        FileCategory::Audio => 40 * MB,
        FileCategory::Document => 45 * MB,
        FileCategory::Archive => 180 * MB,
        FileCategory::Installer => 120 * MB,
        FileCategory::CacheTemp => 25 * MB,
        FileCategory::CodeData => 50 * MB,
        FileCategory::Other => 150 * MB,
    };
    if entry.size_bytes >= bloated_threshold {
        tags.push(AutoTag::Bloated);
    }

    // 9. Forgotten file detection (untouched >= 180 days)
    if days_old >= 180 {
        tags.push(AutoTag::Forgotten);
    }

    tags
}

/// Runs both single-file heuristic tagging and cross-file relational tagging (#duplicate, #burst-photo).
pub fn enrich_entries_with_autotags(entries: &mut [StorageEntry], now_unix: i64) {
    for entry in entries.iter_mut() {
        entry.auto_tags = compute_single_file_tags(entry, now_unix);
    }

    // Group by (content_hash, size_bytes) and (normalized_name, size_bytes)
    let mut hash_groups: HashMap<(String, u64), Vec<usize>> = HashMap::new();
    let mut norm_groups: HashMap<(String, u64), Vec<usize>> = HashMap::new();

    for (idx, entry) in entries.iter().enumerate() {
        if entry.size_bytes == 0 {
            continue;
        }
        if let Some(ref h) = entry.content_hash {
            hash_groups
                .entry((h.clone(), entry.size_bytes))
                .or_default()
                .push(idx);
        }
        let norm_name = normalize_base_filename(&entry.name);
        norm_groups
            .entry((norm_name, entry.size_bytes))
            .or_default()
            .push(idx);
    }

    let mut dup_indices = HashSet::new();
    for group in hash_groups.values() {
        if group.len() > 1 {
            for &idx in group {
                dup_indices.insert(idx);
            }
        }
    }
    for group in norm_groups.values() {
        if group.len() > 1 {
            for &idx in group {
                dup_indices.insert(idx);
            }
        }
    }

    // Burst photo detection: images in same parent_folder within 4 seconds & similar size
    let mut photo_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.category == FileCategory::Image && !e.auto_tags.contains(&AutoTag::Screenshot)
        })
        .map(|(i, _)| i)
        .collect();

    photo_indices.sort_by_key(|&i| entries[i].modified_unix);

    let mut burst_indices = HashSet::new();
    for i in 0..photo_indices.len() {
        let idx_curr = photo_indices[i];
        if entries[idx_curr].name.to_ascii_lowercase().contains("burst") {
            burst_indices.insert(idx_curr);
        }
        if i > 0 {
            let idx_prev = photo_indices[i - 1];
            let curr = &entries[idx_curr];
            let prev = &entries[idx_prev];
            let time_diff = (curr.modified_unix - prev.modified_unix).abs();
            let min_sz = curr.size_bytes.min(prev.size_bytes) as f64;
            let max_sz = curr.size_bytes.max(prev.size_bytes).max(1) as f64;
            if curr.parent_folder == prev.parent_folder
                && time_diff <= 4
                && (min_sz / max_sz) >= 0.72
            {
                burst_indices.insert(idx_curr);
                burst_indices.insert(idx_prev);
            }
        }
    }

    for idx in dup_indices {
        if !entries[idx].auto_tags.contains(&AutoTag::Duplicate) {
            entries[idx].auto_tags.push(AutoTag::Duplicate);
        }
    }
    for idx in burst_indices {
        if !entries[idx].auto_tags.contains(&AutoTag::BurstPhoto) {
            entries[idx].auto_tags.push(AutoTag::BurstPhoto);
        }
    }
}
