use crate::engine::nlp::parse_natural_language_query;
use crate::model::{
    AutoTag, FileCategory, ParsedNlpResult, SavedFilterPreset, SmartFilterRule, SortBy,
    StorageEntry,
};

pub const BUILTIN_PRESETS: [SavedFilterPreset; 7] = [
    SavedFilterPreset {
        name: "Old Unused Videos",
        description: "Videos >= 100 MB untouched for 120+ days",
        nl_query: "find old unused videos over 100mb",
    },
    SavedFilterPreset {
        name: "Leftover Installers",
        description: "APKs, MSIs, EXEs & setup archives in Downloads",
        nl_query: "installers and apks in downloads",
    },
    SavedFilterPreset {
        name: "Messenger Media Bloat",
        description: "WhatsApp, Telegram & chat videos/images >= 10 MB",
        nl_query: "whatsapp and telegram files over 10mb",
    },
    SavedFilterPreset {
        name: "Stale Screen Captures",
        description: "Screenshots & screen recordings older than 30 days",
        nl_query: "screenshots and screen recordings older than 30 days",
    },
    SavedFilterPreset {
        name: "Dev & Build Cache",
        description: "node_modules, target, .gradle & build caches",
        nl_query: "dev artifacts and build cache",
    },
    SavedFilterPreset {
        name: "Redundant Duplicates",
        description: "Exact hash matches and renamed (1) copies",
        nl_query: "duplicate files",
    },
    SavedFilterPreset {
        name: "Storage Giants",
        description: "Massive files over 250 MB across all folders",
        nl_query: "huge files over 250mb",
    },
];

pub fn evaluate_smart_filter(
    entries: &[StorageEntry],
    rule: &SmartFilterRule,
    now_unix: i64,
) -> (Vec<usize>, u64, ParsedNlpResult) {
    let nlp = parse_natural_language_query(&rule.search_query);
    let d = &nlp.derived_rule;

    let mut active_categories = rule.categories.clone();
    for &c in &d.categories {
        if !active_categories.contains(&c) {
            active_categories.push(c);
        }
    }

    let mut active_tags = rule.auto_tags.clone();
    for &t in &d.auto_tags {
        if !active_tags.contains(&t) {
            active_tags.push(t);
        }
    }

    let mut active_folders = rule.folders.clone();
    for f in &d.folders {
        if !active_folders.contains(f) {
            active_folders.push(f.clone());
        }
    }

    let mut active_exts = rule.extensions.clone();
    for e in &d.extensions {
        if !active_exts.contains(e) {
            active_exts.push(e.clone());
        }
    }

    let min_size = match (rule.min_size_bytes, d.min_size_bytes) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let max_size = match (rule.max_size_bytes, d.max_size_bytes) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let min_age = match (rule.min_age_days, d.min_age_days) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let max_age = match (rule.max_age_days, d.max_age_days) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let only_duplicates = rule.only_duplicates || d.only_duplicates;
    let only_junk = rule.only_junk || d.only_junk;

    let mut matched_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            if rule.exclude_whitelisted && item.whitelisted {
                return false;
            }

            if !active_categories.is_empty() && !active_categories.contains(&item.category) {
                return false;
            }

            if let Some(min_s) = min_size {
                if item.size_bytes < min_s {
                    return false;
                }
            }

            if let Some(max_s) = max_size {
                if item.size_bytes > max_s {
                    return false;
                }
            }

            let age_days = item.days_old(now_unix);
            if let Some(min_a) = min_age {
                if age_days < min_a {
                    return false;
                }
            }
            if let Some(max_a) = max_age {
                if age_days > max_a {
                    return false;
                }
            }

            if !active_folders.is_empty() {
                let lower_path = item.path.to_ascii_lowercase();
                let lower_folder = item.parent_folder.to_ascii_lowercase();
                let folder_ok = active_folders.iter().any(|f| {
                    let fl = f.to_ascii_lowercase();
                    lower_folder.contains(&fl) || lower_path.contains(&fl)
                });
                if !folder_ok {
                    return false;
                }
            }

            if !active_exts.is_empty() {
                let item_ext = item.extension.trim_start_matches('.').to_ascii_lowercase();
                let ext_ok = active_exts
                    .iter()
                    .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&item_ext));
                if !ext_ok {
                    return false;
                }
            }

            if !active_tags.is_empty() {
                let tag_ok = active_tags.iter().any(|t| item.auto_tags.contains(t));
                if !tag_ok {
                    return false;
                }
            }

            if !rule.custom_tags.is_empty() {
                let custom_ok = rule
                    .custom_tags
                    .iter()
                    .any(|ct| item.custom_tags.iter().any(|it| it.eq_ignore_ascii_case(ct)));
                if !custom_ok {
                    return false;
                }
            }

            if only_duplicates && !item.auto_tags.contains(&AutoTag::Duplicate) {
                return false;
            }

            if only_junk {
                let is_junk = item.category == FileCategory::CacheTemp
                    || item.category == FileCategory::Installer
                    || item.auto_tags.contains(&AutoTag::CacheTemp)
                    || item.auto_tags.contains(&AutoTag::Installer)
                    || item.auto_tags.contains(&AutoTag::EmptyFile)
                    || item.auto_tags.contains(&AutoTag::DevArtifact);
                if !is_junk {
                    return false;
                }
            }

            if !nlp.remaining_keywords.is_empty() {
                let searchable = format!(
                    "{} {} {}",
                    item.name.to_ascii_lowercase(),
                    item.path.to_ascii_lowercase(),
                    item.custom_tags.join(" ").to_ascii_lowercase()
                );
                let all_kw = nlp
                    .remaining_keywords
                    .iter()
                    .all(|kw| searchable.contains(&kw.to_ascii_lowercase()));
                if !all_kw {
                    return false;
                }
            }

            true
        })
        .map(|(idx, _)| idx)
        .collect();

    matched_indices.sort_by(|&a, &b| {
        let ea = &entries[a];
        let eb = &entries[b];
        match rule.sort_by {
            SortBy::SizeDesc => eb.size_bytes.cmp(&ea.size_bytes),
            SortBy::SizeAsc => ea.size_bytes.cmp(&eb.size_bytes),
            SortBy::AgeDesc => ea.modified_unix.cmp(&eb.modified_unix),
            SortBy::AgeAsc => eb.modified_unix.cmp(&ea.modified_unix),
            SortBy::NameAsc => ea.name.to_ascii_lowercase().cmp(&eb.name.to_ascii_lowercase()),
        }
    });

    let total_bytes: u64 = matched_indices.iter().map(|&i| entries[i].size_bytes).sum();

    (matched_indices, total_bytes, nlp)
}
