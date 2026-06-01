<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    dbDirtyCount, dbAvailableUpdatesCount,
    syncPushDirty, syncPullAll, syncPullTable, onSyncProgress, type SyncProgress
  } from "$lib/api";
  import { selection } from "$lib/selection.svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";

  let dirtyCount = $state(0);
  let updatesAvailable = $state(0);

  // Sync state — throttled to one update per animation frame to avoid
  // overwhelming Svelte's reactivity when events fire hundreds of times/sec
  // (which they do during diff-skip).
  let syncing = $state(false);
  let syncPhase = $state<"push" | "pull">("pull");
  let syncCurrent = $state(0);
  let syncTotal = $state(0);
  let syncFile = $state("");
  let syncStatus = $state<"" | "done" | "error">("");
  let syncError = $state<string | null>(null);
  let logOpen = $state(false);

  // Lightweight log buffer (most recent first). Pruned to LOG_MAX.
  type LogEntry = { ts: number; status: string; file: string; slot: string | null };
  let log = $state<LogEntry[]>([]);
  const LOG_MAX = 100;

  // rAF-throttled update queues
  let pendingUiUpdate: SyncProgress | null = null;
  let pendingLogEntries: LogEntry[] = [];
  let rafId: number | null = null;
  function scheduleFlush() {
    if (rafId !== null) return;
    rafId = requestAnimationFrame(() => {
      rafId = null;
      if (pendingUiUpdate) {
        const p = pendingUiUpdate;
        syncPhase = p.phase;
        syncCurrent = p.current;
        syncTotal = p.total;
        syncFile = p.file;
        if (p.status === "error") { syncStatus = "error"; syncError = p.error; syncing = false; }
        else if (p.status === "done") { syncStatus = "done"; syncing = false; refresh(); }
        else { syncStatus = ""; syncError = null; syncing = true; }
        pendingUiUpdate = null;
      }
      if (pendingLogEntries.length > 0) {
        log = [...pendingLogEntries.reverse(), ...log].slice(0, LOG_MAX);
        pendingLogEntries = [];
      }
    });
  }

  let unlistenProgress: UnlistenFn | null = null;
  let unlistenTraySync: UnlistenFn | null = null;
  let refreshTimer: ReturnType<typeof setInterval> | null = null;

  async function refresh() {
    try { dirtyCount = await dbDirtyCount(); } catch {}
    try { updatesAvailable = await dbAvailableUpdatesCount(); } catch {}
  }

  onMount(async () => {
    await refresh();
    refreshTimer = setInterval(refresh, 5000);

    unlistenProgress = await onSyncProgress((p: SyncProgress) => {
      // Buffer the update; rAF flushes at ≤ 60fps so UI never stalls.
      pendingUiUpdate = p;
      if (p.file && (p.status === "transferring" || p.status === "synced" || p.status === "error")) {
        pendingLogEntries.push({ ts: Date.now(), status: p.status, file: p.file, slot: p.slot });
      }
      scheduleFlush();
    });

    unlistenTraySync = await listen("tray:sync-requested", () => { startPush(); });
  });

  onDestroy(() => {
    if (refreshTimer) clearInterval(refreshTimer);
    if (rafId !== null) cancelAnimationFrame(rafId);
    unlistenProgress?.();
    unlistenTraySync?.();
  });

  function piHost(): string {
    const ip = localStorage.getItem("ppe.pi-ip") ?? "";
    const user = localStorage.getItem("ppe.pi-user") ?? "pi";
    return user && user !== "pi" ? `${user}@${ip}` : ip;
  }
  function cacheDir(): string | null { return localStorage.getItem("ppe.cache-dir") || null; }
  function giteaRoot(): string { return localStorage.getItem("ppe.b2s-repo") || "C:/ai/pinnerpi-b2s/b2s"; }

  async function startPush() {
    if (dirtyCount === 0 || syncing) return;
    const host = piHost();
    if (!host) return;
    syncing = true; syncStatus = ""; syncError = null;
    try { await syncPushDirty(host, cacheDir()); }
    catch (e) { syncStatus = "error"; syncError = String(e); syncing = false; }
  }

  async function startPullAll() {
    if (syncing) return;
    const host = piHost();
    if (!host) return;
    syncing = true; syncStatus = ""; syncError = null;
    try { await syncPullAll(host, cacheDir(), giteaRoot()); }
    catch (e) { syncStatus = "error"; syncError = String(e); syncing = false; }
  }

  async function startPullSelected() {
    if (syncing) return;
    if (selection.id === null || !selection.piFolder) return;
    const host = piHost();
    if (!host) return;
    syncing = true; syncStatus = ""; syncError = null;
    try { await syncPullTable(host, selection.id, selection.piFolder, cacheDir()); }
    catch (e) { syncStatus = "error"; syncError = String(e); syncing = false; }
  }

  let progressPct = $derived(syncTotal > 0 ? (syncCurrent / syncTotal) * 100 : 0);
  function relTime(ts: number): string {
    const s = Math.round((Date.now() - ts) / 1000);
    if (s < 1) return "now";
    if (s < 60) return `${s}s`;
    return `${Math.floor(s / 60)}m`;
  }
</script>

<!-- Log dropdown above the status bar (visible when toggled) -->
{#if logOpen}
  <div class="border-t border-white/8 bg-zinc-950/95 backdrop-blur-md max-h-56 overflow-y-auto">
    <div class="px-4 py-1.5 border-b border-white/8 flex items-center justify-between text-[10px] sticky top-0 bg-zinc-950/95 backdrop-blur-md">
      <span class="text-zinc-500 uppercase tracking-wider">Sync log</span>
      <span class="text-zinc-600 font-mono">{log.length} entries</span>
    </div>
    {#if log.length === 0}
      <div class="px-4 py-4 text-center text-xs text-zinc-600">No activity yet.</div>
    {:else}
      <ul class="py-1">
        {#each log.slice(0, 40) as e (e.ts + e.file)}
          <li class="flex items-center gap-2 px-4 py-1 text-[11px] hover:bg-white/3">
            {#if e.status === "synced"}
              <span class="text-emerald-400 w-3 flex-shrink-0">
                <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 6 L5 9 L10 3"/></svg>
              </span>
            {:else if e.status === "transferring"}
              <span class="text-amber-400 w-3 flex-shrink-0">
                <div class="w-2 h-2 rounded-full bg-amber-400"></div>
              </span>
            {:else if e.status === "error"}
              <span class="text-red-400 w-3 flex-shrink-0">✕</span>
            {/if}
            {#if e.slot}
              <span class="text-[9px] text-zinc-500 font-mono w-12 flex-shrink-0">{e.slot.replace("default_", "")}</span>
            {/if}
            <span class="font-mono text-zinc-300 truncate flex-1">{e.file}</span>
            <span class="text-[10px] text-zinc-600 font-mono flex-shrink-0">{relTime(e.ts)}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

<footer class="border-t border-white/8 bg-zinc-950/80 backdrop-blur px-4 py-1.5 flex items-center gap-3 text-xs select-none">
  <!-- Sync state -->
  <div class="flex items-center gap-2 min-w-0 flex-1">
    {#if syncing}
      <div class="w-2.5 h-2.5 rounded-full border-2 border-amber-400 border-t-transparent animate-spin flex-shrink-0"></div>
      <span class="text-amber-300 whitespace-nowrap font-medium">
        {syncPhase === "push" ? "Pushing" : "Pulling"} {syncCurrent}/{syncTotal}
      </span>
      <div class="w-32 h-1 rounded-full bg-white/10 overflow-hidden flex-shrink-0">
        <div class="h-full bg-amber-400 transition-all duration-100" style:width={`${progressPct}%`}></div>
      </div>
      <span class="text-zinc-500 font-mono flex-shrink-0">{Math.round(progressPct)}%</span>
      {#if syncFile}
        <span class="text-zinc-500 font-mono truncate min-w-0">· {syncFile}</span>
      {/if}
    {:else if syncStatus === "error"}
      <span class="text-red-300 flex items-center gap-1.5 truncate">
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2"><circle cx="6" cy="6" r="5"/><path d="M6 3 V7 M6 9 V9.01"/></svg>
        Sync error{syncError ? `: ${syncError}` : ""}
      </span>
    {:else if syncStatus === "done"}
      <span class="text-emerald-300 flex items-center gap-1.5">
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 6 L5 9 L10 3"/></svg>
        Sync complete · {syncCurrent} file{syncCurrent === 1 ? '' : 's'}
      </span>
    {:else if dirtyCount > 0}
      <button
        onclick={startPush}
        class="flex items-center gap-2 px-3 py-1 rounded-md
               bg-amber-400/15 text-amber-200 hover:bg-amber-400/25
               border border-amber-400/30 transition-colors"
      >
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 6 L6 2 L10 6 M6 2 L6 10"/></svg>
        <span class="font-medium">Push {dirtyCount} change{dirtyCount === 1 ? '' : 's'} to Pi</span>
      </button>
    {:else}
      <span class="text-zinc-500">In sync with cabinet</span>
    {/if}
  </div>

  <!-- Log toggle (shows recent file events) -->
  {#if syncing || log.length > 0}
    <button
      onclick={() => (logOpen = !logOpen)}
      class="px-2 py-0.5 rounded text-zinc-400 hover:text-zinc-200 hover:bg-white/8 transition-colors flex items-center gap-1"
      title="Toggle sync log"
    >
      <span>Log</span>
      <svg width="8" height="8" viewBox="0 0 9 9" fill="none" stroke="currentColor" stroke-width="1.5" class="transition-transform {logOpen ? 'rotate-180' : ''}">
        <path d="M2 5.5 L4.5 3 L7 5.5"/>
      </svg>
    </button>
  {/if}

  <!-- Pull buttons (only when not syncing) -->
  {#if !syncing}
    <!-- Per-table pull — the common case; just the table you're viewing. -->
    {#if selection.id !== null && selection.piFolder}
      <button
        onclick={startPullSelected}
        class="flex items-center gap-1.5 px-2.5 py-1 rounded-md
               text-sky-300 hover:text-sky-200 hover:bg-sky-500/10
               border border-sky-500/30 hover:border-sky-500/60 transition-colors"
        title="Pull just {selection.name} from cabinet (3–10 files, ~seconds)"
      >
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M6 2 L6 10 M2 6 L6 10 L10 6"/></svg>
        <span class="text-xs">Pull this table</span>
      </button>
    {/if}
    <!-- Pull-all — explicit, less prominent. Use sparingly. -->
    <button
      onclick={startPullAll}
      class="flex items-center gap-1.5 px-2.5 py-1 rounded-md
             text-zinc-500 hover:text-zinc-300 hover:bg-white/5
             border border-transparent hover:border-white/10 transition-colors"
      title="Pull ALL media from cabinet (~700 files, minutes — diff-sync skips unchanged)"
    >
      <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 4 L6 8 L10 4 M2 8 L6 12 L10 8"/></svg>
      <span class="text-xs">Pull all</span>
    </button>
  {/if}

  <!-- Updates badge -->
  {#if updatesAvailable > 0}
    <button class="flex items-center gap-1.5 px-2 py-1 rounded
                   bg-sky-500/10 text-sky-300 hover:bg-sky-500/20
                   border border-sky-500/30 transition-colors flex-shrink-0">
      <svg width="11" height="11" viewBox="0 0 12 12" fill="currentColor"><circle cx="6" cy="6" r="4"/></svg>
      <span>{updatesAvailable} update{updatesAvailable === 1 ? '' : 's'} available</span>
    </button>
  {/if}
</footer>
