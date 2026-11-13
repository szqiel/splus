use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::model::{AndroidDevice, FileCategory, StorageEntry, StorageTargetKind};

/// Finds `adb.exe` across PATH, local `./tools/platform-tools/adb.exe`, Android SDK, Scoop, or Winget.
pub fn find_adb_binary() -> Option<PathBuf> {
    // 1. Check local project tools folder first
    if let Ok(cwd) = env::current_dir() {
        let local_adb = cwd.join("tools").join("platform-tools").join("adb.exe");
        if local_adb.is_file() {
            return Some(local_adb);
        }
    }

    // 2. Check alongside executable
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling_adb = dir.join("tools").join("platform-tools").join("adb.exe");
            if sibling_adb.is_file() {
                return Some(sibling_adb);
            }
        }
    }

    // 3. Check standard PATH via `where.exe adb`
    if let Ok(out) = Command::new("where.exe").arg("adb").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(first_line) = s.lines().next() {
                let p = PathBuf::from(first_line.trim());
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }

    // 4. Check standard Windows SDK / Scoop / Choco locations
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let sdk_adb = Path::new(&local_app_data)
            .join("Android")
            .join("Sdk")
            .join("platform-tools")
            .join("adb.exe");
        if sdk_adb.is_file() {
            return Some(sdk_adb);
        }
    }

    if let Ok(user_profile) = env::var("USERPROFILE") {
        let scoop_adb = Path::new(&user_profile)
            .join("scoop")
            .join("apps")
            .join("adb")
            .join("current")
            .join("adb.exe");
        if scoop_adb.is_file() {
            return Some(scoop_adb);
        }
    }

    None
}

/// Downloads Google's official standalone Windows `platform-tools` (`adb.exe`) into `./tools/platform-tools/`.
pub fn install_standalone_adb() -> Result<PathBuf> {
    if let Some(existing) = find_adb_binary() {
        return Ok(existing);
    }

    let cwd = env::current_dir().context("Failed to read current directory")?;
    let tools_dir = cwd.join("tools");
    std::fs::create_dir_all(&tools_dir)?;
    let zip_path = tools_dir.join("platform-tools-windows.zip");

    println!("Downloading official Android SDK Platform-Tools (adb.exe) from dl.google.com...");
    let dl_script = format!(
        "$ProgressPreference = 'SilentlyContinue'; Invoke-WebRequest -Uri 'https://dl.google.com/android/repository/platform-tools-latest-windows.zip' -OutFile '{}' -UseBasicParsing; Expand-Archive -Path '{}' -DestinationPath '{}' -Force; Remove-Item -Force '{}'",
        zip_path.display(),
        zip_path.display(),
        tools_dir.display(),
        zip_path.display()
    );

    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &dl_script])
        .status()
        .context("Failed to invoke PowerShell to download platform-tools")?;

    if !status.success() {
        bail!("Failed to download or extract Android platform-tools.");
    }

    let adb_exe = tools_dir.join("platform-tools").join("adb.exe");
    if !adb_exe.is_file() {
        bail!(
            "platform-tools archive extracted, but {} was not found.",
            adb_exe.display()
        );
    }

    Ok(adb_exe)
}

/// Lists connected Android devices over ADB (`adb devices -l`).
pub fn list_adb_devices() -> Vec<AndroidDevice> {
    let Some(adb) = find_adb_binary() else {
        return Vec::new();
    };

    let Ok(out) = Command::new(&adb).args(["devices", "-l"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut devices = Vec::new();

    for line in text.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let serial = parts[0].to_string();
        let state = parts[1].to_string();
        let model = parts
            .iter()
            .find_map(|p| p.strip_prefix("model:"))
            .unwrap_or("Android_Device")
            .replace('_', " ");

        devices.push(AndroidDevice {
            id: serial,
            name: model,
            mode: StorageTargetKind::AndroidAdb,
            status: state,
            root_path: "/sdcard".to_string(),
        });
    }

    devices
}

/// Scans an Android path via ADB (`adb shell find ... stat ...`).
pub fn scan_adb_path(
    device_serial: Option<&str>,
    remote_root: &str,
    max_depth: Option<usize>,
) -> Result<Vec<StorageEntry>> {
    let adb = find_adb_binary().context(
        "ADB binary (adb.exe) not found. Run `splus adb-setup` to download standalone ADB automatically, or use MTP mode.",
    )?;

    let depth_arg = max_depth
        .map(|d| format!("-maxdepth {} ", d))
        .unwrap_or_default();

    // Use toybox find + stat to stream size|mtime|atime|path in one fast command
    let shell_cmd = format!(
        "find '{}' {} -type f -exec stat -c '%s|%Y|%X|%n' {{}} + 2>/dev/null",
        remote_root.trim_end_matches('/'),
        depth_arg
    );

    let mut cmd = Command::new(&adb);
    if let Some(s) = device_serial {
        cmd.args(["-s", s]);
    }
    cmd.args(["shell", &shell_cmd]);

    let out = cmd
        .output()
        .context("Failed to execute `adb shell` scan command")?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let now_unix = chrono::Utc::now().timestamp();
    let mut entries = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '|');
        let Some(sz_str) = parts.next() else { continue };
        let Some(mtime_str) = parts.next() else {
            continue;
        };
        let Some(atime_str) = parts.next() else {
            continue;
        };
        let Some(full_path) = parts.next() else {
            continue;
        };

        let size_bytes: u64 = sz_str.parse().unwrap_or(0);
        let modified_unix: i64 = mtime_str.parse().unwrap_or(now_unix);
        let accessed_unix: i64 = atime_str.parse().unwrap_or(modified_unix);

        let name = full_path
            .rsplit('/')
            .next()
            .unwrap_or(full_path)
            .to_string();
        let ext = name
            .rfind('.')
            .map(|i| name[i + 1..].to_ascii_lowercase())
            .unwrap_or_default();

        let parent_folder = full_path
            .rfind('/')
            .map(|i| {
                let p = &full_path[..i];
                p.strip_prefix(remote_root.trim_end_matches('/'))
                    .unwrap_or(p)
                    .trim_start_matches('/')
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "sdcard".to_string());

        let category = FileCategory::from_extension_and_path(&ext, full_path);

        entries.push(StorageEntry {
            id: entries.len(),
            name,
            path: full_path.to_string(),
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
            target_kind: StorageTargetKind::AndroidAdb,
            device_id: device_serial.map(|s| s.to_string()),
        });
    }

    Ok(entries)
}

/// Computes SHA-256 hashes directly on the Android device's CPU for size-colliding duplicate candidates.
pub fn compute_adb_hashes_batch(
    device_serial: Option<&str>,
    remote_paths: &[String],
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(adb) = find_adb_binary() else {
        return map;
    };

    for chunk in remote_paths.chunks(25) {
        let quoted: Vec<String> = chunk
            .iter()
            .map(|p| format!("'{}'", p.replace('\'', "'\\''")))
            .collect();
        let shell_cmd = format!("sha256sum {} 2>/dev/null", quoted.join(" "));

        let mut cmd = Command::new(&adb);
        if let Some(s) = device_serial {
            cmd.args(["-s", s]);
        }
        cmd.args(["shell", &shell_cmd]);

        if let Ok(out) = cmd.output() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if let Some((hash, rest)) = trimmed.split_once(char::is_whitespace) {
                    let path = rest.trim().to_string();
                    if hash.len() >= 32 && !path.is_empty() {
                        map.insert(path, hash.to_string());
                    }
                }
            }
        }
    }

    map
}

pub fn delete_adb_file(device_serial: Option<&str>, remote_path: &str) -> Result<()> {
    let adb = find_adb_binary().context("ADB binary not found")?;
    let escaped = remote_path.replace('\'', "'\\''");
    let shell_cmd = format!("rm -f '{}'", escaped);

    let mut cmd = Command::new(&adb);
    if let Some(s) = device_serial {
        cmd.args(["-s", s]);
    }
    let status = cmd
        .args(["shell", &shell_cmd])
        .status()
        .context("Failed to run `adb shell rm`")?;

    if !status.success() {
        bail!("Failed to remove remote Android file: {}", remote_path);
    }
    Ok(())
}

pub fn pull_adb_file(
    device_serial: Option<&str>,
    remote_path: &str,
    local_dest_dir: &Path,
) -> Result<PathBuf> {
    let adb = find_adb_binary().context("ADB binary not found")?;
    std::fs::create_dir_all(local_dest_dir)?;
    let file_name = remote_path.rsplit('/').next().unwrap_or("pulled_file");
    let dest_file = local_dest_dir.join(file_name);

    let mut cmd = Command::new(&adb);
    if let Some(s) = device_serial {
        cmd.args(["-s", s]);
    }
    let status = cmd
        .args(["pull", remote_path, &dest_file.to_string_lossy()])
        .status()
        .context("Failed to execute `adb pull`")?;

    if !status.success() {
        bail!("`adb pull` failed for {}", remote_path);
    }
    Ok(dest_file)
}
