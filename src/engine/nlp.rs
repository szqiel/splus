use regex::Regex;

use crate::model::{
    format_bytes, AutoTag, FileCategory, ParsedNlpResult, ParsedNlpToken, SmartFilterRule,
};

fn parse_byte_amount(num_str: &str, unit_str: &str) -> u64 {
    let val: f64 = num_str.parse().unwrap_or(0.0);
    let mult: f64 = match unit_str.to_ascii_lowercase().as_str() {
        "b" | "byte" | "bytes" => 1.0,
        "kb" | "k" => 1024.0,
        "mb" | "m" => 1024.0 * 1024.0,
        "gb" | "g" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1024.0 * 1024.0,
    };
    (val * mult).round() as u64
}

fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "find"
            | "show"
            | "me"
            | "all"
            | "my"
            | "the"
            | "files"
            | "file"
            | "items"
            | "that"
            | "are"
            | "is"
            | "with"
            | "and"
            | "or"
            | "in"
            | "from"
            | "of"
            | "for"
            | "on"
            | "device"
            | "storage"
            | "folder"
            | "please"
            | "get"
            | "search"
            | "filter"
            | "list"
            | "added"
            | "within"
            | "most"
            | "latest"
            | "newest"
            | "a"
            | "an"
            | "to"
    )
}

fn consume_pattern<F>(working: &mut String, pattern: &str, mut handler: F)
where
    F: FnMut(&regex::Captures<'_>),
{
    let re = Regex::new(pattern).expect("valid NLP regex");
    let matches: Vec<String> = re
        .captures_iter(working)
        .map(|cap| {
            handler(&cap);
            cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()
        })
        .collect();

    for m in matches {
        if !m.is_empty() {
            *working = working.replacen(&m, " ", 1);
        }
    }
}

/// 100% Offline Deterministic Natural Language Parser (Zero External AI API).
/// Converts human queries like "old unused videos over 100mb in downloads" into structured rules.
pub fn parse_natural_language_query(query: &str) -> ParsedNlpResult {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return ParsedNlpResult::default();
    }

    let mut working = format!(" {} ", trimmed.to_ascii_lowercase());
    let mut tokens: Vec<ParsedNlpToken> = Vec::new();
    let mut rule = SmartFilterRule {
        search_query: trimmed.to_string(),
        ..Default::default()
    };

    // 1. Range size: "between 10mb and 500mb" or "from 50mb to 200mb"
    consume_pattern(
        &mut working,
        r"(?i)\b(?:between|from)\s+(\d+(?:\.\d+)?)\s*(kb|mb|gb|tb|b)\s+(?:and|to|-)\s+(\d+(?:\.\d+)?)\s*(kb|mb|gb|tb|b)\b",
        |cap| {
            let a = parse_byte_amount(&cap[1], &cap[2]);
            let b = parse_byte_amount(&cap[3], &cap[4]);
            let low = a.min(b);
            let high = a.max(b);
            rule.min_size_bytes = Some(low);
            rule.max_size_bytes = Some(high);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: format!("Size: {}..{}", format_bytes(low), format_bytes(high)),
                token_kind: "size".to_string(),
            });
        },
    );

    // 2. Min size: "over 100mb", "bigger than 50mb", "larger than 1gb", "> 20mb", ">= 10mb"
    consume_pattern(
        &mut working,
        r"(?i)(?:\b(?:over|above|bigger\s+than|larger\s+than|more\s+than|greater\s+than|at\s+least|exceeding)\s+|>\s*=?\s*)(\d+(?:\.\d+)?)\s*(kb|mb|gb|tb|b)\b",
        |cap| {
            let sz = parse_byte_amount(&cap[1], &cap[2]);
            rule.min_size_bytes = Some(sz);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: format!("Size >= {}", format_bytes(sz)),
                token_kind: "size".to_string(),
            });
        },
    );

    // 3. Max size: "under 10mb", "smaller than 5mb", "less than 50mb", "< 10mb"
    consume_pattern(
        &mut working,
        r"(?i)(?:\b(?:under|below|smaller\s+than|less\s+than|at\s+most)\s+|<\s*=?\s*)(\d+(?:\.\d+)?)\s*(kb|mb|gb|tb|b)\b",
        |cap| {
            let sz = parse_byte_amount(&cap[1], &cap[2]);
            rule.max_size_bytes = Some(sz);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: format!("Size <= {}", format_bytes(sz)),
                token_kind: "size".to_string(),
            });
        },
    );

    // 4. Explicit min age: "older than 6 months", "older than 30 days", "unused for 1 year"
    consume_pattern(
        &mut working,
        r"(?i)\b(?:older\s+than|more\s+than|untouched\s+for|unused\s+for)\s+(\d+)\s*(day|days|d|week|weeks|w|month|months|mo|year|years|yr|y)(?:\s+old)?\b",
        |cap| {
            let count: u64 = cap[1].parse().unwrap_or(0);
            let unit = cap[2].to_ascii_lowercase();
            let days = if unit.starts_with('w') {
                count * 7
            } else if unit.starts_with('m') {
                count * 30
            } else if unit.starts_with('y') {
                count * 365
            } else {
                count
            };
            rule.min_age_days = Some(days);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: format!("Age >= {}d", days),
                token_kind: "age".to_string(),
            });
        },
    );

    // 5. Recent / max age: "within a day/week/month", "last 7 days", "past 30 days", "today", "recent"
    consume_pattern(
        &mut working,
        r"(?i)\b(?:in\s+the\s+|within\s+(?:the\s+)?)?(?:last|past|within)\s+(\d+)\s*(hour|hours|hr|hrs|h|day|days|d|week|weeks|w|month|months|mo)\b",
        |cap| {
            let count: u64 = cap[1].parse().unwrap_or(1);
            let unit = cap[2].to_ascii_lowercase();
            let days = if unit.starts_with('h') {
                (count / 24).max(1)
            } else if unit.starts_with('w') {
                count * 7
            } else if unit.starts_with('m') {
                count * 30
            } else {
                count
            };
            rule.max_age_days = Some(days);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: format!("Age <= {}d", days),
                token_kind: "age".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:within\s+(?:a|one)\s+day|last\s+day|past\s+day|today|yesterday|just\s+added)\b",
        |cap| {
            rule.max_age_days = Some(1);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Within 24h (<= 1d)".to_string(),
                token_kind: "age".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:within\s+(?:a|one)\s+month|last\s+month|this\s+month|past\s+month)\b",
        |cap| {
            rule.max_age_days = Some(30);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Within 30d".to_string(),
                token_kind: "age".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:within\s+(?:a|one)\s+week|last\s+week|this\s+week|past\s+week|recent|recently|new)\b",
        |cap| {
            if rule.max_age_days.is_none() {
                rule.max_age_days = Some(7);
                tokens.push(ParsedNlpToken {
                    raw_text: cap[0].trim().to_string(),
                    label: "Within 7d".to_string(),
                    token_kind: "age".to_string(),
                });
            }
        },
    );

    // 6. Multi-word tags
    consume_pattern(&mut working, r"(?i)\bscreen\s*recordings?\b", |cap| {
        if !rule.auto_tags.contains(&AutoTag::ScreenRecording) {
            rule.auto_tags.push(AutoTag::ScreenRecording);
        }
        tokens.push(ParsedNlpToken {
            raw_text: cap[0].trim().to_string(),
            label: "#screen-recording".to_string(),
            token_kind: "tag".to_string(),
        });
    });

    consume_pattern(
        &mut working,
        r"(?i)\bburst\s*(?:photos?|shots?)?\b",
        |cap| {
            if !rule.auto_tags.contains(&AutoTag::BurstPhoto) {
                rule.auto_tags.push(AutoTag::BurstPhoto);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "#burst-photo".to_string(),
                token_kind: "tag".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:dev\s*artifacts?|node_modules|build\s*artifacts?|build\s*cache)\b",
        |cap| {
            if !rule.auto_tags.contains(&AutoTag::DevArtifact) {
                rule.auto_tags.push(AutoTag::DevArtifact);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "#dev-artifact".to_string(),
                token_kind: "tag".to_string(),
            });
        },
    );

    // 7. Qualitative size & age words
    consume_pattern(
        &mut working,
        r"(?i)\b(?:huge|giant|massive|enormous|bloated)\b",
        |cap| {
            if rule.min_size_bytes.is_none() {
                rule.min_size_bytes = Some(150 * 1024 * 1024);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Huge (>= 150 MB)".to_string(),
                token_kind: "size".to_string(),
            });
        },
    );

    consume_pattern(&mut working, r"(?i)\b(?:large|big|heavy)\b", |cap| {
        if rule.min_size_bytes.is_none() {
            rule.min_size_bytes = Some(50 * 1024 * 1024);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Large (>= 50 MB)".to_string(),
                token_kind: "size".to_string(),
            });
        }
    });

    consume_pattern(&mut working, r"(?i)\b(?:tiny|small)\b", |cap| {
        if rule.max_size_bytes.is_none() {
            rule.max_size_bytes = Some(5 * 1024 * 1024);
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Small (<= 5 MB)".to_string(),
                token_kind: "size".to_string(),
            });
        }
    });

    consume_pattern(
        &mut working,
        r"(?i)\b(?:old|unused|forgotten|stale|ancient|untouched)\b",
        |cap| {
            if rule.min_age_days.is_none() {
                rule.min_age_days = Some(120);
                tokens.push(ParsedNlpToken {
                    raw_text: cap[0].trim().to_string(),
                    label: "Old/Unused (>= 120d)".to_string(),
                    token_kind: "age".to_string(),
                });
            }
        },
    );

    // 8. Tags & Categories
    consume_pattern(
        &mut working,
        r"(?i)\b(?:screenshots?|screen\s*shots?|screen\s*captures?)\b",
        |cap| {
            if !rule.auto_tags.contains(&AutoTag::Screenshot) {
                rule.auto_tags.push(AutoTag::Screenshot);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "#screenshot".to_string(),
                token_kind: "tag".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:whatsapp|telegram|messenger|wechat|discord|chat\s*media)\b",
        |cap| {
            if !rule.auto_tags.contains(&AutoTag::MessengerMedia) {
                rule.auto_tags.push(AutoTag::MessengerMedia);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "#messenger-media".to_string(),
                token_kind: "tag".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:duplicates?|copies|redundant|cloned)\b",
        |cap| {
            rule.only_duplicates = true;
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Duplicates Only".to_string(),
                token_kind: "flag".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:junk|clutter|cache|temp|temporary|trash|garbage)\b",
        |cap| {
            rule.only_junk = true;
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Junk & Cache".to_string(),
                token_kind: "flag".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:videos?|movies?|clips?|films?)\b",
        |cap| {
            if !rule.categories.contains(&FileCategory::Video) {
                rule.categories.push(FileCategory::Video);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Type: Video".to_string(),
                token_kind: "category".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:photos?|images?|pictures?|pics)\b",
        |cap| {
            if !rule.categories.contains(&FileCategory::Image) {
                rule.categories.push(FileCategory::Image);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Type: Image".to_string(),
                token_kind: "category".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:audio|music|songs?|podcasts?|voice\s*notes?)\b",
        |cap| {
            if !rule.categories.contains(&FileCategory::Audio) {
                rule.categories.push(FileCategory::Audio);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Type: Audio".to_string(),
                token_kind: "category".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:documents?|docs?|spreadsheets?|presentations?|slides?)\b",
        |cap| {
            if !rule.categories.contains(&FileCategory::Document) {
                rule.categories.push(FileCategory::Document);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Type: Document".to_string(),
                token_kind: "category".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:archives?|zips?|compressed|backups?)\b",
        |cap| {
            if !rule.categories.contains(&FileCategory::Archive) {
                rule.categories.push(FileCategory::Archive);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Type: Archive".to_string(),
                token_kind: "category".to_string(),
            });
        },
    );

    consume_pattern(
        &mut working,
        r"(?i)\b(?:apks?|installers?|packages?|setups?)\b",
        |cap| {
            if !rule.categories.contains(&FileCategory::Installer) {
                rule.categories.push(FileCategory::Installer);
            }
            if !rule.auto_tags.contains(&AutoTag::Installer) {
                rule.auto_tags.push(AutoTag::Installer);
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: "Installers/APKs".to_string(),
                token_kind: "category".to_string(),
            });
        },
    );

    // 9. Explicit extensions
    consume_pattern(
        &mut working,
        r"(?i)\b\.?(mp4|mkv|mov|avi|jpg|jpeg|png|heic|webp|gif|mp3|flac|wav|m4a|pdf|docx?|xlsx?|pptx?|zip|rar|7z|tar|apk|xapk|msi|exe|iso|log|tmp)s?\b",
        |cap| {
            let ext = cap[1].to_ascii_lowercase();
            if !rule.extensions.contains(&ext) {
                rule.extensions.push(ext.clone());
            }
            tokens.push(ParsedNlpToken {
                raw_text: cap[0].trim().to_string(),
                label: format!(".{}", ext),
                token_kind: "extension".to_string(),
            });
        },
    );

    // 10. Folder keywords
    consume_pattern(&mut working, r"(?i)\b(?:downloads?|download)\b", |cap| {
        rule.folders.push("download".to_string());
        tokens.push(ParsedNlpToken {
            raw_text: cap[0].trim().to_string(),
            label: "Folder: Downloads".to_string(),
            token_kind: "folder".to_string(),
        });
    });

    consume_pattern(&mut working, r"(?i)\b(?:dcim|camera)\b", |cap| {
        rule.folders.push("dcim".to_string());
        tokens.push(ParsedNlpToken {
            raw_text: cap[0].trim().to_string(),
            label: "Folder: DCIM".to_string(),
            token_kind: "folder".to_string(),
        });
    });

    // Remaining keywords
    let remaining_keywords: Vec<String> = working
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
                .collect::<String>()
        })
        .filter(|w| w.len() >= 2 && !is_stop_word(w))
        .collect();

    ParsedNlpResult {
        original_query: trimmed.to_string(),
        remaining_keywords,
        tokens,
        derived_rule: rule,
    }
}
