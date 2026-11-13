use crate::model::{AutoTag, FileCategory, JunkBucket, SafetyLevel, StorageEntry};

const MB: u64 = 1024 * 1024;

pub fn scan_junk_buckets(entries: &[StorageEntry], now_unix: i64) -> Vec<JunkBucket> {
    let mut cache_ids = Vec::new();
    let mut cache_bytes = 0u64;

    let mut installer_ids = Vec::new();
    let mut installer_bytes = 0u64;

    let mut dev_ids = Vec::new();
    let mut dev_bytes = 0u64;

    let mut capture_ids = Vec::new();
    let mut capture_bytes = 0u64;

    let mut messenger_ids = Vec::new();
    let mut messenger_bytes = 0u64;

    let mut giant_ids = Vec::new();
    let mut giant_bytes = 0u64;

    for e in entries {
        if e.whitelisted {
            continue;
        }
        let age_days = e.days_old(now_unix);

        if e.category == FileCategory::CacheTemp
            || e.auto_tags.contains(&AutoTag::CacheTemp)
            || e.auto_tags.contains(&AutoTag::EmptyFile)
        {
            cache_ids.push(e.id);
            cache_bytes += e.size_bytes;
        }

        if e.category == FileCategory::Installer || e.auto_tags.contains(&AutoTag::Installer) {
            installer_ids.push(e.id);
            installer_bytes += e.size_bytes;
        }

        if e.auto_tags.contains(&AutoTag::DevArtifact) {
            dev_ids.push(e.id);
            dev_bytes += e.size_bytes;
        }

        if (e.auto_tags.contains(&AutoTag::Screenshot)
            || e.auto_tags.contains(&AutoTag::ScreenRecording))
            && age_days >= 30
        {
            capture_ids.push(e.id);
            capture_bytes += e.size_bytes;
        }

        if e.auto_tags.contains(&AutoTag::MessengerMedia) && e.size_bytes >= 5 * MB {
            messenger_ids.push(e.id);
            messenger_bytes += e.size_bytes;
        }

        if e.size_bytes >= 100 * MB && age_days >= 180 {
            giant_ids.push(e.id);
            giant_bytes += e.size_bytes;
        }
    }

    let raw = vec![
        JunkBucket {
            id: "cache",
            title: "Temp, Logs & Thumbnail Cache",
            subtitle: "Safe-to-remove .tmp, .log, .thumbdata & cache files",
            safety: SafetyLevel::Safe,
            entry_ids: cache_ids,
            total_bytes: cache_bytes,
        },
        JunkBucket {
            id: "installers",
            title: "Leftover Installers & APKs",
            subtitle: "Downloaded .apk, .msi, setup .exe & package archives",
            safety: SafetyLevel::Safe,
            entry_ids: installer_ids,
            total_bytes: installer_bytes,
        },
        JunkBucket {
            id: "dev",
            title: "Dev Build & Package Caches",
            subtitle: "node_modules, target/debug, .gradle & __pycache__",
            safety: SafetyLevel::Safe,
            entry_ids: dev_ids,
            total_bytes: dev_bytes,
        },
        JunkBucket {
            id: "captures",
            title: "Stale Screen Captures (>30d)",
            subtitle: "Old screenshots & screen recordings older than 30 days",
            safety: SafetyLevel::Review,
            entry_ids: capture_ids,
            total_bytes: capture_bytes,
        },
        JunkBucket {
            id: "messenger",
            title: "Messenger & Chat Media Bloat",
            subtitle: "Forwarded WhatsApp, Telegram & Discord media >= 5 MB",
            safety: SafetyLevel::Review,
            entry_ids: messenger_ids,
            total_bytes: messenger_bytes,
        },
        JunkBucket {
            id: "giants",
            title: "Forgotten Giants (>100 MB, >6mo)",
            subtitle: "Massive files untouched for over 6 months",
            safety: SafetyLevel::Caution,
            entry_ids: giant_ids,
            total_bytes: giant_bytes,
        },
    ];

    raw.into_iter()
        .filter(|b| !b.entry_ids.is_empty())
        .collect()
}
