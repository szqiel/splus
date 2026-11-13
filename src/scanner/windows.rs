use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};

use crate::model::{FileCategory, StorageEntry, StorageTargetKind};
use crate::scanner::ntfs_usn::try_scan_drive_root_usn_recent;
use crate::scanner::{ScanOptions, ScanProgress};

pub fn scan_windows_directory(root: &Path, max_depth: Option<usize>) -> Result<Vec<StorageEntry>> {
    scan_windows_directory_with_progress(
        root,
        &ScanOptions {
            max_depth,
            ..Default::default()
        },
        None,
    )
}

pub fn scan_windows_directory_with_options(
    root: &Path,
    opts: &ScanOptions,
) -> Result<Vec<StorageEntry>> {
    scan_windows_directory_with_progress(root, opts, None)
}

pub fn scan_windows_directory_with_progress(
    root: &Path,
    opts: &ScanOptions,
    progress: Option<&Arc<ScanProgress>>,
) -> Result<Vec<StorageEntry>> {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root_clean_str = trim_windows_verbatim_prefix(&root_canon.to_string_lossy());
    let root_lower = root_clean_str.to_ascii_lowercase();
    let is_windows_target = root_lower.contains("\\windows");
    let is_root_drive = is_drive_root(&root_clean_str);
    let now_unix = chrono::Utc::now().timestamp();

    // Smart Drive-Root Mode: when scanning a full drive root (like C:\) without --all,
    // default to a 1 MB size floor so we don't waste time on 500,000 tiny 1 KB system/config files.
    let mut effective_opts = opts.clone();
    if is_root_drive && !opts.include_all && effective_opts.min_size_bytes.is_none() {
        effective_opts.min_size_bytes = Some(1024 * 1024);
    }

    // Fast-path #1: If scanning a drive root with a recent filter and running as Administrator,
    // query the NTFS MFT / USN Journal directly (< 0.5s). Falls back seamlessly if not Admin.
    if is_root_drive && effective_opts.max_age_days.is_some() {
        if let Some(usn_entries) =
            try_scan_drive_root_usn_recent(&root_canon, &effective_opts, now_unix, progress)
        {
            return Ok(usn_entries);
        }
    }

    // Fast-path #2: Deep breadth-first work fan-out across heavy folders (Users/<you>/AppData/Local, ProgramData, etc.)
    let work_items = collect_parallel_work_roots(
        &root_canon,
        &effective_opts,
        is_windows_target,
        now_unix,
        progress,
    );

    let mut entries: Vec<StorageEntry> = work_items
        .into_par_iter()
        .flat_map_iter(|item| match item {
            WorkItem::SingleFile(entry) => vec![entry],
            WorkItem::Subtree { dir, depth_used } => walk_subtree(
                &dir,
                &root_canon,
                depth_used,
                &effective_opts,
                is_windows_target,
                now_unix,
                progress,
            ),
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
    progress: Option<&Arc<ScanProgress>>,
) -> Vec<WorkItem> {
    let mut items = Vec::new();
    let max_d = opts.max_depth.unwrap_or(usize::MAX);
    if max_d == 0 {
        return items;
    }

    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root_canon.to_path_buf(), 0));

    while let Some((curr_dir, curr_depth)) = queue.pop_front() {
        let Ok(read_dir) = fs::read_dir(&curr_dir) else {
            continue;
        };

        for entry_res in read_dir {
            let Ok(entry) = entry_res else { continue };
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }

            let next_depth = curr_depth + 1;
            if ft.is_file() {
                if let Some(p) = progress {
                    p.scanned_files.fetch_add(1, Ordering::Relaxed);
                }
                if let Ok(meta) = entry.metadata() {
                    if let Some(built) =
                        build_entry_from_meta(&path, &meta, root_canon, opts, now_unix)
                    {
                        if let Some(p) = progress {
                            p.matched_files.fetch_add(1, Ordering::Relaxed);
                        }
                        items.push(WorkItem::SingleFile(built));
                    }
                }
            } else if ft.is_dir() {
                if !opts.include_system
                    && should_skip_dir_path(&path, is_windows_target, opts.include_all)
                {
                    continue;
                }
                if next_depth >= max_d {
                    continue;
                }

                // Expand up to depth 4 for heavy container directories (Users, <username>, AppData, Local, Roaming, ProgramData)
                // so a single thread never gets stuck walking all of C:\Users\<username> alone.
                if should_expand_further(&path, next_depth) {
                    queue.push_back((path, next_depth));
                } else {
                    items.push(WorkItem::Subtree {
                        dir: path,
                        depth_used: next_depth,
                    });
                }
            }
        }
    }

    items
}

fn should_expand_further(dir: &Path, depth: usize) -> bool {
    if depth <= 1 {
        return true;
    }
    if depth >= 4 {
        return false;
    }
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let parent_name = dir
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    matches!(
        name.as_str(),
        "users"
            | "appdata"
            | "local"
            | "roaming"
            | "locallow"
            | "programdata"
            | "program files"
            | "program files (x86)"
            | "blackmagic design"
    ) || parent_name == "users"
}

fn walk_subtree(
    dir: &Path,
    root_canon: &Path,
    depth_used: usize,
    opts: &ScanOptions,
    is_windows_target: bool,
    now_unix: i64,
    progress: Option<&Arc<ScanProgress>>,
) -> Vec<StorageEntry> {
    if let Some(p) = progress {
        let label = dir
            .strip_prefix(root_canon)
            .unwrap_or(dir)
            .to_string_lossy()
            .replace('\\', "/");
        if !label.is_empty() {
            p.set_folder(&label);
        }
    }

    let mut walker = WalkDir::new(dir).follow_links(false);
    if let Some(max_d) = opts.max_depth {
        walker = walker.max_depth(max_d.saturating_sub(depth_used));
    }

    let include_system = opts.include_system;
    let include_all = opts.include_all;
    let mut local_scanned = 0usize;

    let res: Vec<StorageEntry> = walker
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() && !include_system {
                !should_skip_dir_entry(e, is_windows_target, include_all)
            } else {
                true
            }
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            local_scanned += 1;
            if local_scanned & 0x3FF == 0 {
                if let Some(p) = progress {
                    p.scanned_files.fetch_add(1024, Ordering::Relaxed);
                    if let Some(parent) = e.path().parent() {
                        let rel = parent
                            .strip_prefix(root_canon)
                            .unwrap_or(parent)
                            .to_string_lossy()
                            .replace('\\', "/");
                        p.set_folder(&rel);
                    }
                }
            }

            // WalkDir on Windows caches WIN32_FIND_DATA inside DirEntry::metadata() with zero extra syscalls
            let meta = e.metadata().ok()?;
            let entry = build_entry_from_meta(e.path(), &meta, root_canon, opts, now_unix)?;
            if let Some(p) = progress {
                p.matched_files.fetch_add(1, Ordering::Relaxed);
            }
            Some(entry)
        })
        .collect();

    if let Some(p) = progress {
        let remainder = local_scanned & 0x3FF;
        if remainder > 0 {
            p.scanned_files.fetch_add(remainder, Ordering::Relaxed);
        }
    }

    res
}

#[inline]
pub fn build_entry_from_meta(
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
fn should_skip_dir_entry(entry: &DirEntry, is_windows_target: bool, include_all: bool) -> bool {
    should_skip_dir_path(entry.path(), is_windows_target, include_all)
}

#[inline]
pub fn should_skip_dir_path(path: &Path, is_windows_target: bool, include_all: bool) -> bool {
    let Some(name_os) = path.file_name() else {
        return false;
    };
    let name = name_os.to_string_lossy().to_ascii_lowercase();

    // 1. Always skip protected NTFS volume traps
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

    // 2. Unless --all is passed, skip million-file dev toolchain & browser micro-chunk traps
    if !include_all
        && matches!(
            name.as_str(),
            "node_modules"
                | ".git"
                | ".cargo"
                | ".rustup"
                | ".venv"
                | "venv"
                | "__pycache__"
                | ".gradle"
                | ".nuget"
                | ".next"
                | ".turbo"
                | "code cache"
                | "gpucache"
                | "dawnwebgpucache"
                | "dawngraphitecache"
                | "service worker"
                | "cache_data"
                | "shadercache"
                | "grshadercache"
        )
    {
        return true;
    }

    // 3. Unless the user specifically targeted C:\Windows, skip un-deletable OS system internals
    // (while still allowing C:\Windows\Temp and C:\Windows\Prefetch to be scanned)
    if !is_windows_target {
        let full_lower = path.to_string_lossy().to_ascii_lowercase();
        if full_lower == "c:\\windows"
            || full_lower.ends_with(":\\windows")
            || full_lower.contains("\\windows\\")
        {
            if !full_lower.ends_with(":\\windows")
                && !full_lower.contains("\\windows\\temp")
                && !full_lower.contains("\\windows\\prefetch")
            {
                return true;
            }
        }
    }

    false
}

pub fn is_drive_root(clean_path: &str) -> bool {
    let trimmed = clean_path.trim_end_matches(['\\', '/']);
    trimmed.len() == 2 && trimmed.as_bytes()[1] == b':'
}

pub fn trim_windows_verbatim_prefix(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).to_string()
}

pub fn delete_windows_file(path: &str, permanent: bool) -> Result<()> {
    if permanent {
        fs::remove_file(path).with_context(|| format!("Failed to remove file: {}", path))?;
        return Ok(());
    }

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
        _ => fs::remove_file(path).with_context(|| format!("Failed to delete file: {}", path)),
    }
}
