// Typed wrapper around the Tauri Rust commands.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface SshResult {
  ok: boolean;
  stdout: string;
  stderr: string;
  exitCode: number;
}

export async function sshRun(host: string, command: string): Promise<SshResult> {
  return await invoke<SshResult>("ssh_run", { host, command });
}

export async function sshTest(host: string): Promise<boolean> {
  const r = await sshRun(host, "echo OK");
  return r.ok && r.stdout.trim() === "OK";
}

export async function sshCatText(host: string, remotePath: string): Promise<string> {
  const r = await sshRun(host, `cat ${remotePath}`);
  if (!r.ok) throw new Error(`cat ${remotePath} failed: ${r.stderr}`);
  return r.stdout;
}

/** Fetch a remote file as base64. Empty string if the file doesn't exist. */
export async function sshGetBase64(host: string, remotePath: string): Promise<string> {
  return await invoke<string>("ssh_get_base64", { host, remotePath });
}

/** List files (non-directories) in a remote directory. */
export async function sshListDir(host: string, remotePath: string): Promise<string[]> {
  return await invoke<string[]>("ssh_list_dir", { host, remotePath });
}

/** SCP a remote file and return its contents as text. Fast for big files. */
export async function scpGetText(host: string, remotePath: string): Promise<string> {
  return await invoke<string>("scp_get_text", { host, remotePath });
}

/** Read a local file (UTF-8 text). Used for .directb2s sources in the local gitea clone. */
export async function readLocalText(path: string): Promise<string> {
  return await invoke<string>("read_local_text", { path });
}

/** List subdirectories at a local path. */
export async function listLocalDirs(parent: string): Promise<string[]> {
  return await invoke<string[]>("list_local_dirs", { parent });
}

/** True if a local file/dir exists. */
export async function localPathExists(path: string): Promise<boolean> {
  return await invoke<boolean>("local_path_exists", { path });
}

/** Total bytes used by a remote directory (recursive). */
export async function remoteDirSize(host: string, remotePath: string): Promise<number> {
  return await invoke<number>("remote_dir_size", { host, remotePath });
}

/** Total bytes used by PP Doctor's local media cache for this host (0 if absent). */
export async function localCacheSize(host: string): Promise<number> {
  return await invoke<number>("local_cache_size", { host });
}

/** Read a file from the local cache as base64. Empty string if not cached. */
export async function cacheGetBase64(host: string, piFolder: string, slot: string, filename: string, cacheRoot?: string | null): Promise<string> {
  return await invoke<string>("cache_get_base64", {
    host, piFolder, slot, filename, cacheRoot: cacheRoot || null
  });
}

/** Write a UTF-8 text file into the local cache mirror. Used by the b2s
 *  adjustments push-to-cabinet flow: writes the updated event_map.json to
 *  the mirror, then the existing dirty/sync push handles the SCP. */
export async function cacheWriteText(host: string, piFolder: string, slot: string, filename: string, content: string, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("cache_write_text", {
    host, piFolder, slot, filename, content, cacheRoot: cacheRoot || null
  });
}

/** Read a file from the local cache as raw bytes (binary IPC, no base64).
 *  ~5x faster than cacheGetBase64 for large files (10MB .b2scache parses
 *  in ~50ms instead of ~300ms). Use for binary-format cache reads. */
export async function cacheGetBinary(host: string, piFolder: string, slot: string, filename: string, cacheRoot?: string | null): Promise<Uint8Array> {
  // Tauri 2 serializes Vec<u8> as binary; the receiving end gets ArrayBuffer-
  // compatible bytes directly without JSON-encoding through a string.
  const raw = await invoke<ArrayBuffer | number[] | Uint8Array>("cache_get_binary", {
    host, piFolder, slot, filename, cacheRoot: cacheRoot || null
  });
  if (raw instanceof Uint8Array) return raw;
  if (raw instanceof ArrayBuffer) return new Uint8Array(raw);
  // Fallback: number[] (older Tauri). Convert.
  return new Uint8Array(raw as number[]);
}

/** Resolve the absolute path to a cached file (regardless of whether it exists). */
export async function cacheFilePath(host: string, piFolder: string, slot: string, filename: string, cacheRoot?: string | null): Promise<string> {
  return await invoke<string>("cache_file_path", {
    host, piFolder, slot, filename, cacheRoot: cacheRoot || null
  });
}

// ─── Drop-file ingest helpers ────────────────────────────────────────────────

/** Read an absolute path on the host as raw bytes (used to ingest dropped
 *  images for in-browser canvas resize + re-encode). */
export async function readLocalBytes(path: string): Promise<Uint8Array> {
  const raw = await invoke<ArrayBuffer | number[] | Uint8Array>("read_local_bytes", { path });
  if (raw instanceof Uint8Array) return raw;
  if (raw instanceof ArrayBuffer) return new Uint8Array(raw);
  return new Uint8Array(raw as number[]);
}

/** Copy an absolute path into a cache slot, backing up any existing primary
 *  file into `.versions/` (≤5 retained). Returns bytes copied. */
export async function copyFileToCache(host: string, piFolder: string, slot: string, filename: string, srcPath: string, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("copy_file_to_cache", {
    host, piFolder, slot, filename, srcPath, cacheRoot: cacheRoot || null
  });
}

/** Write raw bytes to a cache slot, backing up any existing primary file
 *  into `.versions/` (≤5 retained). Returns bytes written. */
export async function cacheWriteBinary(host: string, piFolder: string, slot: string, filename: string, bytes: Uint8Array, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("cache_write_binary", {
    host, piFolder, slot, filename, bytes: Array.from(bytes), cacheRoot: cacheRoot || null
  });
}

export interface CacheVersion {
  filename: string;
  full_path: string;
  mtime_ms: number;
  size: number;
}

/** List backup versions for a cache file (newest first, ≤5). */
export async function listCacheVersions(host: string, piFolder: string, slot: string, filename: string, cacheRoot?: string | null): Promise<CacheVersion[]> {
  return await invoke<CacheVersion[]>("list_cache_versions", {
    host, piFolder, slot, filename, cacheRoot: cacheRoot || null
  });
}

/** Promote a backup back to the primary slot. The current primary is
 *  itself backed up first. */
export async function restoreCacheVersion(host: string, piFolder: string, slot: string, filename: string, versionFilename: string, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("restore_cache_version", {
    host, piFolder, slot, filename, versionFilename, cacheRoot: cacheRoot || null
  });
}

/** Delete EVERY backup in .versions/ matching the primary file's
 *  stem.ext pattern. Returns count of files removed. UI MUST confirm
 *  with the user first — this is irreversible. */
export async function deleteCacheVersions(host: string, piFolder: string, slot: string, filename: string, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("delete_cache_versions", {
    host, piFolder, slot, filename, cacheRoot: cacheRoot || null
  });
}

/** Delete one specific file from both the local cache mirror and the Pi,
 *  including its .versions/ backups. Optionally also deletes the paired
 *  thumb (used for videos — passes alsoDeleteThumb=true so
 *  <stem>.thumb.jpg goes with the video). Returns prefixed list. */
export async function deleteCacheFile(host: string, piFolder: string, slot: string, filename: string, alsoDeleteThumb: boolean, cacheRoot?: string | null): Promise<string[]> {
  return await invoke<string[]>("delete_cache_file", {
    host, piFolder, slot, filename, alsoDeleteThumb, cacheRoot: cacheRoot || null
  });
}

/** Check whether ffmpeg (bundled in install dir, or on PATH) is callable. */
export async function ffmpegAvailable(): Promise<boolean> {
  return await invoke<boolean>("ffmpeg_available");
}

/** Resolved ffmpeg path for diagnostics (next-to-binary, sidecar, APPDATA,
 *  or bare "ffmpeg" if relying on PATH). */
export async function ffmpegPath(): Promise<string> {
  return await invoke<string>("ffmpeg_path");
}

// ─── Generate-B2S-from-video ────────────────────────────────────────────────

/** Write raw bytes to %TEMP%/ppdoctor/<filename>, return abs path.
 *  Used by the b2s-from-video pipeline to stash a max-brightness PNG
 *  composite before invoking the scaffold script. */
export async function writeTempBytes(filename: string, bytes: Uint8Array): Promise<string> {
  return await invoke<string>("write_temp_bytes", { filename, bytes: Array.from(bytes) });
}

/** Shell out to tools/scaffold_b2s_from_png.py — threshold + connected-
 *  component label the input PNG, emit a .directb2s scaffold with one
 *  <Bulb> per detected blob. Returns the output path on success. */
export async function scaffoldB2sFromPng(
  pngPath: string,
  outputDirectb2s: string,
  b2sRepoRoot: string,
  opts?: { threshold?: number; minArea?: number; maxAreaFrac?: number; baseDim?: number }
): Promise<string> {
  return await invoke<string>("scaffold_b2s_from_png", {
    pngPath, outputDirectb2s, b2sRepoRoot,
    threshold: opts?.threshold ?? null,
    minArea: opts?.minArea ?? null,
    maxAreaFrac: opts?.maxAreaFrac ?? null,
    baseDim: opts?.baseDim ?? null,
  });
}

/** Restore a table to its as-shipped b2s-only state. Deletes any user-
 *  dropped media (backglass.jpg/png/webp/gif/bgra/bmp + backglass.mp4/
 *  webm/mkv/mov/m4v) and their .versions/ backups, both from the local
 *  cache mirror and from the Pi via SSH. Preserves b2scache, event_map,
 *  thumb, glow config. Returns a list of removed paths (prefixed
 *  "local:" or "remote:"). */
export async function resetToB2sDefault(host: string, piFolder: string, cacheRoot?: string | null): Promise<string[]> {
  return await invoke<string[]>("reset_to_b2s_default", {
    host, piFolder, cacheRoot: cacheRoot || null
  });
}

/** Re-encode a dropped video into Pi-Zero-friendly H.264 mp4 (1080p, 24/30
 *  fps based on source, +faststart). Synchronous — resolves when done.
 *  Throws if ffmpeg isn't installed. */
export async function transcodeVideoToCache(host: string, piFolder: string, slot: string, filename: string, srcPath: string, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("transcode_video_to_cache", {
    host, piFolder, slot, filename, srcPath, cacheRoot: cacheRoot || null
  });
}

/** Capture a screenshot of the primary monitor → PNG file. */
export async function takeScreenshot(path: string): Promise<string> {
  return await invoke<string>("take_screenshot", { path });
}

/** Write arbitrary text content to a file (used for state dumps). */
export async function writeStateDump(path: string, content: string): Promise<void> {
  return await invoke<void>("write_state_dump", { path, content });
}

/** Human-readable byte size. */
export function fmtBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let i = 0, v = bytes / 1024;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

/** Append a line to C:/tmp/ppenhancer.log AND console.log it. Best-effort. */
export async function log(tag: string, msg: string, data?: any): Promise<void> {
  const full = data !== undefined
    ? `${tag} ${msg} ${typeof data === "string" ? data : JSON.stringify(data)}`
    : `${tag} ${msg}`;
  // eslint-disable-next-line no-console
  console.log(full);
  try { await invoke<void>("log_line", { text: full }); } catch { /* swallow */ }
}

// ─── SQLite cache (per-cabinet metadata + sync state) ────────────────────────

export interface DbTable {
  id: number;
  name: string;
  pi_folder: string | null;
  local_folder: string | null;
  last_synced_ts: number;
}

export interface DbMediaFile {
  table_id: number;
  slot: string;            // "default_image" | "default_video" | future event folders
  filename: string;
  pi_size: number | null;
  pi_mtime: number | null;
  local_size: number | null;
  local_mtime: number | null;
  dirty: boolean;
}

/** Open / create per-cabinet db. Returns the on-disk path. */
export async function dbOpen(host: string): Promise<string> {
  return await invoke<string>("db_open", { host });
}

export async function dbUpsertTables(rows: DbTable[]): Promise<number> {
  return await invoke<number>("db_upsert_tables", { rows });
}

export async function dbGetTables(): Promise<DbTable[]> {
  return await invoke<DbTable[]>("db_get_tables");
}

export async function dbReplaceMedia(tableId: number, files: DbMediaFile[]): Promise<number> {
  return await invoke<number>("db_replace_media", { tableId, files });
}

export async function dbGetMedia(tableId: number): Promise<DbMediaFile[]> {
  return await invoke<DbMediaFile[]>("db_get_media", { tableId });
}

/** Bulk-fetch every media row in one IPC call. Used at startup to populate
 *  the per-table file lists in <50ms instead of 233 × 130ms = 30s. */
export async function dbGetAllMedia(): Promise<DbMediaFile[]> {
  return await invoke<DbMediaFile[]>("db_get_all_media");
}

export async function dbDirtyCount(): Promise<number> {
  return await invoke<number>("db_dirty_count");
}

export async function dbDirtyFiles(): Promise<DbMediaFile[]> {
  return await invoke<DbMediaFile[]>("db_dirty_files");
}

/**
 * Count media_files rows where local_size IS NOT NULL.
 * Zero = this cabinet has never had a sync_pull run for it → fresh cab,
 * connect screen flags it so /tables auto-triggers a first-time bulk sync.
 */
export async function dbSyncedCount(): Promise<number> {
  return await invoke<number>("db_synced_count");
}

export async function dbMarkDirty(tableId: number, slot: string, filename: string): Promise<void> {
  // DIAGNOSTIC: log every dbMarkDirty call with a JS stack frame so we can
  // tell who's marking what during a drop / save / resync. Captures the
  // function name of the caller + a synthetic timestamp prefix that's
  // grep-friendly in C:/tmp/ppenhancer.log. Remove once the 8-file
  // mystery is resolved (2026-05-26).
  const e = new Error();
  const stack = (e.stack ?? "").split("\n").slice(2, 5).join(" <- ").replace(/^\s+/, "");
  try { await invoke<void>("log_line", { text: `[markDirty] T${tableId} ${slot}/${filename}  via: ${stack}` }); } catch {}
  return await invoke<void>("db_mark_dirty", { tableId, slot, filename });
}

export async function dbClearDirty(tableId: number, slot: string, filename: string): Promise<void> {
  return await invoke<void>("db_clear_dirty", { tableId, slot, filename });
}

// ─── Snapshots (save / restore) ──────────────────────────────────────────────

export interface Snapshot {
  id: number;
  scope: "cabinet" | "table";
  table_id: number | null;
  description: string | null;
  created_ts: number;
}

export async function dbCreateSnapshot(scope: "cabinet" | "table", tableId: number | null, description: string | null): Promise<number> {
  return await invoke<number>("db_create_snapshot", { scope, tableId, description });
}

export async function dbListSnapshots(): Promise<Snapshot[]> {
  return await invoke<Snapshot[]>("db_list_snapshots");
}

export async function dbDeleteSnapshot(id: number): Promise<void> {
  return await invoke<void>("db_delete_snapshot", { id });
}

// ─── Remote updates feed (GitHub portal) ─────────────────────────────────────

export interface RemoteUpdate {
  release_id: string;
  feed_url: string;
  kind: "binary" | "b2s_pack" | "media_pack" | string;
  version: string | null;
  title: string | null;
  released_ts: number | null;
  asset_url: string | null;
  installed_ts: number | null;
  status: "available" | "installed" | "skipped" | "failed";
}

export async function dbUpsertUpdates(rows: RemoteUpdate[]): Promise<number> {
  return await invoke<number>("db_upsert_updates", { rows });
}

export async function dbAvailableUpdatesCount(): Promise<number> {
  return await invoke<number>("db_available_updates_count");
}

// ─── Settings kv ─────────────────────────────────────────────────────────────

export async function dbGetSetting(key: string): Promise<string | null> {
  return await invoke<string | null>("db_get_setting", { key });
}

export async function dbSetSetting(key: string, value: string): Promise<void> {
  return await invoke<void>("db_set_setting", { key, value });
}

// ─── Coverage audit ──────────────────────────────────────────────────────────

/** One table missing essential files in the local cache mirror. */
export interface EssentialsGap {
  table_id: number;
  name: string;
  folder: string;
  /** Which of the essentials are missing: any of "backglass.b2scache",
   *  "backglass.b2s_base.thumb.jpg". Empty = fully cached (won't appear
   *  in the gap list at all). */
  missing: string[];
}

/** Walk the local cache mirror and report tables missing essential files
 *  (cache + thumb). Filesystem-only — no Pi roundtrip. Call after sync
 *  completes to surface coverage holes. */
export async function dbAuditEssentials(host: string, cacheDir: string): Promise<EssentialsGap[]> {
  return await invoke<EssentialsGap[]>("db_audit_essentials", { host, cacheDir });
}

// ─── Sync engine ─────────────────────────────────────────────────────────────

export interface SyncProgress {
  phase: "push" | "pull";
  current: number;
  total: number;
  file: string;
  status: "transferring" | "synced" | "done" | "error";
  error: string | null;
  table_id: number | null;
  slot: string | null;
}

/** Push all dirty files in the DB to the Pi via SCP. Resolves when complete. */
export async function syncPushDirty(host: string, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("sync_push_dirty", { host, cacheRoot: cacheRoot || null });
}

/** Pull one table's media from the Pi into the local cache. */
export async function syncPullTable(host: string, tableId: number, piFolder: string, cacheRoot?: string | null): Promise<number> {
  return await invoke<number>("sync_pull_table", { host, cacheRoot: cacheRoot || null, tableId, piFolder });
}

/** Pull EVERY table's media in one sequential pass with unified progress.
 *  Optional `giteaRoot` merges .directb2s + event_map.json from the local
 *  gitea clone into the cache so the local mirror is self-contained. */
export async function syncPullAll(host: string, cacheRoot?: string | null, giteaRoot?: string | null): Promise<number> {
  return await invoke<number>("sync_pull_all", {
    host,
    cacheRoot: cacheRoot || null,
    giteaRoot: giteaRoot || null
  });
}

/** Subscribe to sync:progress events. Returns an unsubscribe fn. */
export async function onSyncProgress(handler: (p: SyncProgress) => void): Promise<UnlistenFn> {
  return await listen<SyncProgress>("sync:progress", (e) => handler(e.payload));
}

// ─── Update checks ────────────────────────────────────────────────
// PP Doctor self-update + Pi-side ppenhancer update.
// Both repos are public on GitHub; anonymous HTTP, no PAT needed.
// has_update is the only field the UI usually cares about.

export interface UpdateCheckResult {
  installed: string;
  latest: string;
  has_update: boolean;
  release_url: string;
  release_notes: string;
}

export interface InstallReport {
  files_updated: string[];
  files_skipped: string[];
  service_restarted: boolean;
  final_version: string;
}

/** Is there a newer PP Doctor release on GitHub than what's running? */
export async function checkSelfUpdate(): Promise<UpdateCheckResult> {
  return await invoke<UpdateCheckResult>("check_self_update");
}

/** Is there a newer ppenhancer release than what's on this Pi? Reads VERSION via SSH. */
export async function checkPiUpdate(host: string): Promise<UpdateCheckResult> {
  return await invoke<UpdateCheckResult>("check_pi_update", { host });
}

/**
 * Install the latest ppenhancer release on the Pi.
 * Per-file SHA256 check skips unchanged files; restart only if any changed.
 */
export async function installPiUpdate(host: string): Promise<InstallReport> {
  return await invoke<InstallReport>("install_pi_update", { host });
}

/** Build a data: URL from a base64 string with mime-type inferred from extension. */
export function dataUrlFor(filename: string, base64: string): string {
  if (!base64) return "";
  const ext = filename.toLowerCase().split(".").pop() ?? "";
  const mime: Record<string, string> = {
    jpg: "image/jpeg", jpeg: "image/jpeg", png: "image/png",
    webp: "image/webp", gif: "image/gif", bmp: "image/bmp",
    mp4: "video/mp4", webm: "video/webm", mkv: "video/x-matroska"
  };
  const m = mime[ext] ?? "application/octet-stream";
  return `data:${m};base64,${base64}`;
}
