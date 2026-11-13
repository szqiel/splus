pub mod cli;
pub mod engine;
pub mod model;
pub mod scanner;
pub mod tui;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    cli::execute_cli(args)
}

#[cfg(test)]
mod tests {
    use crate::engine::autotag::{enrich_entries_with_autotags, normalize_base_filename};
    use crate::engine::duplicates::{auto_select_redundant_ids, detect_duplicate_clusters};
    use crate::engine::filter::evaluate_smart_filter;
    use crate::engine::junk::scan_junk_buckets;
    use crate::engine::nlp::parse_natural_language_query;
    use crate::engine::treemap::{build_terminal_treemap, render_ascii_treemap, TreemapMode};
    use crate::model::{
        AutoTag, DuplicateReason, FileCategory, SmartFilterRule, StorageEntry, StorageTargetKind,
    };

    const MB: u64 = 1024 * 1024;
    const DAY: i64 = 86_400;

    fn sample_entries(now: i64) -> Vec<StorageEntry> {
        let mut list = vec![
            StorageEntry {
                id: 0,
                name: "VID_2025_Vacation_4K.mp4".to_string(),
                path: "/sdcard/DCIM/Camera/VID_2025_Vacation_4K.mp4".to_string(),
                parent_folder: "DCIM/Camera".to_string(),
                extension: "mp4".to_string(),
                size_bytes: 420 * MB,
                modified_unix: now - 250 * DAY,
                accessed_unix: now - 240 * DAY,
                category: FileCategory::Video,
                auto_tags: vec![],
                custom_tags: vec![],
                content_hash: Some("hash_vacation_1".to_string()),
                whitelisted: false,
                target_kind: StorageTargetKind::AndroidAdb,
                device_id: Some("serial123".to_string()),
            },
            StorageEntry {
                id: 1,
                name: "Genshin_Installer_v5.apk".to_string(),
                path: "/sdcard/Download/Genshin_Installer_v5.apk".to_string(),
                parent_folder: "Downloads".to_string(),
                extension: "apk".to_string(),
                size_bytes: 310 * MB,
                modified_unix: now - 90 * DAY,
                accessed_unix: now - 90 * DAY,
                category: FileCategory::Installer,
                auto_tags: vec![],
                custom_tags: vec![],
                content_hash: Some("hash_apk_1".to_string()),
                whitelisted: false,
                target_kind: StorageTargetKind::AndroidAdb,
                device_id: Some("serial123".to_string()),
            },
            StorageEntry {
                id: 2,
                name: "Architecture_Spec.pdf".to_string(),
                path: "/sdcard/Download/Architecture_Spec.pdf".to_string(),
                parent_folder: "Downloads".to_string(),
                extension: "pdf".to_string(),
                size_bytes: 25 * MB,
                modified_unix: now - 40 * DAY,
                accessed_unix: now - 40 * DAY,
                category: FileCategory::Document,
                auto_tags: vec![],
                custom_tags: vec![],
                content_hash: Some("hash_pdf_dup".to_string()),
                whitelisted: false,
                target_kind: StorageTargetKind::AndroidAdb,
                device_id: Some("serial123".to_string()),
            },
            StorageEntry {
                id: 3,
                name: "Architecture_Spec (1).pdf".to_string(),
                path: "/sdcard/Download/Architecture_Spec (1).pdf".to_string(),
                parent_folder: "Downloads".to_string(),
                extension: "pdf".to_string(),
                size_bytes: 25 * MB,
                modified_unix: now - 35 * DAY,
                accessed_unix: now - 35 * DAY,
                category: FileCategory::Document,
                auto_tags: vec![],
                custom_tags: vec![],
                content_hash: Some("hash_pdf_dup".to_string()),
                whitelisted: false,
                target_kind: StorageTargetKind::AndroidAdb,
                device_id: Some("serial123".to_string()),
            },
            StorageEntry {
                id: 4,
                name: "VID-20260101-WA0099.mp4".to_string(),
                path: "/sdcard/WhatsApp/Media/VID-20260101-WA0099.mp4".to_string(),
                parent_folder: "WhatsApp/Media".to_string(),
                extension: "mp4".to_string(),
                size_bytes: 64 * MB,
                modified_unix: now - 190 * DAY,
                accessed_unix: now - 190 * DAY,
                category: FileCategory::Video,
                auto_tags: vec![],
                custom_tags: vec![],
                content_hash: Some("hash_wa_99".to_string()),
                whitelisted: false,
                target_kind: StorageTargetKind::AndroidAdb,
                device_id: Some("serial123".to_string()),
            },
        ];
        enrich_entries_with_autotags(&mut list, now);
        list
    }

    #[test]
    fn test_normalize_base_filename() {
        assert_eq!(
            normalize_base_filename("Report_2026 (1).pdf"),
            "report_2026.pdf"
        );
        assert_eq!(
            normalize_base_filename("Keynote_Video - Copy.mp4"),
            "keynote_video.mp4"
        );
        assert_eq!(
            normalize_base_filename("archive_copy2.zip"),
            "archive.zip"
        );
    }

    #[test]
    fn test_offline_nlp_query_parser() {
        let res = parse_natural_language_query("find old unused videos over 100mb in downloads");
        assert!(res.derived_rule.categories.contains(&FileCategory::Video));
        assert_eq!(res.derived_rule.min_size_bytes, Some(100 * MB));
        assert_eq!(res.derived_rule.min_age_days, Some(120));
        assert!(res
            .derived_rule
            .folders
            .iter()
            .any(|f| f.contains("download")));
        assert!(res.tokens.len() >= 4);
    }

    #[test]
    fn test_recent_windows_nlp() {
        let day_res = parse_natural_language_query("added within a day");
        assert_eq!(day_res.derived_rule.max_age_days, Some(1));
        assert!(day_res.remaining_keywords.is_empty());

        let week_res = parse_natural_language_query("files added within a week");
        assert_eq!(week_res.derived_rule.max_age_days, Some(7));
        assert!(week_res.remaining_keywords.is_empty());

        let month_res = parse_natural_language_query("videos added within a month");
        assert_eq!(month_res.derived_rule.max_age_days, Some(30));
        assert!(month_res.derived_rule.categories.contains(&FileCategory::Video));
    }

    #[test]
    fn test_autotagger_and_smart_filter() {
        let now = 1_800_000_000i64;
        let entries = sample_entries(now);

        assert!(entries[0].auto_tags.contains(&AutoTag::Bloated));
        assert!(entries[0].auto_tags.contains(&AutoTag::Forgotten));
        assert!(entries[1].auto_tags.contains(&AutoTag::Installer));
        assert!(entries[2].auto_tags.contains(&AutoTag::Duplicate));
        assert!(entries[3].auto_tags.contains(&AutoTag::Duplicate));
        assert!(entries[4].auto_tags.contains(&AutoTag::MessengerMedia));

        let rule = SmartFilterRule {
            search_query: "old unused videos over 100mb".to_string(),
            ..Default::default()
        };
        let (matched, bytes, _) = evaluate_smart_filter(&entries, &rule, now);
        assert_eq!(matched, vec![0]);
        assert_eq!(bytes, 420 * MB);
    }

    #[test]
    fn test_duplicate_detection_and_auto_select() {
        let now = 1_800_000_000i64;
        let entries = sample_entries(now);
        let clusters = detect_duplicate_clusters(&entries);

        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].reason, DuplicateReason::ExactHash);
        assert_eq!(clusters[0].wasted_bytes, 25 * MB);
        assert_eq!(clusters[0].keep_id, 2);

        let redundant = auto_select_redundant_ids(&clusters, &entries);
        assert_eq!(redundant, vec![3]);
    }

    #[test]
    fn test_junk_buckets_and_treemap() {
        let now = 1_800_000_000i64;
        let entries = sample_entries(now);
        let buckets = scan_junk_buckets(&entries, now);
        assert!(!buckets.is_empty());

        let all_idx: Vec<usize> = (0..entries.len()).collect();
        let rects = build_terminal_treemap(&entries, &all_idx, TreemapMode::Category, 80, 16, 8);
        assert!(!rects.is_empty());
        let ascii = render_ascii_treemap(&rects, 80, 16);
        assert!(ascii.contains('╭'));
    }
}
