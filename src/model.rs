use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageTargetKind {
    WindowsLocal,
    AndroidAdb,
    AndroidMtp,
}

impl StorageTargetKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::WindowsLocal => "This PC",
            Self::AndroidAdb => "Phone (USB)",
            Self::AndroidMtp => "Phone (USB)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileCategory {
    Video,
    Image,
    Audio,
    Document,
    Archive,
    Installer,
    CacheTemp,
    CodeData,
    Other,
}

impl FileCategory {
    pub const ALL: [FileCategory; 9] = [
        Self::Video,
        Self::Image,
        Self::Audio,
        Self::Document,
        Self::Archive,
        Self::Installer,
        Self::CacheTemp,
        Self::CodeData,
        Self::Other,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Self::Video => "Videos",
            Self::Image => "Photos",
            Self::Audio => "Audio",
            Self::Document => "Docs",
            Self::Archive => "Archives",
            Self::Installer => "Apps & Setup",
            Self::CacheTemp => "Temp & Cache",
            Self::CodeData => "Code & Data",
            Self::Other => "Other",
        }
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Image => "Photo",
            Self::Audio => "Audio",
            Self::Document => "Doc",
            Self::Archive => "Zip",
            Self::Installer => "App",
            Self::CacheTemp => "Temp",
            Self::CodeData => "Data",
            Self::Other => "Other",
        }
    }

    pub fn from_extension_and_path(ext: &str, path: &str) -> Self {
        let e = ext.trim_start_matches('.').to_ascii_lowercase();
        let lower_path = path.to_ascii_lowercase();

        if lower_path.contains("/cache/")
            || lower_path.contains("\\cache\\")
            || lower_path.contains("/.thumbnails")
            || lower_path.contains("\\temp\\")
            || matches!(
                e.as_str(),
                "tmp" | "temp" | "log" | "bak" | "thumbdata" | "nomedia" | "cache" | "dmp"
            )
        {
            return Self::CacheTemp;
        }

        match e.as_str() {
            "mp4" | "mkv" | "mov" | "avi" | "webm" | "3gp" | "m4v" | "flv" | "wmv" => Self::Video,
            "jpg" | "jpeg" | "png" | "webp" | "heic" | "gif" | "bmp" | "svg" | "raw" | "dng"
            | "tiff" | "ico" => Self::Image,
            "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "opus" | "wma" => Self::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "csv"
            | "epub" | "rtf" | "odt" => Self::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "tgz" => Self::Archive,
            "apk" | "xapk" | "apkm" | "aab" | "ipa" | "msi" | "exe" | "dmg" | "deb" => {
                Self::Installer
            }
            "rs" | "py" | "js" | "ts" | "tsx" | "json" | "toml" | "yaml" | "yml" | "sql" | "db"
            | "sqlite" | "sqlite3" | "xml" | "html" | "css" | "c" | "cpp" | "h" | "go" | "java"
            | "kt" => Self::CodeData,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AutoTag {
    Screenshot,
    ScreenRecording,
    Installer,
    CacheTemp,
    BurstPhoto,
    MessengerMedia,
    Bloated,
    Forgotten,
    Duplicate,
    EmptyFile,
    DevArtifact,
}

impl AutoTag {
    pub const ALL: [AutoTag; 11] = [
        Self::Screenshot,
        Self::ScreenRecording,
        Self::Installer,
        Self::CacheTemp,
        Self::BurstPhoto,
        Self::MessengerMedia,
        Self::Bloated,
        Self::Forgotten,
        Self::Duplicate,
        Self::EmptyFile,
        Self::DevArtifact,
    ];

    pub fn tag_str(&self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::ScreenRecording => "screen-rec",
            Self::Installer => "installer",
            Self::CacheTemp => "temp",
            Self::BurstPhoto => "burst",
            Self::MessengerMedia => "chat",
            Self::Bloated => "large",
            Self::Forgotten => "unused",
            Self::Duplicate => "duplicate",
            Self::EmptyFile => "empty",
            Self::DevArtifact => "dev-cache",
        }
    }

    pub fn priority(&self) -> u8 {
        match self {
            Self::Duplicate => 1,
            Self::Bloated => 2,
            Self::CacheTemp => 3,
            Self::Installer => 4,
            Self::DevArtifact => 5,
            Self::EmptyFile => 6,
            Self::Forgotten => 7,
            Self::BurstPhoto => 8,
            Self::MessengerMedia => 9,
            Self::ScreenRecording => 10,
            Self::Screenshot => 11,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot",
            Self::ScreenRecording => "Screen recording",
            Self::Installer => "App installer or setup file",
            Self::CacheTemp => "Temporary or cache file",
            Self::BurstPhoto => "Similar photo taken in a burst",
            Self::MessengerMedia => "Saved from WhatsApp, Telegram, or chat",
            Self::Bloated => "Very large file",
            Self::Forgotten => "Not opened in 6+ months",
            Self::Duplicate => "Extra copy of another file",
            Self::EmptyFile => "Empty file (0 B)",
            Self::DevArtifact => "Project build or package cache",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntry {
    pub id: usize,
    pub name: String,
    pub path: String,
    pub parent_folder: String,
    pub extension: String,
    pub size_bytes: u64,
    pub modified_unix: i64,
    pub accessed_unix: i64,
    pub category: FileCategory,
    pub auto_tags: Vec<AutoTag>,
    pub custom_tags: Vec<String>,
    pub content_hash: Option<String>,
    pub whitelisted: bool,
    pub target_kind: StorageTargetKind,
    pub device_id: Option<String>,
}

impl StorageEntry {
    pub fn days_old(&self, now_unix: i64) -> u64 {
        let diff = (now_unix - self.modified_unix).max(0) as u64;
        diff / 86_400
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecentWindow {
    Day,
    Week,
    Month,
}

impl RecentWindow {
    pub fn max_days(&self) -> u64 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Day => Self::Week,
            Self::Week => Self::Month,
            Self::Month => Self::Day,
        }
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Day => "Within a day (24h)",
            Self::Week => "Within a week (7d)",
            Self::Month => "Within a month (30d)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortBy {
    SizeDesc,
    SizeAsc,
    AgeDesc,
    AgeAsc,
    NameAsc,
}

impl SortBy {
    pub fn next(&self) -> Self {
        match self {
            Self::SizeDesc => Self::AgeDesc,
            Self::AgeDesc => Self::SizeAsc,
            Self::SizeAsc => Self::AgeAsc,
            Self::AgeAsc => Self::NameAsc,
            Self::NameAsc => Self::SizeDesc,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::SizeDesc => "Largest first",
            Self::SizeAsc => "Smallest first",
            Self::AgeDesc => "Oldest first",
            Self::AgeAsc => "Newest first",
            Self::NameAsc => "Name A–Z",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartFilterRule {
    pub search_query: String,
    pub categories: Vec<FileCategory>,
    pub min_size_bytes: Option<u64>,
    pub max_size_bytes: Option<u64>,
    pub min_age_days: Option<u64>,
    pub max_age_days: Option<u64>,
    pub folders: Vec<String>,
    pub extensions: Vec<String>,
    pub auto_tags: Vec<AutoTag>,
    pub custom_tags: Vec<String>,
    pub only_duplicates: bool,
    pub only_junk: bool,
    pub exclude_whitelisted: bool,
    pub sort_by: SortBy,
}

impl Default for SmartFilterRule {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            categories: Vec::new(),
            min_size_bytes: None,
            max_size_bytes: None,
            min_age_days: None,
            max_age_days: None,
            folders: Vec::new(),
            extensions: Vec::new(),
            auto_tags: Vec::new(),
            custom_tags: Vec::new(),
            only_duplicates: false,
            only_junk: false,
            exclude_whitelisted: false,
            sort_by: SortBy::SizeDesc,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedNlpToken {
    pub raw_text: String,
    pub label: String,
    pub token_kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedNlpResult {
    pub original_query: String,
    pub remaining_keywords: Vec<String>,
    pub tokens: Vec<ParsedNlpToken>,
    pub derived_rule: SmartFilterRule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFilterPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub nl_query: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuplicateReason {
    ExactHash,
    RenamedCopy,
    BurstSequence,
}

impl DuplicateReason {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ExactHash => "Identical file",
            Self::RenamedCopy => "Renamed copy",
            Self::BurstSequence => "Photo burst",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateCluster {
    pub title: String,
    pub reason: DuplicateReason,
    pub entry_ids: Vec<usize>,
    pub keep_id: usize,
    pub total_bytes: u64,
    pub wasted_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyLevel {
    Safe,
    Review,
    Caution,
}

impl SafetyLevel {
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Safe => "Safe",
            Self::Review => "Review",
            Self::Caution => "Careful",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JunkBucket {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub safety: SafetyLevel,
    pub entry_ids: Vec<usize>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreemapRect {
    pub label: String,
    pub sublabel: String,
    pub size_bytes: u64,
    pub percentage: f64,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub category: Option<FileCategory>,
    pub folder: Option<String>,
    pub entry_id: Option<usize>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AndroidDevice {
    pub id: String,
    pub name: String,
    pub mode: StorageTargetKind,
    pub status: String,
    pub root_path: String,
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    if b >= TB {
        format!("{:.2} TB", b / TB)
    } else if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_age(days: u64) -> String {
    if days == 0 {
        "Today".to_string()
    } else if days == 1 {
        "1d ago".to_string()
    } else if days < 30 {
        format!("{}d ago", days)
    } else if days < 365 {
        format!("{}mo ago", days / 30)
    } else {
        format!("{:.1}y ago", days as f64 / 365.0)
    }
}
