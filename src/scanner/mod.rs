pub mod adb;
pub mod mtp;
pub mod ntfs_usn;
pub mod windows;

use std::env;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::engine::autotag::enrich_entries_with_autotags;
use crate::engine::duplicates::populate_hashes_for_size_collisions;
use crate::model::{AndroidDevice, StorageEntry, StorageTargetKind};

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    pub max_depth: Option<usize>,
    pub min_size_bytes: Option<u64>,
    pub max_age_days: Option<u64>,
    pub skip_hashes: bool,
    pub include_system: bool,
    pub include_all: bool,
    pub show_progress: bool,
}

pub struct ScanProgress {
    pub scanned_files: AtomicUsize,
    pub matched_files: AtomicUsize,
    pub current_folder: Mutex<String>,
    pub done: AtomicBool,
}

impl ScanProgress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            scanned_files: AtomicUsize::new(0),
            matched_files: AtomicUsize::new(0),
            current_folder: Mutex::new(String::from("starting...")),
            done: AtomicBool::new(false),
        })
    }

    pub fn set_folder(&self, folder: &str) {
        if let Ok(mut guard) = self.current_folder.try_lock() {
            *guard = folder.to_string();
        }
    }
}

struct LiveStatusGuard {
    progress: Arc<ScanProgress>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LiveStatusGuard {
    fn start(enabled: bool) -> (Arc<ScanProgress>, Self) {
        let progress = ScanProgress::new();
        if !enabled {
            return (
                progress.clone(),
                Self {
                    progress,
                    handle: None,
                },
            );
        }

        let p_clone = progress.clone();
        let handle = thread::spawn(move || {
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut tick = 0usize;
            while !p_clone.done.load(Ordering::Relaxed) {
                let scanned = p_clone.scanned_files.load(Ordering::Relaxed);
                let matched = p_clone.matched_files.load(Ordering::Relaxed);
                let folder = p_clone
                    .current_folder
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                let short_folder = if folder.chars().count() > 42 {
                    let tail: String = folder
                        .chars()
                        .rev()
                        .take(39)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    format!("...{}", tail)
                } else {
                    folder
                };

                let spinner = frames[tick % frames.len()];
                let _ = write!(
                    io::stderr(),
                    "\r\x1b[2K  {} Scanning... {:>7} files checked • {} matched ({})",
                    spinner,
                    scanned,
                    matched,
                    short_folder
                );
                let _ = io::stderr().flush();
                tick += 1;
                thread::sleep(Duration::from_millis(80));
            }
            // Erase the temporary status line cleanly when finished
            let _ = write!(io::stderr(), "\r\x1b[2K");
            let _ = io::stderr().flush();
        });

        (
            progress.clone(),
            Self {
                progress,
                handle: Some(handle),
            },
        )
    }
}

impl Drop for LiveStatusGuard {
    fn drop(&mut self) {
        self.progress.done.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Discovers all connected Android USB devices across both ADB and Windows MTP.
pub fn discover_all_android_devices() -> Vec<AndroidDevice> {
    let mut all = adb::list_adb_devices();
    let mtp_list = mtp::list_mtp_devices();
    all.extend(mtp_list);
    all
}

/// Resolves and scans a target (Windows local folder, `adb://...`, `mtp://...`, or `--android` auto-detect),
/// then runs two-stage collision hashing and offline heuristic auto-tagging.
pub fn scan_and_enrich_target(
    target: Option<&str>,
    use_android: bool,
    device_serial: Option<&str>,
    max_depth: Option<usize>,
) -> Result<(String, StorageTargetKind, Vec<StorageEntry>)> {
    scan_and_enrich_target_with_options(
        target,
        use_android,
        device_serial,
        &ScanOptions {
            max_depth,
            show_progress: true,
            ..Default::default()
        },
    )
}

pub fn scan_and_enrich_target_with_options(
    target: Option<&str>,
    use_android: bool,
    device_serial: Option<&str>,
    opts: &ScanOptions,
) -> Result<(String, StorageTargetKind, Vec<StorageEntry>)> {
    let raw_target = target.unwrap_or("");
    let max_depth = opts.max_depth;
    let (progress, _status_guard) = LiveStatusGuard::start(opts.show_progress);

    let (label, kind, mut entries) = if use_android
        || raw_target.starts_with("adb://")
        || raw_target.starts_with("mtp://")
    {
        if let Some(rest) = raw_target.strip_prefix("adb://") {
            let remote_path = if rest.is_empty() {
                "/sdcard"
            } else if rest.starts_with('/') {
                rest
            } else {
                "/sdcard"
            };
            progress.set_folder(remote_path);
            let list = adb::scan_adb_path(device_serial, remote_path, max_depth)?;
            (
                format!("adb://{}", remote_path),
                StorageTargetKind::AndroidAdb,
                list,
            )
        } else if let Some(rest) = raw_target.strip_prefix("mtp://") {
            let dev_filter = if rest.is_empty() {
                device_serial
            } else {
                Some(rest.split('/').next().unwrap_or(rest))
            };
            progress.set_folder("Android USB MTP");
            let list = mtp::scan_mtp_device(dev_filter, max_depth.unwrap_or(6))?;
            (
                format!("mtp://{}", dev_filter.unwrap_or("Android")),
                StorageTargetKind::AndroidMtp,
                list,
            )
        } else {
            let adb_devs = adb::list_adb_devices();
            let ready_adb = adb_devs.iter().find(|d| d.status == "device");
            if let Some(dev) = ready_adb {
                let remote_path = if raw_target.is_empty() {
                    "/sdcard"
                } else {
                    raw_target
                };
                let serial = device_serial.unwrap_or(&dev.id);
                progress.set_folder(remote_path);
                let list = adb::scan_adb_path(Some(serial), remote_path, max_depth)?;
                (
                    format!("{} ({}) [ADB: {}]", dev.name, serial, remote_path),
                    StorageTargetKind::AndroidAdb,
                    list,
                )
            } else {
                let mtp_devs = mtp::list_mtp_devices();
                if let Some(dev) = mtp_devs.first() {
                    progress.set_folder(&dev.name);
                    let list = mtp::scan_mtp_device(Some(&dev.name), max_depth.unwrap_or(6))?;
                    (
                        format!("{} [USB MTP]", dev.name),
                        StorageTargetKind::AndroidMtp,
                        list,
                    )
                } else {
                    bail!(
                        "No Android USB device detected via ADB or Windows MTP.\n\
                         • For MTP (no dev mode): Unlock your phone and choose 'File Transfer (MTP)' in the USB notification.\n\
                         • For ADB: Enable USB Debugging on your phone (run `splus adb-setup` if you need adb.exe installed)."
                    );
                }
            }
        }
    } else {
        let local_path = if raw_target.is_empty() {
            env::current_dir()?
        } else {
            Path::new(raw_target).to_path_buf()
        };
        if !local_path.exists() {
            bail!("Path does not exist: {}", local_path.display());
        }
        let list = windows::scan_windows_directory_with_progress(&local_path, opts, Some(&progress))?;
        let display = windows::trim_windows_verbatim_prefix(
            &local_path
                .canonicalize()
                .unwrap_or(local_path)
                .to_string_lossy(),
        );
        (display, StorageTargetKind::WindowsLocal, list)
    };

    progress.set_folder("analyzing tags...");
    let now_unix = chrono::Utc::now().timestamp();
    if !opts.skip_hashes {
        populate_hashes_for_size_collisions(&mut entries);
    }
    enrich_entries_with_autotags(&mut entries, now_unix);

    Ok((label, kind, entries))
}

/// Deletes a StorageEntry on its underlying backend (Windows Local, Android ADB, or Android MTP).
pub fn delete_storage_entry(entry: &StorageEntry, permanent: bool) -> Result<()> {
    match entry.target_kind {
        StorageTargetKind::WindowsLocal => windows::delete_windows_file(&entry.path, permanent),
        StorageTargetKind::AndroidAdb => {
            adb::delete_adb_file(entry.device_id.as_deref(), &entry.path)
        }
        StorageTargetKind::AndroidMtp => mtp::delete_mtp_file(&entry.path),
    }
}
