use clap::{Parser, Subcommand, ValueEnum};

use crate::engine::duplicates::{auto_select_redundant_ids, detect_duplicate_clusters};
use crate::engine::filter::evaluate_smart_filter;
use crate::engine::junk::scan_junk_buckets;
use crate::engine::treemap::{build_terminal_treemap, render_ascii_treemap, TreemapMode};
use crate::model::{
    format_age, format_bytes, FileCategory, RecentWindow, SmartFilterRule, SortBy, StorageEntry,
};
use crate::scanner::{
    adb::install_standalone_adb, delete_storage_entry, discover_all_android_devices,
    scan_and_enrich_target_with_options, ScanOptions,
};
use crate::tui::run_interactive_tui;

#[derive(Parser, Debug)]
#[command(
    name = "splus",
    version,
    about = "Minimalist smart storage filter & cleaner for Windows and Android USB"
)]
pub struct Cli {
    /// Folder path (e.g. C:\Users\...\Downloads)
    pub target: Option<String>,

    /// Scan a USB-connected Android phone
    #[arg(short, long, global = true)]
    pub android: bool,

    /// Specific Android phone name or serial
    #[arg(long, global = true)]
    pub device: Option<String>,

    /// Maximum folder depth
    #[arg(long, global = true)]
    pub max_depth: Option<usize>,

    /// Include dev folders (node_modules, .git, .cargo) and files under 1 MB on drive roots
    #[arg(long, global = true)]
    pub all: bool,

    /// Initial search filter when opening UI
    #[arg(short, long)]
    pub query: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Open the interactive terminal workspace
    Tui {
        /// Folder path on Windows or Android
        path: Option<String>,
        /// Optional initial search filter
        #[arg(short, long)]
        query: Option<String>,
    },

    /// Show storage summary, visual map, and largest files
    Scan {
        /// Folder path on Windows or Android
        path: Option<String>,
        /// Group visual map by: category, folder, or files
        #[arg(short, long, value_enum, default_value_t = GroupModeArg::Category)]
        group_by: GroupModeArg,
        /// Number of largest files to show
        #[arg(short, long, default_value_t = 10)]
        top: usize,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },

    /// Find files using plain English (e.g. "old videos over 100mb")
    Query {
        /// Plain English filter (e.g. "old unused videos over 100mb in downloads")
        nl_query: String,
        /// Optional folder path
        #[arg(short, long)]
        path: Option<String>,
        /// Minimum size in MB
        #[arg(long)]
        min_mb: Option<u64>,
        /// Minimum age in days
        #[arg(long)]
        min_days: Option<u64>,
        /// Max results to show
        #[arg(short, long, default_value_t = 25)]
        limit: usize,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },

    /// Show recently added files (within a day, a week, or a month)
    Recent {
        /// Time window: day (24h), week (7d), or month (30d)
        #[arg(value_enum, default_value_t = RecentWindowArg::Week)]
        window: RecentWindowArg,
        /// Optional folder path on Windows or Android
        #[arg(short, long)]
        path: Option<String>,
        /// Minimum file size in MB
        #[arg(long)]
        min_mb: Option<u64>,
        /// Sort by newest timestamp first instead of largest size first
        #[arg(long)]
        newest: bool,
        /// Max results to show
        #[arg(short, long, default_value_t = 25)]
        limit: usize,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },

    /// Find duplicate files and extra copies
    Dupes {
        /// Folder path on Windows or Android
        path: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },

    /// Find temp files, cache, and leftover installers
    Junk {
        /// Folder path on Windows or Android
        path: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },

    /// Clean files (preview by default; add --execute to delete)
    Clean {
        /// Folder path on Windows or Android
        path: Option<String>,
        /// Plain English filter to select files
        #[arg(short, long)]
        query: Option<String>,
        /// Select extra duplicate copies (keeps 1 original per group)
        #[arg(long)]
        duplicates: bool,
        /// Select safe junk (temp, cache, and leftover installers)
        #[arg(long)]
        safe_junk: bool,
        /// Actually delete files (without --execute, only previews)
        #[arg(long)]
        execute: bool,
        /// Permanently delete instead of sending to Trash
        #[arg(long)]
        permanent: bool,
    },

    /// List connected Android phones over USB
    Devices,

    /// Download standalone Android USB tool (adb.exe)
    AdbSetup,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum RecentWindowArg {
    Day,
    Week,
    Month,
}

impl From<RecentWindowArg> for RecentWindow {
    fn from(val: RecentWindowArg) -> Self {
        match val {
            RecentWindowArg::Day => RecentWindow::Day,
            RecentWindowArg::Week => RecentWindow::Week,
            RecentWindowArg::Month => RecentWindow::Month,
        }
    }
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
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                include_all: cli.all,
                show_progress: true,
                ..Default::default()
            };
            let (label, kind, entries) = scan_and_enrich_target_with_options(
                cli.target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
            )?;
            run_interactive_tui(label, kind, entries, cli.query)
        }

        Some(Commands::Tui { path, query }) => {
            let target = path.or(cli.target);
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                include_all: cli.all,
                show_progress: true,
                ..Default::default()
            };
            let (label, kind, entries) = scan_and_enrich_target_with_options(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
            )?;
            run_interactive_tui(label, kind, entries, query.or(cli.query))
        }

        Some(Commands::Devices) => {
            let devs = discover_all_android_devices();
            println!("\n  Devices");
            if devs.is_empty() {
                println!("  • No phone detected over USB.");
                println!("    Plug in your phone and choose 'File Transfer' on the screen.\n");
            } else {
                for d in &devs {
                    println!("  ● {} ({})", d.name, d.status);
                }
                println!();
            }
            Ok(())
        }

        Some(Commands::AdbSetup) => {
            let adb_path = install_standalone_adb()?;
            println!("\n  Ready: {}\n", adb_path.display());
            Ok(())
        }

        Some(Commands::Scan {
            path,
            group_by,
            top,
            json,
        }) => {
            let target = path.or(cli.target);
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                skip_hashes: true,
                include_all: cli.all,
                show_progress: !json,
                ..Default::default()
            };
            let (label, kind, entries) = scan_and_enrich_target_with_options(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
            )?;

            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
                return Ok(());
            }

            let total_bytes: u64 = entries.iter().map(|e| e.size_bytes).sum();
            let now_unix = chrono::Utc::now().timestamp();
            let all_indices: Vec<usize> = (0..entries.len()).collect();

            println!(
                "\n  splus • {} ({})\n  {} across {} files\n",
                label,
                kind.label(),
                format_bytes(total_bytes),
                entries.len()
            );

            // Clean Category Breakdown
            println!("  Breakdown");
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
                let bar_len = ((pct / 100.0) * 20.0).round() as usize;
                let bar = "━".repeat(bar_len) + &"─".repeat(20usize.saturating_sub(bar_len));
                println!(
                    "  {:<14} {}  {:>4.0}%  {:>9}",
                    cat.label(),
                    bar,
                    pct,
                    format_bytes(cat_bytes)
                );
            }
            println!();

            // Rounded Visual Map
            let mode: TreemapMode = group_by.into();
            println!("  Visual Map ({})", mode.label());
            let rects = build_terminal_treemap(&entries, &all_indices, mode, 74, 14, 8);
            for line in render_ascii_treemap(&rects, 74, 14).lines() {
                println!("  {}", line);
            }
            println!();

            // Largest Files
            let mut sorted = entries.clone();
            sorted.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
            println!("  Largest Files");
            print_minimal_file_list(&sorted[..top.min(sorted.len())], now_unix);
            println!();
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
            let q_lower = nl_query.to_ascii_lowercase();
            let needs_hashes = q_lower.contains("dup") || q_lower.contains("copy") || q_lower.contains("copies");
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                min_size_bytes: min_mb.map(|mb| mb * 1024 * 1024),
                skip_hashes: !needs_hashes,
                include_all: cli.all,
                show_progress: !json,
                ..Default::default()
            };
            let (_label, _kind, entries) = scan_and_enrich_target_with_options(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
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

            let (matched_indices, matched_bytes, _nlp) =
                evaluate_smart_filter(&entries, &rule, now_unix);

            let matched_entries: Vec<StorageEntry> =
                matched_indices.iter().map(|&i| entries[i].clone()).collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&matched_entries)?);
                return Ok(());
            }

            println!(
                "\n  > {}   ({} files • {})\n",
                rule.search_query,
                matched_entries.len(),
                format_bytes(matched_bytes)
            );

            if matched_entries.is_empty() {
                println!("  No matching files found.\n");
            } else {
                print_minimal_file_list(&matched_entries[..limit.min(matched_entries.len())], now_unix);
                println!();
            }
            Ok(())
        }

        Some(Commands::Recent {
            window,
            path,
            min_mb,
            newest,
            limit,
            json,
        }) => {
            let target = path.or(cli.target);
            let win: RecentWindow = window.into();
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                min_size_bytes: min_mb.map(|mb| mb * 1024 * 1024),
                max_age_days: Some(win.max_days()),
                skip_hashes: true,
                include_all: cli.all,
                show_progress: !json,
                ..Default::default()
            };
            let (_label, _kind, entries) = scan_and_enrich_target_with_options(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
            )?;

            let now_unix = chrono::Utc::now().timestamp();
            let rule = SmartFilterRule {
                max_age_days: Some(win.max_days()),
                min_size_bytes: min_mb.map(|mb| mb * 1024 * 1024),
                sort_by: if newest {
                    SortBy::AgeAsc
                } else {
                    SortBy::SizeDesc
                },
                ..Default::default()
            };

            let (matched_indices, matched_bytes, _) =
                evaluate_smart_filter(&entries, &rule, now_unix);

            let matched_entries: Vec<StorageEntry> =
                matched_indices.iter().map(|&i| entries[i].clone()).collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&matched_entries)?);
                return Ok(());
            }

            println!(
                "\n  > Recently Added • {}   ({} files • {})\n",
                win.title(),
                matched_entries.len(),
                format_bytes(matched_bytes)
            );

            if matched_entries.is_empty() {
                println!("  No files added in this time window.\n");
            } else {
                // Show Top Recent Folders so the user immediately sees which app/folder ate their space
                let mut folder_totals: std::collections::HashMap<&str, (u64, usize)> =
                    std::collections::HashMap::new();
                for e in &matched_entries {
                    let item = folder_totals.entry(e.parent_folder.as_str()).or_insert((0, 0));
                    item.0 += e.size_bytes;
                    item.1 += 1;
                }
                let mut top_folders: Vec<(&str, u64, usize)> = folder_totals
                    .into_iter()
                    .map(|(f, (b, c))| (f, b, c))
                    .collect();
                top_folders.sort_by(|a, b| b.1.cmp(&a.1));

                println!("  Top Recent Folders");
                for (folder, bytes, count) in top_folders.iter().take(6) {
                    println!(
                        "  {:>9}   {}  ({} files)",
                        format_bytes(*bytes),
                        folder,
                        count
                    );
                }
                println!();

                println!(
                    "  {}",
                    if newest {
                        "Newest Files"
                    } else {
                        "Largest Recent Files"
                    }
                );
                print_minimal_file_list(
                    &matched_entries[..limit.min(matched_entries.len())],
                    now_unix,
                );
                println!();
            }
            Ok(())
        }

        Some(Commands::Dupes { path, json }) => {
            let target = path.or(cli.target);
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                include_all: cli.all,
                show_progress: !json,
                ..Default::default()
            };
            let (_label, _kind, entries) = scan_and_enrich_target_with_options(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
            )?;

            let clusters = detect_duplicate_clusters(&entries);
            if json {
                println!("{}", serde_json::to_string_pretty(&clusters)?);
                return Ok(());
            }

            let total_wasted: u64 = clusters.iter().map(|c| c.wasted_bytes).sum();
            println!(
                "\n  > Duplicates   ({} groups • {} extra space)\n",
                clusters.len(),
                format_bytes(total_wasted)
            );

            if clusters.is_empty() {
                println!("  No duplicate files found.\n");
                return Ok(());
            }

            for c in &clusters {
                println!("  • {}  (+{} extra)", c.title, format_bytes(c.wasted_bytes));
                for &id in &c.entry_ids {
                    if let Some(e) = entries.iter().find(|x| x.id == id) {
                        let mark = if e.id == c.keep_id { "keep " } else { "extra" };
                        println!(
                            "    [{}] {:>9}  {}",
                            mark,
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
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                skip_hashes: true,
                include_all: cli.all,
                show_progress: !json,
                ..Default::default()
            };
            let (_label, _kind, entries) = scan_and_enrich_target_with_options(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
            )?;

            let now_unix = chrono::Utc::now().timestamp();
            let buckets = scan_junk_buckets(&entries, now_unix);
            if json {
                println!("{}", serde_json::to_string_pretty(&buckets)?);
                return Ok(());
            }

            let total_junk: u64 = buckets.iter().map(|b| b.total_bytes).sum();
            println!(
                "\n  > Temp & Junk   ({} across {} categories)\n",
                format_bytes(total_junk),
                buckets.len()
            );

            for b in &buckets {
                println!(
                    "  • {:<32} {:>10}  ({} files)",
                    b.title,
                    format_bytes(b.total_bytes),
                    b.entry_ids.len()
                );
                for &id in b.entry_ids.iter().take(4) {
                    if let Some(e) = entries.iter().find(|x| x.id == id) {
                        println!("      {:>9}  {}", format_bytes(e.size_bytes), e.name);
                    }
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
            let opts = ScanOptions {
                max_depth: cli.max_depth,
                skip_hashes: !duplicates,
                include_all: cli.all,
                show_progress: true,
                ..Default::default()
            };
            let (_label, _kind, entries) = scan_and_enrich_target_with_options(
                target.as_deref(),
                cli.android,
                cli.device.as_deref(),
                &opts,
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

            println!(
                "\n  > Clean {}   ({} files • {})\n",
                if execute { "" } else { "Preview" },
                to_clean.len(),
                format_bytes(total_clean_bytes)
            );

            if to_clean.is_empty() {
                println!(
                    "  Nothing selected. Use --query \"...\", --duplicates, or --safe-junk.\n"
                );
                return Ok(());
            }

            for e in to_clean.iter().take(20) {
                println!("  • {:>9}  {} ({})", format_bytes(e.size_bytes), e.name, e.parent_folder);
            }
            if to_clean.len() > 20 {
                println!("  ... and {} more", to_clean.len() - 20);
            }
            println!();

            if !execute {
                println!(
                    "  Preview only — no files deleted. Add --execute to clean {}.\n",
                    format_bytes(total_clean_bytes)
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
                "  Done: cleaned {} files ({}).\n",
                cleaned_count,
                format_bytes(freed_bytes)
            );
            Ok(())
        }
    }
}

fn print_minimal_file_list(entries: &[StorageEntry], now_unix: i64) {
    for e in entries {
        let tags: Vec<&str> = e.auto_tags.iter().take(2).map(|t| t.tag_str()).collect();
        let tags_str = if tags.is_empty() {
            String::new()
        } else {
            format!(" • {}", tags.join(" · "))
        };
        println!(
            "  {:>9}   {:<36}  {} • {}{}",
            format_bytes(e.size_bytes),
            e.name,
            e.parent_folder,
            format_age(e.days_old(now_unix)),
            tags_str
        );
    }
}
