use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::model::{FileCategory, StorageEntry, StorageTargetKind};

pub fn scan_windows_directory(root: &Path, max_depth: Option<usize>) -> Result<Vec<StorageEntry>> {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let mut walker = WalkDir::new(&root_canon).follow_links(false);
    if let Some(d) = max_depth {
        walker = walker.max_depth(d);
    }

    let raw_paths: Vec<PathBuf> = walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();

    let now_unix = chrono::Utc::now().timestamp();

    let mut entries: Vec<StorageEntry> = raw_paths
        .into_par_iter()
        .filter_map(|path| {
            let meta = fs::metadata(&path).ok()?;
            let size_bytes = meta.len();

            let modified_unix = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(now_unix);

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
                    p.strip_prefix(&root_canon)
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
        })
        .collect();

    for (idx, e) in entries.iter_mut().enumerate() {
        e.id = idx;
    }

    Ok(entries)
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
