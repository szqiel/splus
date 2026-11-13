pub mod adb;
pub mod mtp;
pub mod windows;

use std::env;
use std::path::Path;

use anyhow::{bail, Result};

use crate::engine::autotag::enrich_entries_with_autotags;
use crate::engine::duplicates::populate_hashes_for_size_collisions;
use crate::model::{AndroidDevice, StorageEntry, StorageTargetKind};

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
    let raw_target = target.unwrap_or("");

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
            let list = mtp::scan_mtp_device(dev_filter, max_depth.unwrap_or(6))?;
            (
                format!("mtp://{}", dev_filter.unwrap_or("Android")),
                StorageTargetKind::AndroidMtp,
                list,
            )
        } else {
            // Auto-detect connected Android device: prefer ADB (faster + on-device hashing), fallback to MTP
            let adb_devs = adb::list_adb_devices();
            let ready_adb = adb_devs.iter().find(|d| d.status == "device");
            if let Some(dev) = ready_adb {
                let remote_path = if raw_target.is_empty() {
                    "/sdcard"
                } else {
                    raw_target
                };
                let serial = device_serial.unwrap_or(&dev.id);
                let list = adb::scan_adb_path(Some(serial), remote_path, max_depth)?;
                (
                    format!("{} ({}) [ADB: {}]", dev.name, serial, remote_path),
                    StorageTargetKind::AndroidAdb,
                    list,
                )
            } else {
                let mtp_devs = mtp::list_mtp_devices();
                if let Some(dev) = mtp_devs.first() {
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
        let list = windows::scan_windows_directory(&local_path, max_depth)?;
        let display = windows::trim_windows_verbatim_prefix(
            &local_path
                .canonicalize()
                .unwrap_or(local_path)
                .to_string_lossy(),
        );
        (display, StorageTargetKind::WindowsLocal, list)
    };

    // Enrich with two-stage duplicate hashes and offline heuristic auto-tags
    let now_unix = chrono::Utc::now().timestamp();
    populate_hashes_for_size_collisions(&mut entries);
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
