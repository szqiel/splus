use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

use crate::model::StorageEntry;
use crate::scanner::windows::{build_entry_from_meta, should_skip_dir_path};
use crate::scanner::{ScanOptions, ScanProgress};

const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: isize = -1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00f4;
const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00b3;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct UsnJournalDataV0 {
    usn_journal_id: u64,
    first_usn: i64,
    next_usn: i64,
    lowest_valid_usn: i64,
    max_usn: i64,
    maximum_size: u64,
    allocation_delta: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MftEnumDataV0 {
    start_file_reference_number: u64,
    low_usn: i64,
    high_usn: i64,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *const core::ffi::c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: isize,
    ) -> isize;

    fn DeviceIoControl(
        hDevice: isize,
        dwIoControlCode: u32,
        lpInBuffer: *const core::ffi::c_void,
        nInBufferSize: u32,
        lpOutBuffer: *mut core::ffi::c_void,
        nOutBufferSize: u32,
        lpBytesReturned: *mut u32,
        lpOverlapped: *mut core::ffi::c_void,
    ) -> i32;

    fn CloseHandle(hObject: isize) -> i32;
}

struct HandleGuard(isize);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE && self.0 != 0 {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Attempts an ultra-fast NTFS MFT / USN Journal enumeration when scanning a Windows drive root (e.g. `C:\`)
/// with a recent time filter (`max_age_days`).
/// Requires Administrator privileges to open `\\.\C:`. Returns `None` immediately if not elevated.
pub fn try_scan_drive_root_usn_recent(
    root_canon: &Path,
    opts: &ScanOptions,
    now_unix: i64,
    progress: Option<&Arc<ScanProgress>>,
) -> Option<Vec<StorageEntry>> {
    let max_days = opts.max_age_days?;
    let drive_letter = extract_drive_letter(root_canon)?;
    let volume_device = format!(r"\\.\{}:", drive_letter);
    let wide_vol: Vec<u16> = OsString::from(&volume_device)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let handle = unsafe {
        CreateFileW(
            wide_vol.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };

    if handle == INVALID_HANDLE_VALUE || handle == 0 {
        return None;
    }
    let _guard = HandleGuard(handle);

    let mut journal_data = UsnJournalDataV0::default();
    let mut bytes_returned: u32 = 0;
    let ok_query = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            ptr::null(),
            0,
            &mut journal_data as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<UsnJournalDataV0>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        )
    };
    if ok_query == 0 {
        return None;
    }

    // Windows FILETIME epoch is 1601-01-01 (11644473600 seconds before Unix epoch), in 100-ns ticks
    let cutoff_unix = now_unix.saturating_sub((max_days as i64 + 1) * 86_400);
    let cutoff_filetime: i64 = (cutoff_unix + 11_644_473_600) * 10_000_000;

    let mut enum_data = MftEnumDataV0 {
        start_file_reference_number: 0,
        low_usn: 0,
        high_usn: journal_data.next_usn,
    };

    // Map of directory MFT 48-bit ordinal -> (parent 48-bit ordinal, directory name)
    let mut dirs: HashMap<u64, (u64, String)> = HashMap::with_capacity(65_536);
    // Candidate recent files: (parent 48-bit ordinal, file name)
    let mut recent_candidates: Vec<(u64, String)> = Vec::with_capacity(8_192);

    let mut buffer = vec![0u8; 64 * 1024];
    let mut scanned_count = 0usize;

    loop {
        bytes_returned = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_ENUM_USN_DATA,
                &enum_data as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<MftEnumDataV0>() as u32,
                buffer.as_mut_ptr() as *mut core::ffi::c_void,
                buffer.len() as u32,
                &mut bytes_returned,
                ptr::null_mut(),
            )
        };

        if ok == 0 || bytes_returned <= 8 {
            break;
        }

        // First 8 bytes of output buffer is the next StartFileReferenceNumber
        let next_ref = u64::from_le_bytes(buffer[0..8].try_into().ok()?);
        let mut offset = 8usize;
        let limit = bytes_returned as usize;

        while offset + 60 <= limit {
            let rec = &buffer[offset..limit];
            let record_len = u32::from_le_bytes(rec[0..4].try_into().ok()?) as usize;
            if record_len == 0 || offset + record_len > limit {
                break;
            }

            let major_version = u16::from_le_bytes(rec[4..6].try_into().ok()?);
            if major_version == 2 {
                // USN_RECORD_V2 layout:
                // 8..16: FileReferenceNumber (low 48 bits = MFT entry index)
                // 16..24: ParentFileReferenceNumber
                // 32..40: TimeStamp (LARGE_INTEGER FILETIME)
                // 52..56: FileAttributes
                // 56..58: FileNameLength (bytes)
                // 58..60: FileNameOffset
                let file_ref = u64::from_le_bytes(rec[8..16].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF;
                let parent_ref =
                    u64::from_le_bytes(rec[16..24].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF;
                let timestamp_ft = i64::from_le_bytes(rec[32..40].try_into().ok()?);
                let file_attrs = u32::from_le_bytes(rec[52..56].try_into().ok()?);
                let name_len_bytes = u16::from_le_bytes(rec[56..58].try_into().ok()?) as usize;
                let name_offset = u16::from_le_bytes(rec[58..60].try_into().ok()?) as usize;

                if name_offset + name_len_bytes <= record_len && name_len_bytes >= 2 {
                    let u16_len = name_len_bytes / 2;
                    let name_bytes = &rec[name_offset..name_offset + u16_len * 2];
                    let wide: Vec<u16> = name_bytes
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let name = OsString::from_wide(&wide).to_string_lossy().into_owned();

                    if (file_attrs & FILE_ATTRIBUTE_DIRECTORY) != 0 {
                        dirs.insert(file_ref, (parent_ref, name));
                    } else if timestamp_ft >= cutoff_filetime {
                        recent_candidates.push((parent_ref, name));
                    }
                }
            }

            scanned_count += 1;
            if let Some(p) = progress {
                if scanned_count & 0x1FFF == 0 {
                    p.scanned_files.store(scanned_count, Ordering::Relaxed);
                    p.set_folder("NTFS MFT Journal (fast mode)");
                }
            }

            offset += record_len;
        }

        enum_data.start_file_reference_number = next_ref;
    }

    if let Some(p) = progress {
        p.scanned_files.store(scanned_count, Ordering::Relaxed);
    }

    let drive_root = PathBuf::from(format!("{}:\\", drive_letter));
    let matched_counter = AtomicUsize::new(0);

    // Resolve paths in memory and stat only the recent candidate files in parallel
    let mut entries: Vec<StorageEntry> = recent_candidates
        .into_par_iter()
        .filter_map(|(parent_ref, file_name)| {
            let full_path = resolve_mft_path(parent_ref, &file_name, &dirs, &drive_root)?;
            if !opts.include_system {
                for ancestor in full_path.ancestors().skip(1) {
                    if should_skip_dir_path(ancestor, false, opts.include_all) {
                        return None;
                    }
                }
            }
            let meta = fs::metadata(&full_path).ok()?;
            let entry = build_entry_from_meta(&full_path, &meta, root_canon, opts, now_unix)?;
            let m = matched_counter.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(p) = progress {
                if m & 0x3F == 0 {
                    p.matched_files.store(m, Ordering::Relaxed);
                }
            }
            Some(entry)
        })
        .collect();

    for (idx, e) in entries.iter_mut().enumerate() {
        e.id = idx;
    }

    Some(entries)
}

fn resolve_mft_path(
    mut current_parent: u64,
    file_name: &str,
    dirs: &HashMap<u64, (u64, String)>,
    drive_root: &Path,
) -> Option<PathBuf> {
    // NTFS root directory "." is always MFT reference 5
    const NTFS_ROOT_REF: u64 = 5;
    let mut components = Vec::with_capacity(8);
    let mut depth = 0usize;

    while current_parent != NTFS_ROOT_REF && depth < 64 {
        let (next_parent, dir_name) = dirs.get(&current_parent)?;
        if *next_parent == current_parent {
            break;
        }
        components.push(dir_name.as_str());
        current_parent = *next_parent;
        depth += 1;
    }

    if current_parent != NTFS_ROOT_REF {
        return None;
    }

    let mut full = drive_root.to_path_buf();
    for comp in components.into_iter().rev() {
        full.push(comp);
    }
    full.push(file_name);
    Some(full)
}

fn extract_drive_letter(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let trimmed = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && (bytes[0] as char).is_ascii_alphabetic() {
        let rest = &trimmed[2..];
        if rest.is_empty() || rest == "\\" || rest == "/" {
            return Some((bytes[0] as char).to_ascii_uppercase());
        }
    }
    None
}
