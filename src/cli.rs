use clap::{Parser, Subcommand, ValueEnum};

use crate::engine::duplicates::{auto_select_redundant_ids, detect_duplicate_clusters};
use crate::engine::filter::evaluate_smart_filter;
use crate::engine::junk::scan_junk_buckets;
use crate::engine::treemap::{build_terminal_treemap, render_ascii_treemap, TreemapMode};
use crate::model::{
    format_age, format_bytes, FileCategory, SmartFilterRule, SortBy, StorageEntry,
};
use crate::scanner::{
    adb::install_standalone_adb, delete_storage_entry, discover_all_android_devices,
    scan_and_enrich_target,
};
use crate::tui::run_interactive_tui;

#[derive(Parser, Debug)]
#[command(
    name = "splus",
    version,
    about = "Storage+ — Smart Storage Analyzer, Offline NLP Filter, Treemap & Cleaner for Windows + Android USB (ADB & MTP)"
)]
pub struct Cli {
    /// Target path (e.g. C:\Users\...\Downloads, adb:///sdcard, or mtp://Phone)
    pub target: Option<String>,

    /// Scan a USB-connected Android device (auto-detects ADB or Windows MTP)
    #[arg(short, long, global = true)]
    pub android: bool,

    /// Specific Android device serial or MTP device name
    #[arg(long, global = true)]
    pub device: Option<String>,

    /// Maximum directory traversal depth
    #[arg(long, global = true)]
    pub max_depth: Option<usize>,

    /// Initial natural language query when launching TUI
    #[arg(short, long)]
    pub query: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Launch the Interactive Terminal UI (TUI) dashboard
    Tui {
        /// Target path on Windows or Android
        path: Option<String>,
        /// Optional initial natural language filter query
        #[arg(short, long)]
        query: Option<String>,
    },

    /// Scan storage and display breakdown + ASCII/Unicode 2D Treemap
    Scan {
        /// Target path on Windows or Android
        path: Option<String>,
        /// Group treemap by: category, folder, or files
        #[arg(short, long, value_enum, default_value_t = GroupModeArg::Category)]
        group_by: GroupModeArg,
        /// Number of top largest files to list
        #[arg(short, long, default_value_t = 15)]
        top: usize,
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Filter files using Offline Natural Language & Rule-Based Smart Filters (No AI API)
    Query {
        /// Natural language query (e.g. "find old unused videos over 100mb in downloads")
        nl_query: String,
        /// Optional target path (defaults to current dir or --android)
        #[arg(short, long)]
        path: Option<String>,
        /// Minimum size in MB
        #[arg(long)]
        min_mb: Option<u64>,
        /// Minimum age in days
        #[arg(long)]
        min_days: Option<u64>,
        /// Maximum number of results to display
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Detect exact hash duplicates, renamed (1) copies, and burst photo series
    Dupes {
        /// Target path on Windows or Android
        path: Option<String>,
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Scan for categorized Junk & Clutter buckets (Cache, Installers, Dev Bloat, Captures, Chat Media)
    Junk {
        /// Target path on Windows or Android
        path: Option<String>,
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Clean files matching a smart query, duplicate copies, or safe junk (Dry-Run by default!)
    Clean {
        /// Target path on Windows or Android
        path: Option<String>,
        /// Natural language smart filter query to select files
        #[arg(short, long)]
        query: Option<String>,
        /// Auto-select all redundant duplicate copies (keeps 1 original per group)
        #[arg(long)]
        duplicates: bool,
        /// Auto-select all Safe Junk buckets (cache, temp, leftover installers, dev build cache)
        #[arg(long)]
        safe_junk: bool,
        /// Actually execute deletion (without --execute, runs in safe Dry-Run preview mode)
        #[arg(long)]
        execute: bool,
        /// Permanently delete instead of sending to Windows Recycle Bin
        #[arg(long)]
        permanent: bool,
    },

    /// List connected Android USB devices across both ADB and Windows MTP
    Devices,

    /// Automatically download and configure standalone Android Platform-Tools (adb.exe)
    AdbSetup,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum GroupModeArg {
    Category,
    Folder,
    Files,
}

impl From<GroupModeArg> for TreemapMode {
    fn from(val: GroupModeArg) -> Self {
        match val {
            GroupModeArg::Category => TreemapMode::Category,
            GroupModeArg::Folder => TreemapMode::Folder,
            GroupModeArg::Files => TreemapMode::Files,
        }
    }
}

pub fn execute_cli(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        None => {
            let (label, kind, entries) = scan_and_enrich_target(
                cli.target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                cli.max_depth,
            )?;
            run_interactive_tui(label, kind, entries, cli.query)
        }

        Some(Commands::Tui { path, query }) => {
            let target = path.or(cli.target);
            let (label, kind, entries) = scan_and_enrich_target(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                cli.max_depth,
            )?;
            run_interactive_tui(label, kind, entries, query.or(cli.query))
        }

        Some(Commands::Devices) => {
            let devs = discover_all_android_devices();
            println!("=== Connected Android USB Devices (ADB + Windows MTP) ===");
            if devs.is_empty() {
                println!("No Android USB devices currently detected.");
                println!("  • For MTP Mode (No USB Debugging needed): Connect phone via USB and select 'File Transfer (MTP)'.");
                println!("  • For ADB Mode: Enable USB Debugging on your phone (run `splus adb-setup` if you need adb.exe).");
            } else {
                for (i, d) in devs.iter().enumerate() {
                    println!(
                        "  [{}] {} | Mode: {} | ID: {} | Status: {}",
                        i + 1,
                        d.name,
                        d.mode.label(),
                        d.id,
                        d.status
                    );
                }
            }
            Ok(())
        }

        Some(Commands::AdbSetup) => {
            let adb_path = install_standalone_adb()?;
            println!("Standalone ADB ready at: {}", adb_path.display());
            Ok(())
        }

        Some(Commands::Scan {
            path,
            group_by,
            top,
            json,
        }) => {
            let target = path.or(cli.target);
            let (label, kind, entries) = scan_and_enrich_target(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                cli.max_depth,
            )?;

            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
                return Ok(());
            }

            let total_bytes: u64 = entries.iter().map(|e| e.size_bytes).sum();
            let now_unix = chrono::Utc::now().timestamp();
            let all_indices: Vec<usize> = (0..entries.len()).collect();

            println!("================================================================================");
            println!(
                " STORAGE+ SCAN REPORT | Target: {} [{}]",
                label,
                kind.label()
            );
            println!(
                " Total Analyzed: {} across {} files",
                format_bytes(total_bytes),
                entries.len()
            );
            println!("================================================================================\n");

            // Category Distribution Breakdown
            println!("--- Storage Breakdown by Category ---");
            for cat in FileCategory::ALL {
                let (cat_bytes, cat_count) = entries
                    .iter()
                    .filter(|e| e.category == cat)
                    .fold((0u64, 0usize), |(b, c), e| (b + e.size_bytes, c + 1));
                if cat_count == 0 {
                    continue;
                }
                let pct = if total_bytes > 0 {
                    (cat_bytes as f64 / total_bytes as f64) * 100.0
                } else {
                    0.0
                };
                let bar_len = ((pct / 100.0) * 28.0).round() as usize;
                let bar = "█".repeat(bar_len) + &"░".repeat(28usize.saturating_sub(bar_len));
                println!(
                    "  {:<18} [{}] {:>5.1}% | {:>10} ({} files)",
                    cat.label(),
                    bar,
                    pct,
                    format_bytes(cat_bytes),
                    cat_count
                );
            }
            println!();

            // 2D Terminal Treemap
            let mode: TreemapMode = group_by.into();
            println!("--- 2D Squarified Space Treemap ({}) ---", mode.label());
            let rects = build_terminal_treemap(&entries, &all_indices, mode, 80, 18, 10);
            print!("{}", render_ascii_treemap(&rects, 80, 18));
            println!();

            // Top N Largest Files
            let mut sorted = entries.clone();
            sorted.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
            println!("--- Top {} Largest Files ---", top.min(sorted.len()));
            print_entries_table(&sorted[..top.min(sorted.len())], now_unix);
            Ok(())
        }

        Some(Commands::Query {
            nl_query,
            path,
            min_mb,
            min_days,
            limit,
            json,
        }) => {
            let target = path.or(cli.target);
            let (label, kind, entries) = scan_and_enrich_target(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                cli.max_depth,
            )?;

            let now_unix = chrono::Utc::now().timestamp();
            let mut rule = SmartFilterRule {
                search_query: nl_query,
                sort_by: SortBy::SizeDesc,
                ..Default::default()
            };
            if let Some(mb) = min_mb {
                rule.min_size_bytes = Some(mb * 1024 * 1024);
            }
            if let Some(d) = min_days {
                rule.min_age_days = Some(d);
            }

            let (matched_indices, matched_bytes, nlp) =
                evaluate_smart_filter(&entries, &rule, now_unix);

            let matched_entries: Vec<StorageEntry> =
                matched_indices.iter().map(|&i| entries[i].clone()).collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&matched_entries)?);
                return Ok(());
            }

            println!("================================================================================");
            println!(
                " SMART FILTER QUERY | Target: {} [{}]",
                label,
                kind.label()
            );
            println!(" Query: \"{}\"", rule.search_query);
            let token_labels: Vec<String> =
                nlp.tokens.iter().map(|t| format!("[{}]", t.label)).collect();
            println!(
                " Offline NLP Parsed Rules: {}",
                if token_labels.is_empty() {
                    "[Keyword Search]".to_string()
                } else {
                    token_labels.join(" ")
                }
            );
            if !nlp.remaining_keywords.is_empty() {
                println!(" Remaining Name Keywords: {:?}", nlp.remaining_keywords);
            }
            println!(
                " Matched: {} files ({})",
                matched_entries.len(),
                format_bytes(matched_bytes)
            );
            println!("================================================================================\n");

            print_entries_table(&matched_entries[..limit.min(matched_entries.len())], now_unix);
            Ok(())
        }

        Some(Commands::Dupes { path, json }) => {
            let target = path.or(cli.target);
            let (label, kind, entries) = scan_and_enrich_target(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                cli.max_depth,
            )?;

            let clusters = detect_duplicate_clusters(&entries);
            if json {
                println!("{}", serde_json::to_string_pretty(&clusters)?);
                return Ok(());
            }

            let total_wasted: u64 = clusters.iter().map(|c| c.wasted_bytes).sum();
            println!("================================================================================");
            println!(
                " DUPLICATE & BURST SCANNER | Target: {} [{}]",
                label,
                kind.label()
            );
            println!(
                " Found {} Duplicate Groups | Reclaimable Wasted Space: {}",
                clusters.len(),
                format_bytes(total_wasted)
            );
            println!("================================================================================\n");

            for (i, c) in clusters.iter().enumerate() {
                println!(
                    " {}. {} [{}] — Wasted: {}",
                    i + 1,
                    c.title,
                    c.reason.label(),
                    format_bytes(c.wasted_bytes)
                );
                for &id in &c.entry_ids {
                    if let Some(e) = entries.iter().find(|x| x.id == id) {
                        let role = if e.id == c.keep_id {
                            "KEEP ORIGINAL"
                        } else {
                            "REDUNDANT COPY"
                        };
                        println!(
                            "     • [{:<14}] {:>10} | {}",
                            role,
                            format_bytes(e.size_bytes),
                            e.path
                        );
                    }
                }
                println!();
            }
            Ok(())
        }

        Some(Commands::Junk { path, json }) => {
            let target = path.or(cli.target);
            let (label, kind, entries) = scan_and_enrich_target(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                cli.max_depth,
            )?;

            let now_unix = chrono::Utc::now().timestamp();
            let buckets = scan_junk_buckets(&entries, now_unix);
            if json {
                println!("{}", serde_json::to_string_pretty(&buckets)?);
                return Ok(());
            }

            let total_junk: u64 = buckets.iter().map(|b| b.total_bytes).sum();
            println!("================================================================================");
            println!(
                " CLUTTER & JUNK SCANNER | Target: {} [{}]",
                label,
                kind.label()
            );
            println!(
                " Total Categorized Clutter: {} across {} buckets",
                format_bytes(total_junk),
                buckets.len()
            );
            println!("================================================================================\n");

            for b in &buckets {
                println!(
                    " {} {} — {} ({} files)",
                    b.safety.badge(),
                    b.title,
                    format_bytes(b.total_bytes),
                    b.entry_ids.len()
                );
                println!("   {}", b.subtitle);
                for &id in b.entry_ids.iter().take(5) {
                    if let Some(e) = entries.iter().find(|x| x.id == id) {
                        println!("     • {:>10} | {}", format_bytes(e.size_bytes), e.path);
                    }
                }
                if b.entry_ids.len() > 5 {
                    println!("     ... and {} more files", b.entry_ids.len() - 5);
                }
                println!();
            }
            Ok(())
        }

        Some(Commands::Clean {
            path,
            query,
            duplicates,
            safe_junk,
            execute,
            permanent,
        }) => {
            let target = path.or(cli.target);
            let (label, kind, entries) = scan_and_enrich_target(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                cli.max_depth,
            )?;
            let now_unix = chrono::Utc::now().timestamp();

            let mut selected_ids = std::collections::HashSet::new();

            if let Some(q) = query {
                let rule = SmartFilterRule {
                    search_query: q,
                    exclude_whitelisted: true,
                    ..Default::default()
                };
                let (matched, _, _) = evaluate_smart_filter(&entries, &rule, now_unix);
                for idx in matched {
                    selected_ids.insert(entries[idx].id);
                }
            }

            if duplicates {
                let clusters = detect_duplicate_clusters(&entries);
                for id in auto_select_redundant_ids(&clusters, &entries) {
                    selected_ids.insert(id);
                }
            }

            if safe_junk {
                let buckets = scan_junk_buckets(&entries, now_unix);
                for b in buckets
                    .into_iter()
                    .filter(|b| b.safety == crate::model::SafetyLevel::Safe)
                {
                    for id in b.entry_ids {
                        selected_ids.insert(id);
                    }
                }
            }

            let to_clean: Vec<&StorageEntry> = entries
                .iter()
                .filter(|e| selected_ids.contains(&e.id) && !e.whitelisted)
                .collect();

            let total_clean_bytes: u64 = to_clean.iter().map(|e| e.size_bytes).sum();

            println!("================================================================================");
            println!(
                " SMART CLEANUP {} | Target: {} [{}]",
                if execute { "[EXECUTING]" } else { "[DRY-RUN PREVIEW]" },
                label,
                kind.label()
            );
            println!(
                " Selected for Cleanup: {} files ({})",
                to_clean.len(),
                format_bytes(total_clean_bytes)
            );
            println!("================================================================================\n");

            if to_clean.is_empty() {
                println!(
                    "No files selected. Pass `--query \"...\"`, `--duplicates`, or `--safe-junk`."
                );
                return Ok(());
            }

            for e in to_clean.iter().take(25) {
                println!("  • {:>10} | {}", format_bytes(e.size_bytes), e.path);
            }
            if to_clean.len() > 25 {
                println!("  ... and {} more files", to_clean.len() - 25);
            }
            println!();

            if !execute {
                println!(
                    "DRY-RUN ONLY: No files were modified. Re-run with `--execute` to clean {} ({} files).",
                    format_bytes(total_clean_bytes),
                    to_clean.len()
                );
                return Ok(());
            }

            let mut cleaned_count = 0usize;
            let mut freed_bytes = 0u64;
            for e in to_clean {
                if delete_storage_entry(e, permanent).is_ok() {
                    cleaned_count += 1;
                    freed_bytes += e.size_bytes;
                }
            }

            println!(
                "Cleaned {} files ({} reclaimed)!",
                cleaned_count,
                format_bytes(freed_bytes)
            );
            Ok(())
        }
    }
}

fn print_entries_table(entries: &[StorageEntry], now_unix: i64) {
    println!(
        "  {:<11} {:<9} {:<10} {:<24} {}",
        "SIZE", "AGE", "TYPE", "AUTO-TAGS", "FILE NAME & PATH"
    );
    println!("  {}", "-".repeat(90));
    for e in entries {
        let tags: Vec<&str> = e.auto_tags.iter().map(|t| t.tag_str()).collect();
        let tags_str = if tags.is_empty() {
            "-".to_string()
        } else {
            tags.join(" ")
        };
        println!(
            "  {:<11} {:<9} {:<10} {:<24} {} ({})",
            format_bytes(e.size_bytes),
            format_age(e.days_old(now_unix)),
            e.category.short_label(),
            tags_str,
            e.name,
            e.parent_folder
        );
    }
}
