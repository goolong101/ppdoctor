// PPEnhancer — PinnerPi cabinet manager
//
// Rust backend exposes Tauri commands that shell out to Windows OpenSSH for
// all Pi interactions. Auth is key-based (BatchMode=yes); users push their key
// once with `ssh-copy-id` before first use.

mod db;
mod sync;
use db::*;
use sync::*;

use serde::Serialize;
use std::process::Command;

#[derive(Serialize)]
struct SshResult {
    ok: bool,
    stdout: String,
    stderr: String,
    #[serde(rename = "exitCode")]
    exit_code: i32,
}

fn ssh_target(host: &str) -> String {
    if host.contains('@') { host.to_string() } else { format!("pi@{}", host) }
}

/// Execute a single command on the Pi over SSH.
#[tauri::command]
fn ssh_run(host: String, command: String) -> Result<SshResult, String> {
    let target = ssh_target(&host);
    let output = Command::new("ssh")
        .arg("-o").arg("ConnectTimeout=10")
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg(&target)
        .arg(&command)
        .output()
        .map_err(|e| format!("failed to spawn ssh: {}", e))?;

    Ok(SshResult {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Fetch a remote file as base64. Uses `base64 < <file>` on the Pi (coreutils
/// is always installed), so we don't have to deal with binary-over-stdout.
/// Returns the base64 string, or an empty string if the file doesn't exist.
#[tauri::command]
fn ssh_get_base64(host: String, remote_path: String) -> Result<String, String> {
    let target = ssh_target(&host);
    // Single-quote-escape the path to handle spaces; reject backticks/dollars for safety.
    if remote_path.contains('`') || remote_path.contains('$') {
        return Err("path contains forbidden chars".into());
    }
    let cmd = format!("[ -f '{}' ] && base64 -w0 '{}' || true", remote_path, remote_path);
    let output = Command::new("ssh")
        .arg("-o").arg("ConnectTimeout=10")
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg(&target)
        .arg(&cmd)
        .output()
        .map_err(|e| format!("ssh: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List files (non-directories) in a remote directory. Returns one filename per line.
#[tauri::command]
fn ssh_list_dir(host: String, remote_path: String) -> Result<Vec<String>, String> {
    if remote_path.contains('`') || remote_path.contains('$') {
        return Err("path contains forbidden chars".into());
    }
    let target = ssh_target(&host);
    // -1 = one entry per line; -A = include dotfiles except . and ..; filter to files only.
    let cmd = format!(
        "[ -d '{}' ] && (cd '{}' && find . -maxdepth 1 -type f -printf '%f\\n' 2>/dev/null) || true",
        remote_path, remote_path
    );
    let output = Command::new("ssh")
        .arg("-o").arg("ConnectTimeout=10")
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg(&target)
        .arg(&cmd)
        .output()
        .map_err(|e| format!("ssh: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// SCP a remote file to a local temp file and return its contents as text.
/// Much faster than ssh_get_base64 for big files (no inflation, raw binary
/// transfer). Used for .directb2s (~5-200 MB XML).
#[tauri::command]
fn scp_get_text(host: String, remote_path: String) -> Result<String, String> {
    if remote_path.contains('`') || remote_path.contains('$') {
        return Err("path contains forbidden chars".into());
    }
    let target_path = format!("{}:{}", ssh_target(&host), remote_path);
    let tmp_dir = std::env::temp_dir();
    let tmp = tmp_dir.join(format!(
        "ppe_fetch_{}_{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
    ));

    let output = Command::new("scp")
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg("-o").arg("ConnectTimeout=10")
        .arg(&target_path)
        .arg(&tmp)
        .output()
        .map_err(|e| format!("scp spawn failed: {}", e))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "scp failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let text = std::fs::read_to_string(&tmp)
        .map_err(|e| format!("read temp file: {}", e))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(text)
}

/// Read a local file as UTF-8 text. Used for the .directb2s sources that
/// live in the user's local pinnerpi-b2s gitea clone.
#[tauri::command]
fn read_local_text(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path, e))
}

/// List directories matching a glob-style pattern (used to enumerate the
/// local b2s/NNNN_* folders).
#[tauri::command]
fn list_local_dirs(parent: String) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(&parent).map_err(|e| format!("read_dir {}: {}", parent, e))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// True if a local path exists (file or dir).
#[tauri::command]
fn local_path_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
}

/// Total size in bytes of a remote directory (recursive). Uses `du -sb`
/// so it works on any Pi shell. ~1-2 sec roundtrip on a typical cabinet.
#[tauri::command]
fn remote_dir_size(host: String, remote_path: String) -> Result<i64, String> {
    if remote_path.contains('`') || remote_path.contains('$') {
        return Err("path contains forbidden chars".into());
    }
    let target = ssh_target(&host);
    let cmd = format!("du -sb '{}' 2>/dev/null | awk '{{print $1}}'", remote_path);
    let output = Command::new("ssh")
        .arg("-o").arg("ConnectTimeout=10")
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg(&target)
        .arg(&cmd)
        .output()
        .map_err(|e| format!("ssh: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    s.parse::<i64>().map_err(|e| format!("parse '{}': {}", s, e))
}

/// Resolve the local-cache file path for a given host/folder/slot/filename.
/// Used by the frontend to check if a file is cached before falling back to SSH.
fn cache_file_path_internal(host: &str, pi_folder: &str, slot: &str, filename: &str, cache_root: Option<&str>) -> std::path::PathBuf {
    let bare_host = host.split('@').last().unwrap_or(host);
    let safe: String = bare_host.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    let base = match cache_root.filter(|s| !s.is_empty()) {
        Some(c) => std::path::PathBuf::from(c).join(safe),
        None => dirs::data_dir()
            .expect("data_dir")
            .join("PP Doctor")
            .join("media-cache")
            .join(safe),
    };
    base.join(pi_folder).join(slot).join(filename)
}

/// Read a file from the local media cache, returning base64. Returns empty
/// string if the file isn't cached yet. Used by the frontend to avoid hitting
/// SSH for files that are already locally mirrored.
///
/// NOTE: prefer `cache_get_binary` for large files (>1MB) — base64 inflates
/// the payload by 4/3 AND adds atob + char-copy decoding overhead in JS,
/// roughly 200-300ms for a 10MB cache. Binary path is direct bytes.
#[tauri::command]
fn cache_get_base64(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    cache_root: Option<String>,
) -> Result<String, String> {
    use base64::Engine;
    let path = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    if !path.exists() { return Ok(String::new()); }
    let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// Read a file from the local media cache as raw bytes. Returns empty body
/// if file doesn't exist. Uses `tauri::ipc::Response` so the bytes travel
/// the raw-IPC channel (ArrayBuffer on the JS side) instead of being
/// JSON-encoded as a number array. Measured before this change: 7-11 MB
/// `.b2scache` reads took 780-2300 ms (~9 MB/s, dominated by JSON encode
/// + parse). After: same files in 20-50 ms.
#[tauri::command]
fn cache_get_binary(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    cache_root: Option<String>,
) -> Result<tauri::ipc::Response, String> {
    let path = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    if !path.exists() { return Ok(tauri::ipc::Response::new(Vec::<u8>::new())); }
    let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Write a UTF-8 text payload into the local media cache. Used by the b2s
/// adjustments "Push to cabinet" flow to update the local copy of
/// `b2s_event_map.json` with edited attract values. Pairs with the existing
/// `db_mark_dirty` so the status-bar push will SCP the edited file to the Pi
/// in the next sync. Creates parent directories as needed. Returns the
/// number of bytes written.
#[tauri::command]
fn cache_write_text(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    content: String,
    cache_root: Option<String>,
) -> Result<usize, String> {
    let path = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let bytes = content.as_bytes();
    std::fs::write(&path, bytes).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(bytes.len())
}

/// Drop-file helpers ─────────────────────────────────────────────────────
///
/// `MAX_CACHE_VERSIONS` capped backups live alongside the primary file in a
/// `.versions/` subdir. Filename pattern: `<stem>.<unix_ms>.<ext>` newest-
/// first when sorted lexicographically (timestamp is fixed width 13 digits
/// until year 2286, fine for FIFO ordering). Replaced on every overwrite of
/// the primary; oldest pruned past MAX.
const MAX_CACHE_VERSIONS: usize = 5;

/// Back up the file at `dst` into `dst.parent()/.versions/` before overwrite.
/// No-op if `dst` doesn't exist. Returns the backup path written, if any.
fn backup_existing(dst: &std::path::Path) -> Result<Option<std::path::PathBuf>, String> {
    if !dst.exists() {
        return Ok(None);
    }
    let parent = dst.parent().ok_or_else(|| format!("no parent for {}", dst.display()))?;
    let versions = parent.join(".versions");
    std::fs::create_dir_all(&versions).map_err(|e| format!("mkdir {}: {}", versions.display(), e))?;
    let stem = dst.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext  = dst.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let backup = versions.join(format!("{}.{:013}.{}", stem, ts, ext));
    std::fs::copy(dst, &backup).map_err(|e| format!("backup {}: {}", backup.display(), e))?;
    prune_versions(&versions, stem, ext)?;
    Ok(Some(backup))
}

fn prune_versions(versions_dir: &std::path::Path, stem: &str, ext: &str) -> Result<(), String> {
    let prefix = format!("{}.", stem);
    let suffix = format!(".{}", ext);
    let mut matches: Vec<std::path::PathBuf> = std::fs::read_dir(versions_dir)
        .map_err(|e| format!("read_dir {}: {}", versions_dir.display(), e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with(&prefix) && name.ends_with(&suffix)
        })
        .collect();
    // Newest first (lexicographic on the fixed-width timestamp).
    matches.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    for stale in matches.into_iter().skip(MAX_CACHE_VERSIONS) {
        let _ = std::fs::remove_file(&stale);
    }
    Ok(())
}

/// Read an absolute path on the host as raw bytes. Used by the drop-file
/// flow to ingest dropped images before re-encoding in the frontend canvas.
#[tauri::command]
fn read_local_bytes(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| format!("read {}: {}", path, e))
}

/// Copy an absolute host path into the cache slot. Backs up any existing
/// file at the destination into `.versions/` first (≤5 retained).
#[tauri::command]
fn copy_file_to_cache(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    src_path: String,
    cache_root: Option<String>,
) -> Result<u64, String> {
    let dst = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    backup_existing(&dst)?;
    std::fs::copy(&src_path, &dst).map_err(|e| format!("copy {} -> {}: {}", src_path, dst.display(), e))
}

/// Write raw bytes to a cache file. Backs up any existing primary file
/// into `.versions/` first. Used by the drop-file flow after frontend
/// canvas resize + JPEG re-encode.
#[tauri::command]
fn cache_write_binary(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    bytes: Vec<u8>,
    cache_root: Option<String>,
) -> Result<usize, String> {
    let path = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    backup_existing(&path)?;
    let n = bytes.len();
    std::fs::write(&path, &bytes).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(n)
}

/// List `.versions/` entries for a given primary file, newest first.
/// Each entry: { filename, full_path, mtime_ms, size }.
#[tauri::command]
fn list_cache_versions(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    cache_root: Option<String>,
) -> Result<Vec<serde_json::Value>, String> {
    let primary = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    let parent = match primary.parent() { Some(p) => p, None => return Ok(vec![]) };
    let versions = parent.join(".versions");
    if !versions.exists() { return Ok(vec![]); }
    let stem = primary.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext  = primary.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    let prefix = format!("{}.", stem);
    let suffix = format!(".{}", ext);
    let mut entries: Vec<(String, std::path::PathBuf, u64, u64)> = std::fs::read_dir(&versions)
        .map_err(|e| format!("read_dir {}: {}", versions.display(), e))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?.to_string();
            if !name.starts_with(&prefix) || !name.ends_with(&suffix) { return None; }
            let meta = e.metadata().ok()?;
            let size = meta.len();
            let mtime = meta.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            Some((name, p, mtime, size))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(entries.into_iter().map(|(name, path, mtime, size)| {
        serde_json::json!({
            "filename": name,
            "full_path": path.to_string_lossy(),
            "mtime_ms": mtime,
            "size": size,
        })
    }).collect())
}

/// Delete a single media file from both the local mirror AND the Pi. Also
/// wipes any `.versions/` backups for that file's stem.ext pattern. Paired
/// thumb (e.g. <name>.thumb.jpg next to a video) is removed when the user
/// asks to. Used by the per-row "Delete" button in the Files list.
///
/// Returns the list of paths removed (with "local:"/"remote:" prefix), same
/// format as reset_to_b2s_default for status display.
#[tauri::command]
fn delete_cache_file(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    cache_root: Option<String>,
    also_delete_thumb: Option<bool>,
) -> Result<Vec<String>, String> {
    let mut removed: Vec<String> = vec![];
    let primary = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    let parent = match primary.parent() { Some(p) => p.to_path_buf(), None => return Ok(removed) };

    // 1) Local primary
    if primary.exists() {
        if std::fs::remove_file(&primary).is_ok() {
            removed.push(format!("local:{}", primary.display()));
        }
    }
    // 2) Local .versions/<stem>.<ts>.<ext> entries matching this file
    let stem = primary.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext  = primary.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    let versions = parent.join(".versions");
    if versions.exists() {
        let prefix = format!("{}.", stem);
        let suffix = format!(".{}", ext);
        if let Ok(rd) = std::fs::read_dir(&versions) {
            for entry in rd.flatten() {
                let p = entry.path();
                let n = match p.file_name().and_then(|n| n.to_str()) { Some(s) => s.to_string(), None => continue };
                if n.starts_with(&prefix) && n.ends_with(&suffix) {
                    if std::fs::remove_file(&p).is_ok() {
                        removed.push(format!("local:{}", p.display()));
                    }
                }
            }
        }
        // Drop the .versions dir entirely if empty after pruning
        if let Ok(rd) = std::fs::read_dir(&versions) {
            if rd.count() == 0 { let _ = std::fs::remove_dir(&versions); }
        }
    }
    // 3) Companion thumb if asked (e.g. <stem>.thumb.jpg for a video)
    if also_delete_thumb.unwrap_or(false) {
        let thumb = parent.join(format!("{}.thumb.jpg", stem));
        if thumb.exists() {
            if std::fs::remove_file(&thumb).is_ok() {
                removed.push(format!("local:{}", thumb.display()));
            }
        }
    }
    // 4) Pi side — SSH delete of the same files
    let safe_folder = pi_folder.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    let safe_slot   = slot.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let safe_name   = filename.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !safe_folder || !safe_slot || !safe_name {
        return Err(format!("unsafe filename / folder: {}/{}/{}", pi_folder, slot, filename));
    }
    let remote_primary = format!("/home/pi/PinnerPi/media/{}/{}/{}", pi_folder, slot, filename);
    let remote_thumb   = format!("/home/pi/PinnerPi/media/{}/{}/{}.thumb.jpg", pi_folder, slot, stem);
    let rm_cmd = if also_delete_thumb.unwrap_or(false) {
        format!("for f in '{}' '{}'; do [ -f \"$f\" ] && rm -fv \"$f\"; done", remote_primary, remote_thumb)
    } else {
        format!("for f in '{}'; do [ -f \"$f\" ] && rm -fv \"$f\"; done", remote_primary)
    };
    let ssh_target = ssh_target(&host);
    let out = std::process::Command::new("ssh")
        .args(["-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=no"])
        .arg(&ssh_target)
        .arg(&rm_cmd)
        .output()
        .map_err(|e| format!("ssh spawn: {}", e))?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let t = line.trim();
        if !t.is_empty() { removed.push(format!("remote:{}", t)); }
    }
    Ok(removed)
}

/// Delete every backup in `.versions/` matching a primary file's stem.ext
/// pattern. Returns the count of files removed. The primary file itself
/// is NOT touched. Used by the "Delete old versions" UI button after
/// user confirmation.
#[tauri::command]
fn delete_cache_versions(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    cache_root: Option<String>,
) -> Result<usize, String> {
    let primary = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    let parent = primary.parent().ok_or_else(|| format!("no parent for {}", primary.display()))?;
    let versions = parent.join(".versions");
    if !versions.exists() { return Ok(0); }
    let stem = primary.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext  = primary.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    let prefix = format!("{}.", stem);
    let suffix = format!(".{}", ext);
    let mut count = 0usize;
    if let Ok(rd) = std::fs::read_dir(&versions) {
        for entry in rd.flatten() {
            let p = entry.path();
            let name = match p.file_name().and_then(|n| n.to_str()) { Some(n) => n, None => continue };
            if name.starts_with(&prefix) && name.ends_with(&suffix) {
                if std::fs::remove_file(&p).is_ok() { count += 1; }
            }
        }
    }
    // If we just emptied the directory, drop it too.
    if let Ok(rd) = std::fs::read_dir(&versions) {
        if rd.count() == 0 { let _ = std::fs::remove_dir(&versions); }
    }
    Ok(count)
}

/// Restore a previously-backed-up version (from `.versions/`) to the
/// primary slot. The currently-primary file is itself backed up first.
#[tauri::command]
fn restore_cache_version(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    version_filename: String,
    cache_root: Option<String>,
) -> Result<u64, String> {
    let primary = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    let parent = primary.parent().ok_or_else(|| format!("no parent for {}", primary.display()))?;
    let src = parent.join(".versions").join(&version_filename);
    if !src.exists() { return Err(format!("version not found: {}", src.display())); }
    backup_existing(&primary)?;
    std::fs::copy(&src, &primary).map_err(|e| format!("restore {} -> {}: {}", src.display(), primary.display(), e))
}

/// ffmpeg transcode for dropped videos ────────────────────────────────────
///
/// Produces a Pi-Zero-friendly H.264 mp4 tuned for VC4's hw decoder:
///   • 1080p30 (scaled to fit 1920×1080, aspect preserved, never upscaled)
///   • H.264 Baseline @ L4.0  (no B-frames, no CABAC — VC4's v4l2m2m
///     decoder backpressures on reordering streams)
///   • preset slow, crf 19, maxrate 3M  (quality/memory balance for 512 MB Pi Zero 2W)
///   • yuv420p, +faststart, GOP = 30 (1s keyframes for snappy loop seek)
///   • AAC 128 kbps stereo
/// See the documented ffmpeg command at the bottom of this function for
/// the exact equivalent invocation users can run by hand.
///
/// Binary resolution order (so we can ship ffmpeg alongside PP Doctor
/// without depending on the host's PATH):
///   1. `<install_dir>/ffmpeg.exe`   (next to pp-doctor.exe — bundled)
///   2. `<install_dir>/binaries/ffmpeg.exe`  (sidecar convention)
///   3. `<APPDATA>/PPDoctor/ffmpeg.exe`   (user-installed)
///   4. system PATH (`ffmpeg`)
/// Same for ffprobe.

fn ffmpeg_search_dirs() -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            out.push(parent.to_path_buf());
            out.push(parent.join("binaries"));
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        out.push(std::path::PathBuf::from(appdata).join("PPDoctor"));
    }
    out
}

fn resolve_tool(name: &str) -> std::path::PathBuf {
    let exe_name = if cfg!(windows) { format!("{}.exe", name) } else { name.to_string() };
    for dir in ffmpeg_search_dirs() {
        let cand = dir.join(&exe_name);
        if cand.is_file() { return cand; }
    }
    // Fall back to bare name — std::process::Command will resolve via PATH.
    std::path::PathBuf::from(name)
}

/// Where we actually found ffmpeg, for diagnostics.
#[tauri::command]
fn ffmpeg_path() -> String {
    resolve_tool("ffmpeg").to_string_lossy().to_string()
}

/// True iff `ffmpeg` (bundled or PATH) is callable.
#[tauri::command]
fn ffmpeg_available() -> bool {
    let probe = std::process::Command::new(resolve_tool("ffmpeg"))
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    probe.map(|s| s.success()).unwrap_or(false)
}

fn detect_source_fps(src: &str) -> Option<f64> {
    let out = std::process::Command::new(resolve_tool("ffprobe"))
        .args([
            "-v", "0",
            "-select_streams", "v:0",
            "-of", "csv=p=0",
            "-show_entries", "stream=r_frame_rate",
            src,
        ])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    // r_frame_rate is "num/den" — parse and divide.
    let (num, den) = s.split_once('/')?;
    let n: f64 = num.trim().parse().ok()?;
    let d: f64 = den.trim().parse().ok()?;
    if d <= 0.0 { return None; }
    Some(n / d)
}

#[tauri::command]
fn transcode_video_to_cache(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    src_path: String,
    cache_root: Option<String>,
) -> Result<u64, String> {
    if !ffmpeg_available() {
        return Err(format!(
            "ffmpeg not found — place ffmpeg.exe in {} (next to pp-doctor.exe) or install on PATH",
            ffmpeg_search_dirs().first().map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }
    let dst = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    backup_existing(&dst)?;

    // Always target 30 fps CFR for Pi V4L2 M2M consistency. Low-fps sources
    // (24, 23.976, 25) get motion-blend interpolation to fill in tween
    // frames — eliminates the judder you get from naive frame duplication.
    // High-fps sources (60, 50) get downsampled. mi_mode=blend is the
    // pragmatic choice: faster than mci (motion-compensated, slow + can
    // ghost), better than dup (visible judder). For mostly-static
    // backglass content, blend produces clean tween frames.
    let src_fps = detect_source_fps(&src_path).unwrap_or(30.0);
    let target_fps: u32 = 30;
    let gop = target_fps.to_string();
    let vf_interpolate: String = if src_fps < 29.5 {
        format!("minterpolate=fps={}:mi_mode=blend,", target_fps)
    } else {
        // Source already ≥30 fps — drop frames cleanly via fps filter.
        format!("fps={},", target_fps)
    };
    // ALWAYS output 1920×1080. Earlier this filter used
    //   scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease
    // which kept sub-1080p sources at their native size (e.g., Zen exports at
    // 1280×720 stayed 720p). On the Pi Zero renderer that creates a
    // resolution mismatch between videos: each video→video crossfade triggers
    // an SDL_DestroyTexture+CreateTexture on the incoming layer to match the
    // new video's dimensions. On the VC4 GPU, that mid-crossfade destroy+
    // create thrashes texture-pool state and renders the OLD outgoing video
    // as 3 horizontal tiles for ~150-500 ms — the longer the alloc takes
    // (1080p is ~4× a 720p alloc) the longer the corruption window. Standardizing
    // the whole library at 1920×1080 means both video layers settle at one
    // size on first play and never recreate again — clean fades everywhere.
    //
    // Always fill 1920×1080. 16:9 sources scale cleanly; non-16:9 get
    // stretched (no black bars) — the cabinet backglass is fixed 16:9 and
    // black bars waste screen real estate, so the user prefers stretch over
    // letterbox/pillarbox. Lanczos for quality on upscales.
    let vf_chain = format!(
        "{}scale=1920:1080:flags=lanczos",
        vf_interpolate
    );

    // Encode into a temp sibling, then atomic-rename. ffmpeg refuses to
    // overwrite an in-place file even with -y on some platforms.
    let tmp = dst.with_extension(format!("transcode-{}.mp4",
        std::process::id()));

    // Pi Zero 2W's h264_v4l2m2m hw decoder stutters on frame-reordering
    // streams. Strip B-frames, single reference, fixed GOP, modest rate
    // cap so the V4L2 M2M decode pipeline doesn't backpressure.
    //
    // CRF 19 + preset slow + maxrate 3M target ≈ 1.5-2 Mbps average. The
    // ceiling is RAM, not decode bandwidth: Pi Zero 2W has ~354 MB usable
    // (after GPU split) and the demuxer/decoder ring buffers grow with
    // bitrate. Empirically a 5 Mbps file at 90s loop put the system into
    // active swap paging (113 MB swap-in), which feels like stutter even
    // though no clock was throttled. 3M maxrate / shorter loops keep the
    // working set in RAM. Increase only after confirming free swap stays
    // ≈ untouched during sustained playback.
    // Thumb filename mirrors the video filename: backglass.mp4 →
    // backglass.thumb.jpg, sibling in default_video/. Pi renderer matches
    // 960×540 (THUMB_W×THUMB_H from renderer.cpp:46-47); anything else is
    // auto-deleted by preloadThumbs as a stale-dimension thumb.
    let stem = dst.file_stem().and_then(|s| s.to_str()).unwrap_or("backglass");
    let dst_thumb = dst.with_file_name(format!("{}.thumb.jpg", stem));
    let tmp_thumb = dst.with_extension(format!("thumb-{}.jpg", std::process::id()));
    backup_existing(&dst_thumb)?;

    // Single ffmpeg invocation emits BOTH the transcoded mp4 AND a 960×540
    // first-frame thumb. Per-output -vf scales independently. Saves a
    // second ffmpeg spawn and reuses the demux pass.
    //
    // Pi Zero 2W's h264_v4l2m2m hw decoder stutters on B-frame reordering,
    // so strip B-frames and use single-ref. zerolatency tune was a bad
    // choice — it disables AQ / mbtree / lookahead which left the first
    // GOP visibly pixelated, especially at the loop boundary. We just want
    // a B-frame-free Main-profile stream with normal rate-control behavior.
    let status = std::process::Command::new(resolve_tool("ffmpeg"))
        .args([
            "-hide_banner", "-loglevel", "error", "-y",
            "-i", &src_path,
            // ── Output 1: transcoded video ────────────────────────────
            "-map", "0:v:0", "-map", "0:a:0?",
            "-vf", &vf_chain,
            "-r", &target_fps.to_string(),
            "-fps_mode", "cfr",          // constant frame rate (no vfr drift on Pi V4L2 timing)
            "-vsync", "cfr",             // legacy alias — harmless if redundant
            "-c:v", "libx264",
            "-preset", "slow",
            // Baseline profile — no CABAC, no B-frames, no 8x8 transforms.
            // Pi Zero 2W's bcm2835-codec V4L2 M2M decodes this fastest;
            // Main profile's CABAC entropy coding adds measurable CPU even
            // when the GPU does the rest of the heavy lifting. The size
            // cost (~10-15% larger than Main at same crf) is irrelevant
            // for short backglass loops, but the smoothness win is real.
            "-profile:v", "baseline",
            "-level", "4.0",
            "-coder", "0",               // belt-and-braces: force CAVLC even if profile changes
            "-pix_fmt", "yuv420p",
            "-bf", "0", "-refs", "1",
            "-sc_threshold", "0",
            "-g", &gop, "-keyint_min", &gop,
            "-crf", "19",
            "-maxrate", "3M", "-bufsize", "6M",
            "-movflags", "+faststart",
            "-c:a", "aac", "-b:a", "128k",
            "-ac", "2",
        ])
        .arg(&tmp)
        .args([
            // ── Output 2: 960×540 thumb (single frame) ─────────────────
            "-map", "0:v:0",
            "-vf", "scale=960:540:force_original_aspect_ratio=decrease,pad=960:540:(960-iw)/2:(540-ih)/2:color=black",
            "-frames:v", "1",
            "-q:v", "4",          // JPEG quality ~85, matches Pi's THUMB_JPEG_QUALITY=70 ballpark
            "-pix_fmt", "yuvj420p",
            "-f", "image2",
        ])
        .arg(&tmp_thumb)
        .status()
        .map_err(|e| format!("spawn ffmpeg: {}", e))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&tmp_thumb);
        return Err(format!("ffmpeg failed (exit {:?})", status.code()));
    }
    std::fs::rename(&tmp, &dst).map_err(|e| format!("rename {} -> {}: {}", tmp.display(), dst.display(), e))?;
    // Best-effort thumb rename — failure here doesn't fail the transcode.
    if let Err(e) = std::fs::rename(&tmp_thumb, &dst_thumb) {
        eprintln!("[transcode] thumb rename failed: {}", e);
        let _ = std::fs::remove_file(&tmp_thumb);
    }
    let size = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    Ok(size)
}

/// Reset-to-b2s-default ──────────────────────────────────────────────────
///
/// Deletes dropped user media from BOTH the local cache mirror AND the Pi,
/// restoring the table to its as-shipped b2s-only state. Preserves:
///   - backglass.b2scache       (the b2s render cache — the actual source)
///   - b2s_event_map.json       (lamp/attract authoring)
///   - backglass.b2s_base.thumb.jpg + similar thumb.* (b2s-derived previews)
///   - .glow / glow_config.json (image-glow auto-detect output for non-b2s)
///   - any backglass.directb2s  (XML source, rare in local mirror)
///   - any folder we don't recognize (sibling event folders, etc.)
///
/// Deletes (user-droppable media in default_image / default_video):
///   - backglass.{jpg,jpeg,png,webp,gif,bgra,bmp} in default_image
///   - backglass.{mp4,webm,mkv,mov,m4v}            in default_video
///   - the entire .versions/ backup tree in both slots
///
/// Returns the list of file paths removed (local + remote, with prefix
/// "local:" or "remote:") for confirmation.

fn is_droppable_media_filename(filename: &str, slot: &str) -> bool {
    let lower = filename.to_lowercase();
    let ext = lower.split('.').last().unwrap_or("");
    match slot {
        // In default_image/ only the b2s-derived thumb is preserved by name
        // ("backglass.b2s_base.thumb.jpg" — caught by the early return below).
        // Everything else that's a viewable image extension is treated as a
        // user drop and deleted on reset.
        "default_image" => {
            // Preserve b2s-derived thumb files (any *.b2s_base.thumb.jpg or
            // similar). Drop only "raw" backglass image drops.
            if lower.ends_with(".b2s_base.thumb.jpg") { return false; }
            if lower == "backglass.b2s_base.thumb.jpg" { return false; }
            matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif" | "bgra" | "bmp")
        }
        // default_video/ is entirely user-drop territory — preserve nothing
        // by default. Matches any common video container + any .thumb.jpg
        // sibling generated by transcode_video_to_cache.
        "default_video" => {
            matches!(ext, "mp4" | "webm" | "mkv" | "mov" | "m4v")
                || lower.ends_with(".thumb.jpg")
        }
        _ => false,
    }
}

#[tauri::command]
fn reset_to_b2s_default(
    host: String,
    pi_folder: String,
    cache_root: Option<String>,
) -> Result<Vec<String>, String> {
    let mut removed: Vec<String> = vec![];
    // 1. Local mirror: walk default_image and default_video, delete primary
    //    droppable media + the entire .versions/ subfolder in each.
    for slot in &["default_image", "default_video"] {
        // Resolve a path inside the slot to locate the slot dir, then walk.
        let probe = cache_file_path_internal(&host, &pi_folder, slot, "_", cache_root.as_deref());
        let slot_dir = match probe.parent() { Some(p) => p.to_path_buf(), None => continue };
        if !slot_dir.exists() { continue; }
        // Primary files
        if let Ok(rd) = std::fs::read_dir(&slot_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if !p.is_file() { continue; }
                let name = match p.file_name().and_then(|n| n.to_str()) { Some(n) => n, None => continue };
                if is_droppable_media_filename(name, slot) {
                    if std::fs::remove_file(&p).is_ok() {
                        removed.push(format!("local:{}", p.display()));
                    }
                }
            }
        }
        // Backup tree
        let versions = slot_dir.join(".versions");
        if versions.exists() {
            if std::fs::remove_dir_all(&versions).is_ok() {
                removed.push(format!("local:{}", versions.display()));
            }
        }
    }
    // 2. Pi: delete the same patterns under /home/pi/PinnerPi/media/<folder>/
    //    A single ssh round-trip with rm -fv (verbose so we see what was
    //    removed). Note: this is the Pi Zero, no globbing surprises since
    //    we're matching exact 'backglass.<ext>' filenames.
    let safe_folder = pi_folder
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !safe_folder {
        return Err(format!("unsafe pi_folder: {}", pi_folder));
    }
    let remote = format!("/home/pi/PinnerPi/media/{}", pi_folder);
    // Globs (shell side) — `nullglob` so missing matches expand to nothing.
    // default_video/ is entirely user content (default state = empty), so
    // we wipe every video extension + every .thumb.jpg in it. default_image/
    // is more selective: only drop bare backglass.<imgext>, never touch
    // backglass.b2s_base.thumb.jpg, backglass.b2scache, or
    // b2s_event_map.json (those are b2s assets to preserve).
    let rm_cmd = format!(
        "shopt -s nullglob; \
         for f in {0}/default_image/backglass.jpg {0}/default_image/backglass.jpeg \
                  {0}/default_image/backglass.png {0}/default_image/backglass.webp \
                  {0}/default_image/backglass.gif {0}/default_image/backglass.bgra \
                  {0}/default_image/backglass.bmp \
                  {0}/default_video/*.mp4  {0}/default_video/*.webm \
                  {0}/default_video/*.mkv  {0}/default_video/*.mov \
                  {0}/default_video/*.m4v  {0}/default_video/*.thumb.jpg; \
           do [ -f \"$f\" ] && rm -fv \"$f\"; done",
        remote
    );
    let ssh_target = ssh_target(&host);
    let out = std::process::Command::new("ssh")
        .args(["-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=no"])
        .arg(&ssh_target)
        .arg(&rm_cmd)
        .output()
        .map_err(|e| format!("ssh spawn: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() { removed.push(format!("remote:{}", trimmed)); }
    }
    Ok(removed)
}

/// Generate-B2S-from-video helpers ───────────────────────────────────────
///
/// Pipeline: PP Doctor extracts a max-brightness composite from the video
/// (canvas-side), writes it to a temp PNG, then this command shells out
/// to tools/scaffold_b2s_from_png.py which:
///   1. Thresholds bright pixels
///   2. Connected-component labels into blobs
///   3. Drops noise (min_area) and full-DMD overlays (max_area_frac)
///   4. Emits one <Bulb> per kept blob with bbox + masked lit sprite
///   5. Wraps in <DirectB2SData> XML and writes .directb2s
///
/// Output is a scaffold the user can review/edit before pushing to Pi.

/// Resolve the Python interpreter. Prefers an explicit env var, then
/// "python", then "python3".
fn resolve_python() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("PPDOCTOR_PYTHON") {
        return std::path::PathBuf::from(p);
    }
    // Try `python` first (Windows installer convention)
    for name in &["python", "python3"] {
        let exe_name = if cfg!(windows) { format!("{}.exe", name) } else { name.to_string() };
        if let Ok(out) = std::process::Command::new(&exe_name).arg("--version").output() {
            if out.status.success() { return std::path::PathBuf::from(name); }
        }
    }
    std::path::PathBuf::from("python")
}

/// Write a Vec<u8> to a temp file under %TEMP%/ppdoctor/, return the path.
/// Used by the b2s-from-video pipeline to stash the max-brightness PNG
/// before invoking the scaffold script.
#[tauri::command]
fn write_temp_bytes(filename: String, bytes: Vec<u8>) -> Result<String, String> {
    let tmp_root = std::env::temp_dir().join("ppdoctor");
    std::fs::create_dir_all(&tmp_root).map_err(|e| format!("mkdir {}: {}", tmp_root.display(), e))?;
    // Sanitize: only allow ASCII alnum + . _ -
    let safe: String = filename.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-').collect();
    if safe.is_empty() { return Err("empty filename after sanitization".into()); }
    let path = tmp_root.join(&safe);
    std::fs::write(&path, &bytes).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(path.to_string_lossy().to_string())
}

/// Run tools/scaffold_b2s_from_png.py against the given input PNG and
/// emit a .directb2s scaffold. Returns the scaffold's absolute path on
/// success, or an error message describing why it failed (no Python,
/// missing PIL/numpy, script error, etc.).
#[tauri::command]
fn scaffold_b2s_from_png(
    png_path: String,
    output_directb2s: String,
    b2s_repo_root: String,
    threshold: Option<u8>,
    min_area: Option<u32>,
    max_area_frac: Option<f32>,
    base_dim: Option<f32>,
) -> Result<String, String> {
    let script = std::path::Path::new(&b2s_repo_root).join("tools").join("scaffold_b2s_from_png.py");
    if !script.exists() {
        return Err(format!("scaffold script not found: {}", script.display()));
    }
    let py = resolve_python();
    let mut cmd = std::process::Command::new(&py);
    cmd.arg(&script).arg(&png_path).arg(&output_directb2s);
    if let Some(t) = threshold     { cmd.arg("--threshold").arg(t.to_string()); }
    if let Some(a) = min_area      { cmd.arg("--min-area").arg(a.to_string()); }
    if let Some(f) = max_area_frac { cmd.arg("--max-area-frac").arg(format!("{:.3}", f)); }
    if let Some(d) = base_dim      { cmd.arg("--base-dim").arg(format!("{:.3}", d)); }
    let out = cmd.output().map_err(|e| format!("spawn python: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "scaffold script failed (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if !std::path::Path::new(&output_directb2s).exists() {
        return Err(format!("scaffold did not produce output: {}", output_directb2s));
    }
    Ok(output_directb2s)
}

/// Same path resolver but exposed as a command so the frontend can list
/// what's in the cache.
#[tauri::command]
fn cache_file_path(
    host: String,
    pi_folder: String,
    slot: String,
    filename: String,
    cache_root: Option<String>,
) -> Result<String, String> {
    let p = cache_file_path_internal(&host, &pi_folder, &slot, &filename, cache_root.as_deref());
    Ok(p.to_string_lossy().to_string())
}

/// Total bytes used by PP Doctor's local media cache for this host.
/// Returns 0 if the cache dir doesn't exist yet.
#[tauri::command]
fn local_cache_size(host: String) -> Result<i64, String> {
    let base = dirs::data_dir().ok_or("no APPDATA")?;
    let safe: String = host.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    let dir = base.join("PP Doctor").join("media-cache").join(safe);
    if !dir.exists() { return Ok(0); }
    fn walk(p: &std::path::Path) -> i64 {
        let mut sum = 0i64;
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                if let Ok(ft) = e.file_type() {
                    if ft.is_file() {
                        sum += e.metadata().map(|m| m.len() as i64).unwrap_or(0);
                    } else if ft.is_dir() {
                        sum += walk(&e.path());
                    }
                }
            }
        }
        sum
    }
    Ok(walk(&dir))
}

/// Capture a screenshot of the primary monitor via PowerShell + .NET. Saves
/// to the given path. The webview doesn't have a native screenshot API; this
/// shell-out avoids adding a Rust crate dependency.
#[tauri::command]
fn take_screenshot(path: String) -> Result<String, String> {
    let script = format!(r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
$bmp.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
"#, path.replace('\\', "/").replace('\'', "''"));
    let output = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command").arg(&script)
        .output()
        .map_err(|e| format!("powershell spawn: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(path)
}

/// Write arbitrary JSON state to a file. Used by the frontend's debug snapshot.
#[tauri::command]
fn write_state_dump(path: String, content: String) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// Append a line to C:/tmp/ppenhancer.log. Used for cross-process debugging
/// — tail this file from an external terminal to see what the app is doing.
#[tauri::command]
fn log_line(text: String) -> Result<(), String> {
    use std::io::Write;
    let path = std::path::Path::new("C:/tmp/ppenhancer.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "[{}] {}", ms, text).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DbState::new())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use tauri::{
                Manager, Emitter,
                menu::{Menu, MenuItem, PredefinedMenuItem},
                tray::{TrayIconBuilder, MouseButton, MouseButtonState, TrayIconEvent},
            };

            // Tray right-click menu: Show window / Sync now / Quit.
            let show = MenuItem::with_id(app, "show",  "Open PP Doctor", true, None::<&str>)?;
            let sync = MenuItem::with_id(app, "sync",  "Sync to cabinet", true, None::<&str>)?;
            let snap = MenuItem::with_id(app, "snap",  "Snapshot for AI debug", true, None::<&str>)?;
            let sep  = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit",  "Quit",            true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &sync, &snap, &sep, &quit])?;

            let _tray = TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("PP Doctor — PPEnhancer cabinet manager")
                .menu(&menu)
                .show_menu_on_left_click(false) // left-click = focus window, right-click = menu
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => { app.exit(0); }
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.unminimize();
                            let _ = win.set_focus();
                        }
                    }
                    "sync" => {
                        // Frontend listens for this event and triggers the sync flow.
                        let _ = app.emit("tray:sync-requested", ());
                    }
                    "snap" => {
                        // Frontend writes state dump + Rust takes screenshot.
                        let _ = app.emit("tray:snapshot-requested", ());
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up, ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.unminimize();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ssh_run, ssh_get_base64, ssh_list_dir, scp_get_text,
            read_local_text, list_local_dirs, local_path_exists, log_line,
            remote_dir_size, local_cache_size,
            cache_get_base64, cache_get_binary, cache_write_text, cache_file_path,
            read_local_bytes, copy_file_to_cache, cache_write_binary,
            list_cache_versions, restore_cache_version, delete_cache_versions,
            delete_cache_file,
            transcode_video_to_cache, ffmpeg_available, ffmpeg_path,
            reset_to_b2s_default,
            write_temp_bytes, scaffold_b2s_from_png,
            take_screenshot, write_state_dump,
            db_open, db_upsert_tables, db_get_tables,
            db_replace_media, db_get_media, db_get_all_media,
            db_dirty_count, db_dirty_files,
            db_mark_dirty, db_clear_dirty,
            db_create_snapshot, db_list_snapshots, db_delete_snapshot,
            db_upsert_updates, db_available_updates_count,
            db_get_setting, db_set_setting,
            db_audit_essentials,
            sync_push_dirty, sync_pull_table, sync_pull_all, sync_prune_non_essential
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
