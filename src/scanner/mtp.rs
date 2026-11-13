use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::model::{AndroidDevice, FileCategory, StorageEntry, StorageTargetKind};

/// Lists USB-connected MTP Portable Devices (Android phones/tablets in File Transfer mode)
/// via Windows Shell.Application namespace 0x11 ("This PC").
pub fn list_mtp_devices() -> Vec<AndroidDevice> {
    let ps_script = r#"
$shell = New-Object -ComObject Shell.Application
$pc = $shell.NameSpace(0x11)
if ($null -ne $pc) {
  foreach ($item in $pc.Items()) {
    if (-not $item.IsFileSystem -and $item.IsFolder -and $item.Path -like '::{*') {
      Write-Output "$($item.Name)|$($item.Path)"
    }
  }
}
"#;

    let Ok(out) = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
        .output()
    else {
        return Vec::new();
    };

    if !out.status.success() {
        return Vec::new();
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut devices = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((name, shell_path)) = trimmed.split_once('|') {
            devices.push(AndroidDevice {
                id: name.trim().to_string(),
                name: name.trim().to_string(),
                mode: StorageTargetKind::AndroidMtp,
                status: "MTP Connected".to_string(),
                root_path: shell_path.trim().to_string(),
            });
        }
    }

    devices
}

/// Scans an Android device connected via Windows MTP (Portable Devices) over USB.
pub fn scan_mtp_device(
    device_name_filter: Option<&str>,
    max_depth: usize,
) -> Result<Vec<StorageEntry>> {
    let filter_str = device_name_filter.unwrap_or("").replace('\'', "''");
    let ps_script = format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
$shell = New-Object -ComObject Shell.Application
$pc = $shell.NameSpace(0x11)
$targetDevice = $null
foreach ($item in $pc.Items()) {{
  if (-not $item.IsFileSystem -and $item.IsFolder -and $item.Path -like '::{{*') {{
    if ('{filter}' -eq '' -or $item.Name -like '*{filter}*') {{
      $targetDevice = $item
      break
    }}
  }}
}}
if ($null -eq $targetDevice) {{
  Write-Error 'NO_MTP_DEVICE'
  exit 2
}}
$devName = $targetDevice.Name
function Walk-Mtp($folder, $relPath, $depth) {{
  if ($depth -le 0 -or $null -eq $folder) {{ return }}
  foreach ($it in $folder.Items()) {{
    $name = $it.Name
    $curPath = if ($relPath -eq '') {{ $name }} else {{ "$relPath/$name" }}
    if ($it.IsFolder) {{
      Walk-Mtp $it.GetFolder $curPath ($depth - 1)
    }} else {{
      $sz = $it.Size
      $mod = $it.ModifyDate
      $unix = if ($null -ne $mod) {{ [int64]([datetime]$mod - [datetime]'1970-01-01').TotalSeconds }} else {{ 0 }}
      Write-Output "$sz|$unix|$devName|$curPath"
    }}
  }}
}}
Walk-Mtp $targetDevice.GetFolder '' {depth}
"#,
        filter = filter_str,
        depth = max_depth.max(1)
    );

    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .output()
        .context("Failed to execute Windows MTP Shell scanner")?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("NO_MTP_DEVICE") {
            bail!(
                "No USB MTP Android device detected under 'This PC'. Unlock your phone and select 'File Transfer (MTP)' in the USB notification, or enable USB Debugging for ADB."
            );
        }
        bail!("MTP scan failed: {}", err.trim());
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let now_unix = chrono::Utc::now().timestamp();
    let mut entries = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(4, '|');
        let Some(sz_str) = parts.next() else { continue };
        let Some(mtime_str) = parts.next() else {
            continue;
        };
        let Some(dev_name) = parts.next() else {
            continue;
        };
        let Some(rel_path) = parts.next() else {
            continue;
        };

        let size_bytes: u64 = sz_str.parse().unwrap_or(0);
        let modified_unix: i64 = mtime_str
            .parse()
            .ok()
            .filter(|&t| t > 0)
            .unwrap_or(now_unix);

        let name = rel_path.rsplit('/').next().unwrap_or(rel_path).to_string();
        let ext = name
            .rfind('.')
            .map(|i| name[i + 1..].to_ascii_lowercase())
            .unwrap_or_default();

        let parent_folder = rel_path
            .rfind('/')
            .map(|i| rel_path[..i].to_string())
            .unwrap_or_else(|| dev_name.to_string());

        let full_virtual_path = format!("mtp://{}/{}", dev_name, rel_path);
        let category = FileCategory::from_extension_and_path(&ext, &full_virtual_path);

        entries.push(StorageEntry {
            id: entries.len(),
            name,
            path: full_virtual_path,
            parent_folder,
            extension: ext,
            size_bytes,
            modified_unix,
            accessed_unix: modified_unix,
            category,
            auto_tags: Vec::new(),
            custom_tags: Vec::new(),
            content_hash: None,
            whitelisted: false,
            target_kind: StorageTargetKind::AndroidMtp,
            device_id: Some(dev_name.to_string()),
        });
    }

    Ok(entries)
}

/// Deletes a file on a connected MTP device by navigating its Shell.Application hierarchy and invoking Delete.
pub fn delete_mtp_file(virtual_mtp_path: &str) -> Result<()> {
    let stripped = virtual_mtp_path
        .strip_prefix("mtp://")
        .unwrap_or(virtual_mtp_path);
    let Some((dev_name, rel_path)) = stripped.split_once('/') else {
        bail!("Invalid MTP virtual path: {}", virtual_mtp_path);
    };

    let escaped_dev = dev_name.replace('\'', "''");
    let escaped_rel = rel_path.replace('\'', "''");

    let ps_script = format!(
        r#"
$shell = New-Object -ComObject Shell.Application
$pc = $shell.NameSpace(0x11)
$dev = $null
foreach ($item in $pc.Items()) {{
  if ($item.Name -eq '{dev}') {{ $dev = $item; break }}
}}
if ($null -eq $dev) {{ exit 2 }}
$segments = '{rel}'.Split('/')
$folder = $dev.GetFolder
for ($i = 0; $i -lt $segments.Length - 1; $i++) {{
  $seg = $segments[$i]
  $next = $null
  foreach ($it in $folder.Items()) {{
    if ($it.Name -eq $seg) {{ $next = $it.GetFolder; break }}
  }}
  if ($null -eq $next) {{ exit 3 }}
  $folder = $next
}}
$leafName = $segments[$segments.Length - 1]
foreach ($it in $folder.Items()) {{
  if ($it.Name -eq $leafName) {{
    $it.InvokeVerb('delete')
    exit 0
  }}
}}
exit 4
"#,
        dev = escaped_dev,
        rel = escaped_rel
    );

    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .status()
        .context("Failed to execute MTP delete script")?;

    if !status.success() {
        bail!("Failed to delete MTP item: {}", virtual_mtp_path);
    }
    Ok(())
}
