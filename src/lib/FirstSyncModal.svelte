<script lang="ts">
  // First-connect blocking sync modal.
  //
  // Opens when sessionStorage has `ppe.first-sync-host` set (Connect
  // screen flagged a fresh cab). Runs sync_pull_all in the background
  // and shows live progress wired from the existing onSyncProgress
  // event channel (same one StatusBar uses for its bottom-bar progress).
  //
  // Blocking: while syncing, the user can't navigate tables. This is
  // intentional — the whole point is to GUARANTEE a warm cache before
  // they start clicking around, so loadB2SAttract reads local in O(ms)
  // instead of falling through to an O(seconds) SSH fetch per table.
  //
  // The modal is dismissable mid-sync (small "Skip" button) for cases
  // where the user wants to start browsing immediately and let the sync
  // continue in the background — clicking Skip just hides the modal
  // and leaves the running sync alone.

  import { onMount, onDestroy } from "svelte";
  import { syncPullAll, onSyncProgress, type SyncProgress } from "./api";
  import type { UnlistenFn } from "@tauri-apps/api/event";

  let host: string | null = $state(null);
  let visible = $state(false);
  let syncing = $state(false);
  let phase = $state("");
  let current = $state(0);
  let total = $state(0);
  let currentFile = $state("");
  let errorMsg = $state<string | null>(null);
  let unlisten: UnlistenFn | null = null;

  function cacheDir(): string | null {
    return localStorage.getItem("ppe.cache-dir") || null;
  }
  function giteaRoot(): string {
    return localStorage.getItem("ppe.b2s-repo") || "C:/ai/pinnerpi-b2s/b2s";
  }

  // Live %. derived avoids a flicker between current/total updates.
  let pct = $derived(total > 0 ? Math.round((current / total) * 100) : 0);

  async function run() {
    if (!host || syncing) return;
    syncing = true;
    errorMsg = null;
    try {
      // syncPullAll returns the count of successfully pulled files.
      // We don't need the return value here — the progress events
      // already drove the UI to its terminal state.
      await syncPullAll(host, cacheDir(), giteaRoot());
    } catch (e) {
      errorMsg = String(e);
    } finally {
      syncing = false;
      // Clear the trigger so a subsequent navigation to /tables doesn't
      // re-show the modal. If the user truly wants to redo a full pull,
      // the StatusBar's "Sync All" button is still there.
      sessionStorage.removeItem("ppe.first-sync-host");
      // Auto-dismiss when sync completes cleanly with no error. Keep
      // the modal up on error so the user can read the message.
      if (!errorMsg) visible = false;
    }
  }

  onMount(async () => {
    host = sessionStorage.getItem("ppe.first-sync-host");
    if (!host) return;

    unlisten = await onSyncProgress((p: SyncProgress) => {
      phase = p.phase || phase;
      if (typeof p.current === "number") current = p.current;
      if (typeof p.total === "number") total = p.total;
      if (p.file) currentFile = p.file;
    });

    visible = true;
    void run();
  });

  onDestroy(() => {
    unlisten?.();
  });

  function skip() {
    // Hide the modal but leave the sync running in the background.
    // StatusBar will continue showing progress.
    visible = false;
  }

  function dismissAfterError() {
    visible = false;
    errorMsg = null;
    sessionStorage.removeItem("ppe.first-sync-host");
  }
</script>

{#if visible}
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/80 backdrop-blur-sm">
    <div class="bg-zinc-900 border border-white/10 rounded-lg w-[460px] max-w-[90vw] p-6 shadow-xl">
      <div class="flex items-center gap-3 mb-4">
        <!-- Spinner that doesn't animate when error'd. -->
        {#if errorMsg}
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-red-400">
            <circle cx="12" cy="12" r="10"/>
            <line x1="12" y1="8" x2="12" y2="12"/>
            <line x1="12" y1="16" x2="12" y2="16"/>
          </svg>
        {:else}
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-amber-400 animate-spin">
            <path d="M21 12a9 9 0 1 1-9-9"/>
          </svg>
        {/if}
        <h2 class="text-sm font-semibold text-zinc-100">
          {errorMsg ? "Sync failed" : "First-time sync"}
        </h2>
      </div>

      {#if errorMsg}
        <p class="text-sm text-red-300 mb-3">{errorMsg}</p>
        <p class="text-xs text-zinc-400 mb-4">
          You can still browse tables — PP Doctor will fall back to SSH-on-demand for B2S data.
          Use the status-bar "Sync All" button to retry later.
        </p>
        <div class="flex justify-end">
          <button
            onclick={dismissAfterError}
            class="px-3 py-1.5 rounded bg-zinc-800 hover:bg-zinc-700 text-xs text-zinc-100"
          >Continue</button>
        </div>
      {:else}
        <p class="text-xs text-zinc-400 mb-4">
          Pulling backglass data from <span class="font-mono text-zinc-300">{host}</span>
          so table navigation is instant afterwards. ~30 seconds for a fresh cabinet.
        </p>

        <!-- Progress bar. -->
        <div class="h-2 bg-zinc-800 rounded overflow-hidden mb-2">
          <div
            class="h-full bg-amber-400 transition-[width] duration-100"
            style="width: {pct}%;"
          ></div>
        </div>
        <div class="flex justify-between text-[10px] text-zinc-500 mb-4">
          <span>{phase || "starting"}</span>
          <span>{current} / {total || "?"}</span>
        </div>

        {#if currentFile}
          <p class="text-[10px] text-zinc-500 font-mono truncate mb-4" title={currentFile}>
            {currentFile}
          </p>
        {/if}

        <div class="flex justify-end gap-2">
          <button
            onclick={skip}
            class="px-3 py-1.5 rounded bg-zinc-800 hover:bg-zinc-700 text-xs text-zinc-400 hover:text-zinc-100"
            title="Continue sync in the background and use the app now"
          >Skip</button>
        </div>
      {/if}
    </div>
  </div>
{/if}
