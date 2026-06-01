// PP Doctor — local SQLite cache for cabinet metadata + sync state.
//
// Database lives at:  %APPDATA%\PP Doctor\cabinets\<host>.db
// One database per cabinet host so multiple cabinets can be managed in parallel.

use rusqlite::{Connection, params};
use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct DbState {
    pub conn: Mutex<Option<Connection>>,
}

impl DbState {
    pub fn new() -> Self {
        Self { conn: Mutex::new(None) }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DbTable {
    pub id: i64,
    pub name: String,
    pub pi_folder: Option<String>,
    pub local_folder: Option<String>,
    pub last_synced_ts: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DbMediaFile {
    pub table_id: i64,
    pub slot: String,
    pub filename: String,
    pub pi_size: Option<i64>,
    pub pi_mtime: Option<i64>,
    pub local_size: Option<i64>,
    pub local_mtime: Option<i64>,
    pub dirty: bool,
}

/// Resolve the per-cabinet db file path. Creates the parent dir.
fn db_path_for(host: &str) -> Result<PathBuf, String> {
    let base = dirs::data_dir().ok_or("no APPDATA dir")?;
    let dir = base.join("PP Doctor").join("cabinets");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {}", dir.display(), e))?;
    // Sanitize host (the only un-trusted input here) — strip anything that
    // isn't alphanumeric, '.', '_', '-'.
    let safe: String = host.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    Ok(dir.join(format!("{}.db", safe)))
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO schema_version (version) VALUES (1);

-- Cabinet table catalog
CREATE TABLE IF NOT EXISTS tables (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    pi_folder       TEXT,
    local_folder    TEXT,
    last_synced_ts  INTEGER NOT NULL DEFAULT 0
);

-- Tracked media files per (table, slot). dirty=1 → needs push to Pi.
CREATE TABLE IF NOT EXISTS media_files (
    table_id      INTEGER NOT NULL,
    slot          TEXT NOT NULL,
    filename      TEXT NOT NULL,
    pi_size       INTEGER,
    pi_mtime      INTEGER,
    local_size    INTEGER,
    local_mtime   INTEGER,
    dirty         INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (table_id, slot, filename)
);
CREATE INDEX IF NOT EXISTS idx_media_dirty ON media_files(dirty) WHERE dirty=1;
CREATE INDEX IF NOT EXISTS idx_media_table ON media_files(table_id);

-- Save / restore: a snapshot captures media_files state at a point in time.
-- Actual bytes are copied to <appdata>/PP Doctor/snapshots/<id>/<table>/<slot>/<filename>.
CREATE TABLE IF NOT EXISTS snapshots (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    scope        TEXT NOT NULL,        -- 'cabinet' or 'table'
    table_id     INTEGER,              -- NULL for cabinet-wide snapshots
    description  TEXT,
    created_ts   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS snapshot_files (
    snapshot_id  INTEGER NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
    table_id     INTEGER NOT NULL,
    slot         TEXT NOT NULL,
    filename     TEXT NOT NULL,
    size         INTEGER,
    mtime        INTEGER,
    backup_path  TEXT NOT NULL,
    PRIMARY KEY (snapshot_id, table_id, slot, filename)
);

-- Remote update feed (GitHub portal). On startup we fetch the JSON manifest,
-- upsert any new releases here, surface "X updates available" in the UI.
CREATE TABLE IF NOT EXISTS remote_updates (
    release_id    TEXT PRIMARY KEY,
    feed_url      TEXT NOT NULL,
    kind          TEXT NOT NULL,         -- 'binary' | 'b2s_pack' | 'media_pack'
    version       TEXT,
    title         TEXT,
    released_ts   INTEGER,
    asset_url     TEXT,
    installed_ts  INTEGER,
    status        TEXT NOT NULL DEFAULT 'available'  -- available|installed|skipped|failed
);

-- Free-form per-cabinet settings (default attract speed, last sync URL, etc.)
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT
);
"#;

/// Open (or create) the cabinet db. Stored in DbState's mutex.
#[tauri::command]
pub fn db_open(host: String, state: tauri::State<'_, DbState>) -> Result<String, String> {
    let path = db_path_for(&host)?;
    let conn = Connection::open(&path).map_err(|e| format!("open db: {}", e))?;
    conn.execute_batch(SCHEMA).map_err(|e| format!("schema: {}", e))?;
    *state.conn.lock().unwrap() = Some(conn);
    Ok(path.display().to_string())
}

/// Bulk-upsert tables (called after the initial scan).
#[tauri::command]
pub fn db_upsert_tables(rows: Vec<DbTable>, state: tauri::State<'_, DbState>) -> Result<usize, String> {
    let mut g = state.conn.lock().unwrap();
    let conn = g.as_mut().ok_or("db not open")?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO tables (id, name, pi_folder, local_folder, last_synced_ts) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(id) DO UPDATE SET \
               name=excluded.name, pi_folder=excluded.pi_folder, \
               local_folder=excluded.local_folder, last_synced_ts=excluded.last_synced_ts"
        ).map_err(|e| e.to_string())?;
        for r in rows {
            stmt.execute(params![r.id, r.name, r.pi_folder, r.local_folder, r.last_synced_ts]).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

#[tauri::command]
pub fn db_get_tables(state: tauri::State<'_, DbState>) -> Result<Vec<DbTable>, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    let mut stmt = conn.prepare("SELECT id, name, pi_folder, local_folder, last_synced_ts FROM tables ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows: Result<Vec<DbTable>, _> = stmt.query_map([], |r| {
        Ok(DbTable {
            id: r.get(0)?,
            name: r.get(1)?,
            pi_folder: r.get(2)?,
            local_folder: r.get(3)?,
            last_synced_ts: r.get(4)?,
        })
    }).map_err(|e| e.to_string())?.collect();
    rows.map_err(|e| e.to_string())
}

/// Bulk-replace media_files for one table (full snapshot from a Pi scan).
#[tauri::command]
pub fn db_replace_media(table_id: i64, files: Vec<DbMediaFile>, state: tauri::State<'_, DbState>) -> Result<usize, String> {
    let mut g = state.conn.lock().unwrap();
    let conn = g.as_mut().ok_or("db not open")?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    // Keep dirty rows so a sync isn't dropped by a metadata refresh.
    tx.execute("DELETE FROM media_files WHERE table_id = ?1 AND dirty = 0", params![table_id])
        .map_err(|e| e.to_string())?;
    let mut n = 0;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO media_files (table_id, slot, filename, pi_size, pi_mtime, local_size, local_mtime, dirty) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(table_id, slot, filename) DO UPDATE SET \
               pi_size=excluded.pi_size, pi_mtime=excluded.pi_mtime"
        ).map_err(|e| e.to_string())?;
        for f in files {
            stmt.execute(params![
                f.table_id, f.slot, f.filename,
                f.pi_size, f.pi_mtime, f.local_size, f.local_mtime,
                if f.dirty { 1 } else { 0 }
            ]).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Return every media_files row in one query, ordered by table_id.
/// Used at startup so the fast-path render doesn't do 233 individual IPC
/// round-trips (was costing 30s on a cold launch, 2026-05-26).
#[tauri::command]
pub fn db_get_all_media(state: tauri::State<'_, DbState>) -> Result<Vec<DbMediaFile>, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    let mut stmt = conn.prepare(
        "SELECT table_id, slot, filename, pi_size, pi_mtime, local_size, local_mtime, dirty \
         FROM media_files ORDER BY table_id, slot, filename"
    ).map_err(|e| e.to_string())?;
    let rows: Result<Vec<DbMediaFile>, _> = stmt.query_map([], |r| {
        Ok(DbMediaFile {
            table_id: r.get(0)?,
            slot: r.get(1)?,
            filename: r.get(2)?,
            pi_size: r.get(3)?,
            pi_mtime: r.get(4)?,
            local_size: r.get(5)?,
            local_mtime: r.get(6)?,
            dirty: r.get::<_, i64>(7)? != 0,
        })
    }).map_err(|e| e.to_string())?.collect();
    rows.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn db_get_media(table_id: i64, state: tauri::State<'_, DbState>) -> Result<Vec<DbMediaFile>, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    let mut stmt = conn.prepare(
        "SELECT table_id, slot, filename, pi_size, pi_mtime, local_size, local_mtime, dirty \
         FROM media_files WHERE table_id = ?1 ORDER BY slot, filename"
    ).map_err(|e| e.to_string())?;
    let rows: Result<Vec<DbMediaFile>, _> = stmt.query_map([table_id], |r| {
        Ok(DbMediaFile {
            table_id: r.get(0)?,
            slot: r.get(1)?,
            filename: r.get(2)?,
            pi_size: r.get(3)?,
            pi_mtime: r.get(4)?,
            local_size: r.get(5)?,
            local_mtime: r.get(6)?,
            dirty: r.get::<_, i64>(7)? != 0,
        })
    }).map_err(|e| e.to_string())?.collect();
    rows.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn db_dirty_count(state: tauri::State<'_, DbState>) -> Result<i64, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    conn.query_row("SELECT COUNT(*) FROM media_files WHERE dirty = 1", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn db_dirty_files(state: tauri::State<'_, DbState>) -> Result<Vec<DbMediaFile>, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    let mut stmt = conn.prepare(
        "SELECT table_id, slot, filename, pi_size, pi_mtime, local_size, local_mtime, dirty \
         FROM media_files WHERE dirty = 1 ORDER BY table_id, slot, filename"
    ).map_err(|e| e.to_string())?;
    let rows: Result<Vec<DbMediaFile>, _> = stmt.query_map([], |r| {
        Ok(DbMediaFile {
            table_id: r.get(0)?,
            slot: r.get(1)?,
            filename: r.get(2)?,
            pi_size: r.get(3)?,
            pi_mtime: r.get(4)?,
            local_size: r.get(5)?,
            local_mtime: r.get(6)?,
            dirty: r.get::<_, i64>(7)? != 0,
        })
    }).map_err(|e| e.to_string())?.collect();
    rows.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn db_mark_dirty(table_id: i64, slot: String, filename: String, state: tauri::State<'_, DbState>) -> Result<(), String> {
    // DIAGNOSTIC: log every Rust-side db_mark_dirty call to a file the
    // user already tails for [markDirty] entries. Combined with the JS
    // wrapper log we get full coverage of WHO marks WHAT dirty when.
    // Remove once the multi-file-push mystery is resolved (2026-05-26).
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("C:/tmp/ppenhancer.log") {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(f, "[{}] [RUST mark_dirty] T{} {}/{}", ts, table_id, slot, filename);
    }

    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    // UPSERT — a freshly dropped file may not have a row yet (rows are
    // created by the meta scan); without the INSERT path, dbMarkDirty
    // would silently no-op and syncPushDirty would have nothing to push.
    conn.execute(
        "INSERT INTO media_files (table_id, slot, filename, dirty) VALUES (?1, ?2, ?3, 1) \
         ON CONFLICT(table_id, slot, filename) DO UPDATE SET dirty = 1",
        params![table_id, slot, filename]
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn db_clear_dirty(table_id: i64, slot: String, filename: String, state: tauri::State<'_, DbState>) -> Result<(), String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    conn.execute(
        "UPDATE media_files SET dirty = 0 WHERE table_id = ?1 AND slot = ?2 AND filename = ?3",
        params![table_id, slot, filename]
    ).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Snapshots (save / restore) ──────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub id: i64,
    pub scope: String,
    pub table_id: Option<i64>,
    pub description: Option<String>,
    pub created_ts: i64,
}

#[tauri::command]
pub fn db_create_snapshot(scope: String, table_id: Option<i64>, description: Option<String>, state: tauri::State<'_, DbState>) -> Result<i64, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    conn.execute(
        "INSERT INTO snapshots (scope, table_id, description, created_ts) VALUES (?1, ?2, ?3, ?4)",
        params![scope, table_id, description, ts]
    ).map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn db_list_snapshots(state: tauri::State<'_, DbState>) -> Result<Vec<Snapshot>, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    let mut stmt = conn.prepare(
        "SELECT id, scope, table_id, description, created_ts FROM snapshots ORDER BY created_ts DESC"
    ).map_err(|e| e.to_string())?;
    let rows: Result<Vec<Snapshot>, _> = stmt.query_map([], |r| {
        Ok(Snapshot {
            id: r.get(0)?, scope: r.get(1)?, table_id: r.get(2)?,
            description: r.get(3)?, created_ts: r.get(4)?,
        })
    }).map_err(|e| e.to_string())?.collect();
    rows.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn db_delete_snapshot(id: i64, state: tauri::State<'_, DbState>) -> Result<(), String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    conn.execute("DELETE FROM snapshots WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Remote updates feed (GitHub portal) ─────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteUpdate {
    pub release_id: String,
    pub feed_url: String,
    pub kind: String,
    pub version: Option<String>,
    pub title: Option<String>,
    pub released_ts: Option<i64>,
    pub asset_url: Option<String>,
    pub installed_ts: Option<i64>,
    pub status: String,
}

#[tauri::command]
pub fn db_upsert_updates(rows: Vec<RemoteUpdate>, state: tauri::State<'_, DbState>) -> Result<usize, String> {
    let mut g = state.conn.lock().unwrap();
    let conn = g.as_mut().ok_or("db not open")?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO remote_updates (release_id, feed_url, kind, version, title, released_ts, asset_url, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'available') \
             ON CONFLICT(release_id) DO UPDATE SET \
               feed_url=excluded.feed_url, kind=excluded.kind, version=excluded.version, \
               title=excluded.title, released_ts=excluded.released_ts, asset_url=excluded.asset_url"
        ).map_err(|e| e.to_string())?;
        for r in rows {
            stmt.execute(params![r.release_id, r.feed_url, r.kind, r.version, r.title, r.released_ts, r.asset_url])
                .map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

#[tauri::command]
pub fn db_available_updates_count(state: tauri::State<'_, DbState>) -> Result<i64, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    conn.query_row("SELECT COUNT(*) FROM remote_updates WHERE status = 'available'", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

// ─── Settings kv ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn db_get_setting(key: String, state: tauri::State<'_, DbState>) -> Result<Option<String>, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get::<_, String>(0))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other.to_string()),
        })
}

#[tauri::command]
pub fn db_set_setting(key: String, value: String, state: tauri::State<'_, DbState>) -> Result<(), String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// One table's report after the local-mirror audit. `missing` lists which
/// of the essential files (backglass.b2scache, backglass.b2s_base.thumb.jpg)
/// are absent locally. Empty `missing` = fully cached.
#[derive(Debug, Serialize, Deserialize)]
pub struct EssentialsGap {
    pub table_id: i64,
    pub name: String,
    pub folder: String,
    pub missing: Vec<String>,
}

/// Walk the local cache mirror and report every table missing essential
/// files (cache + thumb). Used post-sync to surface coverage gaps the
/// user reported 2026-05-27 (PBA World Champion Soccer had an empty
/// default_image/ folder despite the sync running). Filesystem-only,
/// no Pi roundtrip — the result reflects whatever the LOCAL mirror has
/// right now. Pair with a re-run of sync_pull_table for the listed tables
/// when the user wants to fill the gap.
#[tauri::command]
pub fn db_audit_essentials(
    host: String,
    cache_dir: String,
    state: tauri::State<'_, DbState>,
) -> Result<Vec<EssentialsGap>, String> {
    let g = state.conn.lock().unwrap();
    let conn = g.as_ref().ok_or("db not open")?;
    let mut stmt = conn
        .prepare("SELECT id, name, pi_folder FROM tables WHERE pi_folder IS NOT NULL ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let safe_host: String = host
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    let base = PathBuf::from(&cache_dir).join(&safe_host);

    // Essentials list — keep in sync with sync.rs::is_essential_file
    // (b2scache + thumb only; event_map is optional; .glow is derived).
    const ESSENTIALS: &[&str] = &["backglass.b2scache", "backglass.b2s_base.thumb.jpg"];

    let mut out = Vec::new();
    for (id, name, folder) in rows {
        let mut missing = Vec::new();
        for fname in ESSENTIALS {
            let p = base.join(&folder).join("default_image").join(fname);
            if !p.exists() {
                missing.push((*fname).to_string());
            }
        }
        if !missing.is_empty() {
            out.push(EssentialsGap { table_id: id, name, folder, missing });
        }
    }
    Ok(out)
}
