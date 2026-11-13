use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use rayon::prelude::*;

use crate::engine::autotag::normalize_base_filename;
use crate::model::{
    AutoTag, DuplicateCluster, DuplicateReason, FileCategory, StorageEntry, StorageTargetKind,
};
use crate::scanner::adb::compute_adb_hashes_batch;

/// Computes a fast BLAKE3 fingerprint for a local Windows file.
/// For large files (> 128 MB), hashes the first 64 KB, middle 64 KB, and last 64 KB + file size
/// for sub-millisecond performance; for files <= 128 MB, hashes the entire file.
pub fn compute_local_blake3_hash(path: &str, size_bytes: u64) -> Option<String> {
    if size_bytes == 0 {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();

    const CHUNK: u64 = 64 * 1024;
    if size_bytes <= 128 * 1024 * 1024 {
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    } else {
        let mut buf = vec![0u8; CHUNK as usize];
        let n1 = file.read(&mut buf).ok()?;
        hasher.update(&buf[..n1]);

        file.seek(SeekFrom::Start(size_bytes / 2)).ok()?;
        let n2 = file.read(&mut buf).ok()?;
        hasher.update(&buf[..n2]);

        file.seek(SeekFrom::Start(size_bytes.saturating_sub(CHUNK)))
            .ok()?;
        let n3 = file.read(&mut buf).ok()?;
        hasher.update(&buf[..n3]);
        hasher.update(&size_bytes.to_le_bytes());
    }

    Some(hasher.finalize().to_hex().to_string())
}

/// Populates content hashes only for entries that share an identical `size_bytes` with at least one other file.
/// This two-stage strategy skips >90% of disk/USB I/O.
pub fn populate_hashes_for_size_collisions(entries: &mut [StorageEntry]) {
    // On massive scans (>15,000 files), only hash colliding files >= 512 KB to avoid thousands of tiny disk reads
    let min_hash_size = if entries.len() > 15_000 {
        512 * 1024
    } else {
        1
    };

    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        if entry.size_bytes >= min_hash_size {
            by_size.entry(entry.size_bytes).or_default().push(idx);
        }
    }

    let colliding_indices: Vec<usize> = by_size
        .into_values()
        .filter(|group| group.len() > 1)
        .flatten()
        .collect();

    if colliding_indices.is_empty() {
        return;
    }

    // 1. Parallel BLAKE3 hashing for WindowsLocal files
    let local_jobs: Vec<(usize, String, u64)> = colliding_indices
        .iter()
        .copied()
        .filter(|&idx| {
            entries[idx].target_kind == StorageTargetKind::WindowsLocal
                && entries[idx].content_hash.is_none()
        })
        .map(|idx| (idx, entries[idx].path.clone(), entries[idx].size_bytes))
        .collect();

    let local_results: Vec<(usize, Option<String>)> = local_jobs
        .into_par_iter()
        .map(|(idx, path, sz)| (idx, compute_local_blake3_hash(&path, sz)))
        .collect();

    for (idx, hash_opt) in local_results {
        if let Some(h) = hash_opt {
            entries[idx].content_hash = Some(h);
        }
    }

    // 2. Batched on-device sha256sum for AndroidAdb files
    let adb_indices: Vec<usize> = colliding_indices
        .iter()
        .copied()
        .filter(|&idx| {
            entries[idx].target_kind == StorageTargetKind::AndroidAdb
                && entries[idx].content_hash.is_none()
        })
        .collect();

    if !adb_indices.is_empty() {
        let device_id = entries[adb_indices[0]].device_id.clone();
        let remote_paths: Vec<String> = adb_indices
            .iter()
            .map(|&idx| entries[idx].path.clone())
            .collect();
        let hash_map = compute_adb_hashes_batch(device_id.as_deref(), &remote_paths);
        for idx in adb_indices {
            if let Some(h) = hash_map.get(&entries[idx].path) {
                entries[idx].content_hash = Some(h.clone());
            }
        }
    }
}

/// Detects duplicate clusters (Exact Hash, Renamed Copy, and Burst Photo sequences).
pub fn detect_duplicate_clusters(entries: &[StorageEntry]) -> Vec<DuplicateCluster> {
    let mut clusters: Vec<DuplicateCluster> = Vec::new();
    let mut assigned: HashSet<usize> = HashSet::new();

    // 1. Exact content hash clusters
    let mut by_hash: HashMap<(String, u64), Vec<usize>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        if entry.size_bytes == 0 {
            continue;
        }
        if let Some(ref h) = entry.content_hash {
            by_hash
                .entry((h.clone(), entry.size_bytes))
                .or_default()
                .push(idx);
        }
    }

    for ((_, _), mut group) in by_hash {
        if group.len() > 1 {
            group.sort_by_key(|&i| entries[i].modified_unix);
            for &i in &group {
                assigned.insert(i);
            }
            let keep_idx = group
                .iter()
                .copied()
                .find(|&i| entries[i].whitelisted)
                .unwrap_or(group[0]);
            let total_bytes: u64 = group.iter().map(|&i| entries[i].size_bytes).sum();
            let wasted_bytes = total_bytes.saturating_sub(entries[keep_idx].size_bytes);

            clusters.push(DuplicateCluster {
                title: normalize_base_filename(&entries[keep_idx].name),
                reason: DuplicateReason::ExactHash,
                entry_ids: group.into_iter().map(|i| entries[i].id).collect(),
                keep_id: entries[keep_idx].id,
                total_bytes,
                wasted_bytes,
            });
        }
    }

    // 2. Renamed copy clusters (e.g. "report (1).pdf" and "report.pdf" with identical size)
    let mut by_norm: HashMap<(String, u64), Vec<usize>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        if assigned.contains(&idx) || entry.size_bytes == 0 {
            continue;
        }
        let norm = normalize_base_filename(&entry.name);
        by_norm
            .entry((norm, entry.size_bytes))
            .or_default()
            .push(idx);
    }

    for ((norm_title, _), mut group) in by_norm {
        if group.len() > 1 {
            group.sort_by_key(|&i| entries[i].modified_unix);
            for &i in &group {
                assigned.insert(i);
            }
            let keep_idx = group
                .iter()
                .copied()
                .find(|&i| entries[i].whitelisted)
                .unwrap_or(group[0]);
            let total_bytes: u64 = group.iter().map(|&i| entries[i].size_bytes).sum();
            let wasted_bytes = total_bytes.saturating_sub(entries[keep_idx].size_bytes);

            clusters.push(DuplicateCluster {
                title: norm_title,
                reason: DuplicateReason::RenamedCopy,
                entry_ids: group.into_iter().map(|i| entries[i].id).collect(),
                keep_id: entries[keep_idx].id,
                total_bytes,
                wasted_bytes,
            });
        }
    }

    // 3. Burst photo sequences in the same folder
    let mut burst_candidates: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(idx, e)| {
            !assigned.contains(idx)
                && e.category == FileCategory::Image
                && e.auto_tags.contains(&AutoTag::BurstPhoto)
        })
        .map(|(i, _)| i)
        .collect();

    burst_candidates.sort_by_key(|&i| (entries[i].parent_folder.clone(), entries[i].modified_unix));

    let mut current_seq: Vec<usize> = Vec::new();
    for idx in burst_candidates {
        if current_seq.is_empty() {
            current_seq.push(idx);
        } else {
            let prev_idx = *current_seq.last().unwrap();
            let prev = &entries[prev_idx];
            let curr = &entries[idx];
            if curr.parent_folder == prev.parent_folder
                && (curr.modified_unix - prev.modified_unix).abs() <= 5
            {
                current_seq.push(idx);
            } else {
                if current_seq.len() > 1 {
                    push_burst_cluster(&current_seq, entries, &mut clusters);
                }
                current_seq.clear();
                current_seq.push(idx);
            }
        }
    }
    if current_seq.len() > 1 {
        push_burst_cluster(&current_seq, entries, &mut clusters);
    }

    clusters.sort_by(|a, b| b.wasted_bytes.cmp(&a.wasted_bytes));
    clusters
}

fn push_burst_cluster(
    seq: &[usize],
    entries: &[StorageEntry],
    clusters: &mut Vec<DuplicateCluster>,
) {
    let mut sorted = seq.to_vec();
    // Keep largest (highest detail) or whitelisted photo in burst
    sorted.sort_by(|&a, &b| entries[b].size_bytes.cmp(&entries[a].size_bytes));
    let keep_idx = sorted
        .iter()
        .copied()
        .find(|&i| entries[i].whitelisted)
        .unwrap_or(sorted[0]);
    let total_bytes: u64 = sorted.iter().map(|&i| entries[i].size_bytes).sum();
    let wasted_bytes = total_bytes.saturating_sub(entries[keep_idx].size_bytes);

    clusters.push(DuplicateCluster {
        title: format!("Burst Series ({})", entries[keep_idx].name),
        reason: DuplicateReason::BurstSequence,
        entry_ids: sorted.into_iter().map(|i| entries[i].id).collect(),
        keep_id: entries[keep_idx].id,
        total_bytes,
        wasted_bytes,
    });
}

/// Returns entry IDs of all redundant duplicate copies (keeping `keep_id` in each cluster).
pub fn auto_select_redundant_ids(
    clusters: &[DuplicateCluster],
    entries: &[StorageEntry],
) -> Vec<usize> {
    let whitelisted_ids: HashSet<usize> = entries
        .iter()
        .filter(|e| e.whitelisted)
        .map(|e| e.id)
        .collect();

    let mut out = Vec::new();
    for cluster in clusters {
        for &id in &cluster.entry_ids {
            if id != cluster.keep_id && !whitelisted_ids.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}
