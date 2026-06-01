// PP Doctor — sync engine.
//
// Two directions:
//   sync_push_dirty   — local edits (dirty=1 rows) → Pi via SCP, clears the flag
//   sync_pull_table   — Pi → local cache (single table — used for lazy mirror)
//
// Both emit "sync:progress" Tauri events the frontend listens for to drive
// the status-bar progress bar.

use rusqlite::params;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x00004000;

/// Spawn a Command with reduced OS priority so SCP storms don't starve the
/// foreground UI process. No-op on non-Windows (Tauri targets Windows here).
fn low_prio_cmd(bin: &str) -> Command {
    let mut c = Command::new(bin);
    #[cfg(windows)]
    {
        c.creation_flags(BELOW_NORMAL_PRIORITY_CLASS);
    }
    c
}

use crate::db::DbState;

/// Process-wide guard against concurrent sync runs. If a frontend race causes
/// `sync_pull_all` to be invoked multiple times in parallel, the duplicates
/// short-circuit with `Err("sync already in progress")`.
static SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

struct SyncGuard;
impl Drop for SyncGuard {
    fn drop(&mut self) { SYNC_RUNNING.store(false, Ordering::SeqCst); }
}
fn try_claim_sync() -> Result<SyncGuard, String> {
    if SYNC_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("sync already in progress".into());
    }
    Ok(SyncGuard)
}

#[derive(Serialize, Clone)]
pub struct SyncProgress {
    pub phase: String,                // "push" | "pull"
    pub current: usize,
    pub total: usize,
    pub file: String,
    pub status: String,               // "transferring" | "synced" | "done" | "error"
    pub error: Option<String>,
    pub table_id: Option<i64>,        // present on per-file events
    pub slot: Option<String>,
}

/// Whitelist of files PP Doctor actually needs from the Pi. The other files
/// in default_image/ (especially `backglass.directb2s` at ~18-30 MB each ×
/// 233 tables = ~4-7 GB) are priority-3 fallback only — the .b2scache has
/// the same content in a smaller, ready-to-render form. Filtering at the
/// sync layer keeps the local mirror small (~2 GB instead of 6 GB).
///
/// Note: this filter applies to BOTH sync_pull_all and sync_pull_table so
/// "Pull this table" stays consistent with the full mirror.
fn is_essential_file(filename: &str) -> bool {
    // Backglass cache (PP Doctor primary source)
    if filename == "backglass.b2scache" { return true; }
    if filename == "backglass.b2s_base.thumb.jpg" { return true; }
    // Event map
    if filename == "b2s_event_map.json" { return true; }
    // Glow cache (small, derived locally if missing — but keep if present)
    if filename.ends_with(".glow") { return true; }
    // Skip everything else: backglass.directb2s (heavy XML, fallback only),
    // any default_video/* (PP Doctor doesn't preview video right now),
    // legacy artifacts.
    false
}

fn safe_host(host: &str) -> String {
    host.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect()
}

fn cache_root(host: &str, custom: Option<&str>) -> PathBuf {
    let bare_host = host.split('@').last().unwrap_or(host);
    match custom.filter(|s| !s.is_empty()) {
        Some(c) => PathBuf::from(c).join(safe_host(bare_host)),
        None => {
            let base = dirs::data_dir().expect("data_dir");
            base.join("PP Doctor").join("media-cache").join(safe_host(bare_host))
        }
    }
}

fn ssh_target(host: &str) -> String {
    if host.contains('@') { host.to_string() } else { format!("pi@{}", host) }
}

/// Path to the SSH ControlMaster socket for a host — a single persistent
/// connection that all subsequent scp/ssh calls reuse. Eliminates the
/// per-file handshake overhead (~500ms-1s per call) that dominates small-file
/// transfers over slow Wi-Fi.
fn control_path(host: &str) -> std::path::PathBuf {
    let bare = host.split('@').last().unwrap_or(host);
    let safe: String = bare.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .collect();
    std::env::temp_dir().join(format!("ppd_ssh_{}.sock", safe))
}

/// Common SSH options for every scp/ssh call inside the sync loop.
///
/// ControlMaster was originally enabled here to share one persistent connection
/// across all scp calls (eliminating ~500ms-1s handshake per file). It's been
/// dropped because Windows OpenSSH scp (C:\Windows\System32\OpenSSH\scp.exe,
/// the binary Tauri's Command::new("scp") resolves to via CreateProcess PATH
/// search) doesn't support ControlMaster — every call fails with
/// `getsockname failed: Not a socket` and exit 255. We discovered this on
/// 2026-05-25 after the mirror appeared to "succeed" 695× per run while
/// writing zero files (only the gitea-merge path was producing cache files).
///
/// Trade-off: each scp now pays a fresh handshake (~500ms-1s) but at least
/// transfers actually complete. For a fresh-cache full mirror of ~700 files
/// that adds ~5-10 minutes of wall time compared to a working ControlMaster.
fn cm_args(host: &str) -> Vec<String> {
    let _ = host;
    vec![
        "-o".into(), "BatchMode=yes".into(),
        "-o".into(), "StrictHostKeyChecking=accept-new".into(),
        "-o".into(), "ConnectTimeout=10".into(),
    ]
}

fn emit(app: &tauri::AppHandle, p: SyncProgress) {
    let _ = app.emit("sync:progress", p);
}

/// Rate-limited emit for the high-frequency "synced" path. Sync of ~700 files
/// blasts ~hundreds of events/sec through the Tauri IPC channel; even with
/// rAF throttling on the frontend it leaves the UI thread unresponsive
/// (verified 2026-05-25 — whole app froze during a fresh full-cabinet pull).
/// We coalesce non-terminal status emits to at most one every EMIT_MIN_MS,
/// but always emit terminal/critical statuses (transferring start, error,
/// done, the FINAL synced of each transfer) unfiltered so the user sees the
/// progress bar move and errors land immediately.
const EMIT_MIN_MS: u128 = 50;
fn emit_throttled(
    app: &tauri::AppHandle,
    last_emit: &mut std::time::Instant,
    p: SyncProgress,
    force: bool,
) {
    let elapsed = last_emit.elapsed().as_millis();
    let critical = force
        || p.status == "error"
        || p.status == "done"
        || p.status == "transferring";
    if critical || elapsed >= EMIT_MIN_MS {
        *last_emit = std::time::Instant::now();
        let _ = app.emit("sync:progress", p);
    }
}

/// Push every dirty file in the DB to the Pi via SCP. Clears the dirty flag
/// on each file as it succeeds. Returns the number of files successfully pushed.
#[tauri::command]
pub fn sync_push_dirty(
    app: tauri::AppHandle,
    host: String,
    cache_root: Option<String>,
    state: tauri::State<'_, DbState>,
) -> Result<usize, String> {
    let _guard = try_claim_sync()?;   // refuses duplicate concurrent calls

    // 1) Snapshot dirty rows + their pi_folder. Drop the lock before SCP-ing.
    let rows: Vec<(i64, String, String, Option<String>)> = {
        let g = state.conn.lock().unwrap();
        let conn = g.as_ref().ok_or("db not open")?;
        let mut stmt = conn.prepare(
            "SELECT mf.table_id, mf.slot, mf.filename, t.pi_folder \
             FROM media_files mf \
             JOIN tables t ON mf.table_id = t.id \
             WHERE mf.dirty = 1 \
             ORDER BY mf.table_id, mf.slot, mf.filename"
        ).map_err(|e| e.to_string())?;
        let mapped = stmt.query_map([], |r| Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))).map_err(|e| e.to_string())?;
        let collected: Result<Vec<_>, _> = mapped.collect();
        collected.map_err(|e| e.to_string())?
    };

    let total = rows.len();
    if total == 0 {
        emit(&app, SyncProgress { phase: "push".into(), current: 0, total: 0,
            file: "".into(), status: "done".into(), error: None,
            table_id: None, slot: None });
        return Ok(0);
    }

    let cache = cache_root_for(&host, cache_root.as_deref());
    let target = ssh_target(&host);
    let mut succeeded = 0usize;

    for (i, (table_id, slot, filename, pi_folder)) in rows.iter().enumerate() {
        let pf = match pi_folder.as_ref() {
            Some(s) if !s.is_empty() => s.clone(),
            _ => {
                emit(&app, SyncProgress {
                    phase: "push".into(), current: i + 1, total,
                    file: filename.clone(),
                    status: "error".into(),
                    error: Some(format!("table {} has no pi_folder", table_id)),
                    table_id: Some(*table_id), slot: Some(slot.clone()),
                });
                continue;
            }
        };
        let local_path = cache.join(&pf).join(slot).join(filename);
        let remote_path = format!("/home/pi/PinnerPi/media/{}/{}/{}", pf, slot, filename);

        emit(&app, SyncProgress {
            phase: "push".into(), current: i + 1, total,
            file: filename.clone(), status: "transferring".into(), error: None,
            table_id: Some(*table_id), slot: Some(slot.clone()),
        });

        if !local_path.exists() {
            emit(&app, SyncProgress {
                phase: "push".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(format!("local file missing: {}", local_path.display())),
                table_id: Some(*table_id), slot: Some(slot.clone()),
            });
            continue;
        }

        let dest = format!("{}:{}", target, remote_path);
        let mut cmd = low_prio_cmd("scp");
        for a in cm_args(&host) { cmd.arg(a); }
        cmd.arg(&local_path).arg(&dest);
        let result = cmd.output();

        match result {
            Ok(o) if o.status.success() => {
                let g = state.conn.lock().unwrap();
                if let Some(conn) = g.as_ref() {
                    let _ = conn.execute(
                        "UPDATE media_files SET dirty = 0 WHERE table_id = ?1 AND slot = ?2 AND filename = ?3",
                        params![table_id, slot, filename],
                    );
                }
                emit(&app, SyncProgress {
                    phase: "push".into(), current: i + 1, total,
                    file: filename.clone(), status: "synced".into(), error: None,
                    table_id: Some(*table_id), slot: Some(slot.clone()),
                });
                succeeded += 1;
            }
            Ok(o) => emit(&app, SyncProgress {
                phase: "push".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                table_id: Some(*table_id), slot: Some(slot.clone()),
            }),
            Err(e) => emit(&app, SyncProgress {
                phase: "push".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(e.to_string()),
                table_id: Some(*table_id), slot: Some(slot.clone()),
            }),
        }
    }

    emit(&app, SyncProgress {
        phase: "push".into(), current: total, total,
        file: "".into(), status: "done".into(), error: None,
        table_id: None, slot: None,
    });
    Ok(succeeded)
}

/// Pull a single table's media files from the Pi into the local cache.
/// Used by lazy fetch (when the user clicks a table in mirror mode) and
/// by the future "Mirror all tables" flow (one table at a time).
///
/// Diff-sync: a file is SKIPPED (counted as synced) if the local file already
/// matches the Pi's size + mtime. Makes incremental syncs fast.
#[tauri::command]
pub fn sync_pull_table(
    app: tauri::AppHandle,
    host: String,
    cache_root: Option<String>,
    table_id: i64,
    pi_folder: String,
    state: tauri::State<'_, DbState>,
) -> Result<usize, String> {
    // Pull slot, filename, pi_size, pi_mtime so we can diff-skip.
    let rows: Vec<(String, String, Option<i64>, Option<i64>)> = {
        let g = state.conn.lock().unwrap();
        let conn = g.as_ref().ok_or("db not open")?;
        let mut stmt = conn.prepare(
            "SELECT slot, filename, pi_size, pi_mtime FROM media_files \
             WHERE table_id = ?1 ORDER BY slot, filename"
        ).map_err(|e| e.to_string())?;
        let mapped = stmt.query_map([table_id], |r| Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
        ))).map_err(|e| e.to_string())?;
        let collected: Result<Vec<_>, _> = mapped.collect();
        collected.map_err(|e| e.to_string())?
    };

    let total = rows.len();
    if total == 0 {
        emit(&app, SyncProgress { phase: "pull".into(), current: 0, total: 0,
            file: "".into(), status: "done".into(), error: None,
            table_id: Some(table_id), slot: None });
        return Ok(0);
    }

    let cache = cache_root_for(&host, cache_root.as_deref());
    let target = ssh_target(&host);
    let mut succeeded = 0usize;
    let mut _skipped = 0usize;

    for (i, (slot, filename, pi_size, pi_mtime)) in rows.iter().enumerate() {
        // Skip non-essential files (e.g. backglass.directb2s) — see
        // is_essential_file doc for rationale.
        if !is_essential_file(filename) { continue; }
        let dir = cache.join(&pi_folder).join(slot);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            emit(&app, SyncProgress {
                phase: "pull".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(format!("mkdir {}: {}", dir.display(), e)),
                table_id: Some(table_id), slot: Some(slot.clone()),
            });
            continue;
        }
        let local_path = dir.join(filename);

        // DIFF SYNC: if local file exists AND matches Pi's size + mtime,
        // skip the SCP entirely. Emit "synced" so UI marks it as available.
        if let (Some(psize), Some(pmt)) = (pi_size, pi_mtime) {
            if let Ok(md) = std::fs::metadata(&local_path) {
                let lsize = md.len() as i64;
                let lmt = md.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                // mtime tolerance: 2 sec (FAT32 has 2-sec resolution; Linux has 1-sec)
                if lsize == *psize && (lmt - pmt).abs() <= 2 {
                    _skipped += 1;
                    emit(&app, SyncProgress {
                        phase: "pull".into(), current: i + 1, total,
                        file: filename.clone(), status: "synced".into(), error: None,
                        table_id: Some(table_id), slot: Some(slot.clone()),
                    });
                    continue;
                }
            }
        }

        emit(&app, SyncProgress {
            phase: "pull".into(), current: i + 1, total,
            file: filename.clone(), status: "transferring".into(), error: None,
            table_id: Some(table_id), slot: Some(slot.clone()),
        });

        let remote_path = format!("/home/pi/PinnerPi/media/{}/{}/{}", pi_folder, slot, filename);
        let src = format!("{}:{}", target, remote_path);

        // -p preserves mtime so future diff-checks work
        let mut cmd = low_prio_cmd("scp");
        cmd.arg("-p");
        for a in cm_args(&host) { cmd.arg(a); }
        cmd.arg(&src).arg(&local_path);
        let result = cmd.output();

        match result {
            Ok(o) if o.status.success() => {
                if let Ok(md) = std::fs::metadata(&local_path) {
                    let size = md.len() as i64;
                    let mtime = md.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let g = state.conn.lock().unwrap();
                    if let Some(conn) = g.as_ref() {
                        let _ = conn.execute(
                            "UPDATE media_files SET local_size = ?1, local_mtime = ?2 \
                             WHERE table_id = ?3 AND slot = ?4 AND filename = ?5",
                            params![size, mtime, table_id, slot, filename],
                        );
                    }
                }
                emit(&app, SyncProgress {
                    phase: "pull".into(), current: i + 1, total,
                    file: filename.clone(), status: "synced".into(), error: None,
                    table_id: Some(table_id), slot: Some(slot.clone()),
                });
                succeeded += 1;
            }
            Ok(o) => emit(&app, SyncProgress {
                phase: "pull".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                table_id: Some(table_id), slot: Some(slot.clone()),
            }),
            Err(e) => emit(&app, SyncProgress {
                phase: "pull".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(e.to_string()),
                table_id: Some(table_id), slot: Some(slot.clone()),
            }),
        }
    }

    emit(&app, SyncProgress {
        phase: "pull".into(), current: total, total,
        file: "".into(), status: "done".into(), error: None,
        table_id: Some(table_id), slot: None,
    });
    Ok(succeeded)
}

/// Pull EVERY table's media in one sequential pass. The user-facing "sync"
/// operation — emits unified global progress (file 234 of 1500), so the title
/// bar shows one continuous bar across all tables. Diff-skip per file (size +
/// mtime match → skip SCP, mark synced). On each successful pull, updates the
/// table's `last_synced_ts` so we know what was synced when.
///
/// `gitea_root` is the path to the local pinnerpi-b2s clone — when supplied,
/// .directb2s + b2s_event_map.json get merged from there into the cache so
/// the local mirror is self-contained even for tables whose .directb2s isn't
/// on the Pi.
#[tauri::command]
pub fn sync_pull_all(
    app: tauri::AppHandle,
    host: String,
    cache_root: Option<String>,
    gitea_root: Option<String>,
    state: tauri::State<'_, DbState>,
) -> Result<usize, String> {
    let _guard = try_claim_sync()?;   // refuses duplicate concurrent calls

    // Snapshot ALL files across all tables in one query.
    type Row = (i64, String, String, String, Option<i64>, Option<i64>);
    let all: Vec<Row> = {
        let g = state.conn.lock().unwrap();
        let conn = g.as_ref().ok_or("db not open")?;
        let mut stmt = conn.prepare(
            "SELECT mf.table_id, t.pi_folder, mf.slot, mf.filename, mf.pi_size, mf.pi_mtime \
             FROM media_files mf \
             JOIN tables t ON mf.table_id = t.id \
             WHERE t.pi_folder IS NOT NULL AND t.pi_folder <> '' \
             ORDER BY mf.table_id, mf.slot, mf.filename"
        ).map_err(|e| e.to_string())?;
        let mapped = stmt.query_map([], |r| Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<i64>>(5)?,
        ))).map_err(|e| e.to_string())?;
        let collected: Result<Vec<_>, _> = mapped.collect();
        collected.map_err(|e| e.to_string())?
    };

    let total = all.len();
    if total == 0 {
        emit(&app, SyncProgress {
            phase: "pull".into(), current: 0, total: 0,
            file: "".into(), status: "done".into(), error: None,
            table_id: None, slot: None,
        });
        return Ok(0);
    }

    let cache = cache_root_for(&host, cache_root.as_deref());
    let target = ssh_target(&host);
    let mut succeeded = 0usize;
    let mut last_table: Option<i64> = None;
    let now_sec = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);

    // Diagnostic counters — written to log at end so we can see why files
    // aren't being skipped when they should be.
    let mut diag_skipped = 0usize;
    let mut diag_transferred = 0usize;
    let mut diag_no_pi_meta = 0usize;
    let mut diag_no_local = 0usize;
    let mut diag_mismatch_size = 0usize;
    let mut diag_mismatch_mtime = 0usize;
    let mut first_mismatch_log: Option<String> = None;
    let mut last_emit_ts = std::time::Instant::now() - std::time::Duration::from_millis(EMIT_MIN_MS as u64 + 1);

    for (i, (table_id, pi_folder, slot, filename, pi_size, pi_mtime)) in all.iter().enumerate() {
        // Whitelist: skip non-essential files (e.g. backglass.directb2s
        // at ~18-30 MB each — saves ~4-5 GB of disk on a full sync).
        if !is_essential_file(filename) {
            continue;
        }
        // When the table changes, mark the previous one as synced in DB.
        if last_table.is_some() && last_table != Some(*table_id) {
            if let Some(prev) = last_table {
                let g = state.conn.lock().unwrap();
                if let Some(conn) = g.as_ref() {
                    let _ = conn.execute(
                        "UPDATE tables SET last_synced_ts = ?1 WHERE id = ?2",
                        params![now_sec, prev],
                    );
                }
            }
        }
        last_table = Some(*table_id);

        let dir = cache.join(pi_folder).join(slot);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            emit(&app, SyncProgress {
                phase: "pull".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(format!("mkdir {}: {}", dir.display(), e)),
                table_id: Some(*table_id), slot: Some(slot.clone()),
            });
            continue;
        }
        let local_path = dir.join(filename);

        // DIFF SKIP — local matches Pi → no transfer
        let mut skip_decision: Option<&str> = None;  // for diag
        if let (Some(psize), Some(pmt)) = (pi_size, pi_mtime) {
            match std::fs::metadata(&local_path) {
                Ok(md) => {
                    let lsize = md.len() as i64;
                    let lmt = md.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if lsize == *psize && (lmt - pmt).abs() <= 2 {
                        // Skipped-files emit dozens of events/sec during a fresh
                        // diff-sync; throttle so the UI thread stays responsive.
                        // (We always emit transferring/error/done unthrottled.)
                        emit_throttled(&app, &mut last_emit_ts, SyncProgress {
                            phase: "pull".into(), current: i + 1, total,
                            file: filename.clone(), status: "synced".into(), error: None,
                            table_id: Some(*table_id), slot: Some(slot.clone()),
                        }, false);
                        succeeded += 1;
                        diag_skipped += 1;
                        continue;
                    }
                    // Mismatch — record why
                    if lsize != *psize {
                        diag_mismatch_size += 1;
                        skip_decision = Some("size");
                        if first_mismatch_log.is_none() {
                            first_mismatch_log = Some(format!(
                                "size mismatch: {} | pi_size={} local_size={} pi_mtime={} local_mtime={}",
                                filename, psize, lsize, pmt, lmt
                            ));
                        }
                    } else {
                        diag_mismatch_mtime += 1;
                        skip_decision = Some("mtime");
                        if first_mismatch_log.is_none() {
                            first_mismatch_log = Some(format!(
                                "mtime mismatch: {} | pi_mtime={} local_mtime={} diff={}",
                                filename, pmt, lmt, (lmt - pmt).abs()
                            ));
                        }
                    }
                }
                Err(_) => {
                    diag_no_local += 1;
                    skip_decision = Some("no-local");
                }
            }
        } else {
            diag_no_pi_meta += 1;
            skip_decision = Some("no-pi-meta");
        }
        diag_transferred += 1;
        let _ = skip_decision; // (kept for inspection in debugger)

        emit(&app, SyncProgress {
            phase: "pull".into(), current: i + 1, total,
            file: filename.clone(), status: "transferring".into(), error: None,
            table_id: Some(*table_id), slot: Some(slot.clone()),
        });

        let remote_path = format!("/home/pi/PinnerPi/media/{}/{}/{}", pi_folder, slot, filename);
        let src = format!("{}:{}", target, remote_path);
        let mut cmd = low_prio_cmd("scp");
        cmd.arg("-p");
        for a in cm_args(&host) { cmd.arg(a); }
        cmd.arg(&src).arg(&local_path);
        let result = cmd.output();

        match result {
            Ok(o) if o.status.success() => {
                if let Ok(md) = std::fs::metadata(&local_path) {
                    let size = md.len() as i64;
                    let mtime = md.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let g = state.conn.lock().unwrap();
                    if let Some(conn) = g.as_ref() {
                        let _ = conn.execute(
                            "UPDATE media_files SET local_size = ?1, local_mtime = ?2 \
                             WHERE table_id = ?3 AND slot = ?4 AND filename = ?5",
                            params![size, mtime, table_id, slot, filename],
                        );
                    }
                }
                emit(&app, SyncProgress {
                    phase: "pull".into(), current: i + 1, total,
                    file: filename.clone(), status: "synced".into(), error: None,
                    table_id: Some(*table_id), slot: Some(slot.clone()),
                });
                succeeded += 1;
            }
            Ok(o) => emit(&app, SyncProgress {
                phase: "pull".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                table_id: Some(*table_id), slot: Some(slot.clone()),
            }),
            Err(e) => emit(&app, SyncProgress {
                phase: "pull".into(), current: i + 1, total,
                file: filename.clone(), status: "error".into(),
                error: Some(e.to_string()),
                table_id: Some(*table_id), slot: Some(slot.clone()),
            }),
        }
    }

    // Stamp the LAST table as fully synced
    if let Some(last) = last_table {
        let g = state.conn.lock().unwrap();
        if let Some(conn) = g.as_ref() {
            let _ = conn.execute(
                "UPDATE tables SET last_synced_ts = ?1 WHERE id = ?2",
                params![now_sec, last],
            );
        }
    }

    // ── Gitea-merge: copy .directb2s + b2s_event_map.json from the local
    //    pinnerpi-b2s clone into the cache so it's self-contained. ─────────
    let mut merged = 0usize;
    if let Some(gitea) = gitea_root.as_ref().filter(|s| !s.is_empty()) {
        // Get (id, pi_folder, local_folder) for tables that have a gitea path.
        let tables_with_gitea: Vec<(i64, String, String)> = {
            let g = state.conn.lock().unwrap();
            let conn = g.as_ref().ok_or("db not open")?;
            let mut stmt = conn.prepare(
                "SELECT id, pi_folder, local_folder FROM tables \
                 WHERE pi_folder IS NOT NULL AND pi_folder <> '' \
                 AND local_folder IS NOT NULL AND local_folder <> ''"
            ).map_err(|e| e.to_string())?;
            let mapped = stmt.query_map([], |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))).map_err(|e| e.to_string())?;
            let collected: Result<Vec<_>, _> = mapped.collect();
            collected.map_err(|e| e.to_string())?
        };
        let _ = gitea; // silence unused warning if no work (param kept for ext)
        let files_of_interest = ["backglass.directb2s", "b2s_event_map.json"];
        for (table_id, pi_folder, local_folder) in tables_with_gitea {
            for fname in &files_of_interest {
                let src = std::path::PathBuf::from(&local_folder).join(fname);
                if !src.exists() { continue; }
                let dst_dir = cache.join(&pi_folder).join("default_image");
                if let Err(_) = std::fs::create_dir_all(&dst_dir) { continue; }
                let dst = dst_dir.join(fname);

                // Skip if dst already matches src (size + mtime ~equal)
                if let (Ok(s_md), Ok(d_md)) = (std::fs::metadata(&src), std::fs::metadata(&dst)) {
                    if s_md.len() == d_md.len() {
                        let s_mt = s_md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                        let d_mt = d_md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                        if (s_mt - d_mt).abs() <= 2 {
                            // Already in cache, just mark synced
                            emit(&app, SyncProgress {
                                phase: "pull".into(), current: total + merged, total: total + merged,
                                file: (*fname).into(), status: "synced".into(), error: None,
                                table_id: Some(table_id), slot: Some("default_image".into()),
                            });
                            continue;
                        }
                    }
                }

                if std::fs::copy(&src, &dst).is_ok() {
                    merged += 1;
                    // Update media_files row (insert if needed) so it diff-skips next time
                    if let Ok(md) = std::fs::metadata(&dst) {
                        let size = md.len() as i64;
                        let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                        let g = state.conn.lock().unwrap();
                        if let Some(conn) = g.as_ref() {
                            let _ = conn.execute(
                                "INSERT INTO media_files (table_id, slot, filename, pi_size, pi_mtime, local_size, local_mtime, dirty) \
                                 VALUES (?1, 'default_image', ?2, ?3, ?4, ?3, ?4, 0) \
                                 ON CONFLICT(table_id, slot, filename) DO UPDATE SET \
                                   local_size = ?3, local_mtime = ?4",
                                params![table_id, fname, size, mtime],
                            );
                        }
                    }
                    emit(&app, SyncProgress {
                        phase: "pull".into(), current: total + merged, total: total + merged,
                        file: (*fname).into(), status: "synced".into(), error: None,
                        table_id: Some(table_id), slot: Some("default_image".into()),
                    });
                }
            }
        }
    }

    // Write diagnostic summary to the log file so we can see what happened.
    let summary = format!(
        "[sync_pull_all] total={} skipped={} transferred={} no_pi_meta={} no_local={} size_mismatch={} mtime_mismatch={} first_mismatch={:?}",
        total, diag_skipped, diag_transferred, diag_no_pi_meta, diag_no_local,
        diag_mismatch_size, diag_mismatch_mtime, first_mismatch_log
    );
    if let Some(parent) = std::path::Path::new("C:/tmp/ppenhancer.log").parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("C:/tmp/ppenhancer.log") {
        let _ = writeln!(f, "[{}] {}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
            summary);
    }

    emit(&app, SyncProgress {
        phase: "pull".into(), current: total + merged, total: total + merged,
        file: "".into(), status: "done".into(), error: None,
        table_id: None, slot: None,
    });
    Ok(succeeded + merged)
}

// Helper renamed to avoid name collision with the `cache_root` parameter above
fn cache_root_for(host: &str, custom: Option<&str>) -> PathBuf {
    cache_root(host, custom)
}

/// Walk the local mirror and delete any file that fails `is_essential_file`.
/// Returns (files_deleted, bytes_freed). Used by a "Reclaim disk" UI button to
/// reclaim ~4 GB of .directb2s files previously synced when the whitelist
/// wasn't enforced.
#[tauri::command]
pub fn sync_prune_non_essential(host: String, cache_root: Option<String>) -> Result<(usize, i64), String> {
    let cache = cache_root_for(&host, cache_root.as_deref());
    if !cache.exists() {
        return Ok((0, 0));
    }
    let mut count = 0usize;
    let mut bytes = 0i64;
    fn walk(dir: &std::path::Path, count: &mut usize, bytes: &mut i64) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                walk(&entry.path(), count, bytes);
            } else if ft.is_file() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
                if !is_essential_file(name) {
                    if let Ok(md) = entry.metadata() {
                        *bytes += md.len() as i64;
                    }
                    if std::fs::remove_file(&path).is_ok() {
                        *count += 1;
                    }
                }
            }
        }
    }
    walk(&cache, &mut count, &mut bytes);
    Ok((count, bytes))
}
