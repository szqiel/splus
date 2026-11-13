use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};

use crate::model::{FileCategory, StorageEntry, StorageTargetKind};
use crate::scanner::ScanOptions;

pub fn scan_windows_directory(root: &Path, max_depth: Option<usize>) -> Result<Vec<StorageEntry>> {
    scan_windows_directory_with_options(
        root,
        &ScanOptions {
            max_depth,
            ..Default::default()
        },
    )
}

pub fn scan_windows_directory_with_options(
    root: &Path,
    opts: &ScanOptions,
) -> Result<Vec<StorageEntry>> {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root_clean_str = trim_windows_verbatim_prefix(&root_canon.to_string_lossy());
    let is_windows_target = root_clean_str.to_ascii_lowercase().contains("\\windows");
    let now_unix = chrono::Utc::now().timestamp();

    // Collect work roots up to depth 2 so rayon can walk large drives (like C:\) across all CPU cores in parallel
    let work_items = collect_parallel_work_roots(&root_canon, opts, is_windows_target, now_unix);

    let mut entries: Vec<StorageEntry> = work_items
        .into_par_iter()
        .flat_map_iter(|item| match item {
            WorkItem::SingleFile(entry) => vec![entry],
            WorkItem::Subtree { dir, depth_used } => {
                walk_subtree(&dir, &root_canon, depth_used, opts, is_windows_target, now_unix)
            }
        })
        .collect();

    for (idx, e) in entries.iter_mut().enumerate() {
        e.id = idx;
    }

    Ok(entries)
}

enum WorkItem {
    SingleFile(StorageEntry),
    Subtree { dir: PathBuf, depth_used: usize },
}

fn collect_parallel_work_roots(
    root_canon: &Path,
    opts: &ScanOptions,
    is_windows_target: bool,
    now_unix: i64,
) -> Vec<WorkItem> {
    let mut items = Vec::new();
    let max_d = opts.max_depth.unwrap_or(usize::MAX);
    if max_d == 0 {
        return items;
    }

    let Ok(lvl1) = fs::read_dir(root_canon) else {
        return items;
    };

    for e1 in lvl1.flatten() {
        let p1 = e1.path();
        let Ok(ft1) = e1.file_type() else { continue };
        if ft1.is_symlink() {
            continue;
        }
        if ft1.is_file() {
            if let Ok(meta) = e1.metadata() {
                if let Some(entry) =
                    build_entry_from_meta(&p1, &meta, root_canon, opts, now_unix)
                {
                    items.push(WorkItem::SingleFile(entry));
                }
            }
        } else if ft1.is_dir() {
            if !opts.include_system && should_skip_dir_path(&p1, is_windows_target) {
                continue;
            }
            if max_d == 1 {
                continue;
            }
            // Expand one more level (e.g. C:\Users\SYAIR, C:\ProgramData\Blackmagic Design) for balanced parallel threads
            if let Ok(lvl2) = fs::read_dir(&p1) {
                for e2 in lvl2.flatten() {
                    let p2 = e2.path();
                    let Ok(ft2) = e2.file_type() else { continue };
                    if ft2.is_symlink() {
                        continue;
                    }
                    if ft2.is_file() {
                        if let Ok(meta) = e2.metadata() {
                            if let Some(entry) =
                                build_entry_from_meta(&p2, &meta, root_canon, opts, now_unix)
                            {
                                items.push(WorkItem::SingleFile(entry));
                            }
                        }
                    } else if ft2.is_dir() {
                        if !opts.include_system && should_skip_dir_path(&p2, is_windows_target) {
                            continue;
                        }
                        if max_d >= 2 {
                            items.push(WorkItem::Subtree {
                                dir: p2,
                                depth_used: 2,
                            });
                        }
                    }
                }
            }
        }
    }

    items
}

fn walk_subtree(
    dir: &Path,
    root_canon: &Path,
    depth_used: usize,
    opts: &ScanOptions,
    is_windows_target: bool,
    now_unix: i64,
) -> Vec<StorageEntry> {
    let mut walker = WalkDir::new(dir).follow_links(false);
    if let Some(max_d) = opts.max_depth {
        walker = walker.max_depth(max_d.saturating_sub(depth_used));
    }

    let include_system = opts.include_system;
    walker
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() && !include_system {
                !should_skip_dir_entry(e, is_windows_target)
            } else {
                true
            }
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            // WalkDir on Windows caches WIN32_FIND_DATA inside DirEntry::metadata() with zero extra syscalls
            let meta = e.metadata().ok()?;
            build_entry_from_meta(e.path(), &meta, root_canon, opts, now_unix)
        })
        .collect()
}

#[inline]
fn build_entry_from_meta(
    path: &Path,
    meta: &fs::Metadata,
    root_canon: &Path,
    opts: &ScanOptions,
    now_unix: i64,
) -> Option<StorageEntry> {
    let size_bytes = meta.len();

    // Fast early size check before string allocations
    if let Some(min_sz) = opts.min_size_bytes {
        if size_bytes < min_sz {
            return None;
        }
    }

    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(now_unix);

    let ctime = meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(mtime);

    // On Windows NTFS, newly downloaded/extracted files may carry an old archive mtime
    // while their creation time on disk is today. Use the fresher of mtime and ctime.
    let modified_unix = mtime.max(ctime);

    // Fast early age check before string allocations
    if let Some(max_days) = opts.max_age_days {
        let diff_secs = (now_unix - modified_unix).max(0) as u64;
        let days_old = diff_secs / 86_400;
        if days_old > max_days {
            return None;
        }
    }

    let accessed_unix = meta
        .accessed()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(modified_unix);

    let name = path.file_name()?.to_string_lossy().to_string();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let full_path_str = trim_windows_verbatim_prefix(&path.to_string_lossy());
    let parent_folder = path
        .parent()
        .map(|p| {
            p.strip_prefix(root_canon)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ".".to_string());

    let category = FileCategory::from_extension_and_path(&ext, &full_path_str);

    Some(StorageEntry {
        id: 0,
        name,
        path: full_path_str,
        parent_folder,
        extension: ext,
        size_bytes,
        modified_unix,
        accessed_unix,
        category,
        auto_tags: Vec::new(),
        custom_tags: Vec::new(),
        content_hash: None,
        whitelisted: false,
        target_kind: StorageTargetKind::WindowsLocal,
        device_id: None,
    })
}

#[inline]
fn should_skip_dir_entry(entry: &DirEntry, is_windows_target: bool) -> bool {
    should_skip_dir_path(entry.path(), is_windows_target)
}

#[inline]
fn should_skip_dir_path(path: &Path, is_windows_target: bool) -> bool {
    let Some(name_os) = path.file_name() else {
        return false;
    };
    let name = name_os.to_string_lossy().to_ascii_lowercase();

    // Always skip protected NTFS volume traps
    if matches!(
        name.as_str(),
        "$recycle.bin"
            | "system volume information"
            | "$windows.~bt"
            | "$windows.~ws"
            | "recovery"
            | "perflogs"
            | "config.msi"
            | "msocache"
    ) {
        return true;
    }

    // Unless the user specifically targeted C:\Windows, skip un-deletable OS system internals
    // (while still allowing C:\Windows\Temp and C:\Windows\Prefetch to be scanned)
    if !is_windows_target {
        let full_lower = path.to_string_lossy().to_ascii_lowercase();
        if full_lower.contains("\\windows\\") {
            if !full_lower.contains("\\windows\\temp")
                && !full_lower.contains("\\windows\\prefetch")
            {
                return true;
            }
        }
    }

    false
}

pub fn trim_windows_verbatim_prefix(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).to_string()
}

pub fn delete_windows_file(path: &str, permanent: bool) -> Result<()> {
    if permanent {
        fs::remove_file(path).with_context(|| format!("Failed to remove file: {}", path))?;
        return Ok(());
    }

    // Send to Windows Recycle Bin via PowerShell Microsoft.VisualBasic.FileIO
    let escaped = path.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{}', 'OnlyErrorDialogs', 'SendToRecycleBin')",
        escaped
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        _ => {
            // Fallback to standard fs::remove_file if RecycleBin invocation fails
            fs::remove_file(path).with_context(|| format!("Failed to delete file: {}", path))
        }
    }
}
