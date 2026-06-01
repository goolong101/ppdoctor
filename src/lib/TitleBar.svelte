<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Logo from "./Logo.svelte";
  import SettingsModal from "./SettingsModal.svelte";

  const appWindow = getCurrentWindow();
  let isMaximized = $state(false);
  let settingsOpen = $state(false);

  appWindow.isMaximized().then(v => (isMaximized = v));
  appWindow.onResized(async () => { isMaximized = await appWindow.isMaximized(); });
</script>

<div
  data-tauri-drag-region
  class="select-none flex items-center h-9 bg-zinc-950/90 border-b border-white/8
         px-3 gap-2"
>
  <!-- Brand -->
  <div data-tauri-drag-region class="flex items-center gap-2">
    <Logo size={18} />
    <span class="font-bold text-sm tracking-tight text-zinc-100">PP Doctor</span>
  </div>

  <!-- Settings -->
  <button
    onclick={() => (settingsOpen = true)}
    class="ml-1 w-7 h-7 flex items-center justify-center rounded-md
           text-zinc-400 hover:text-amber-300 hover:bg-white/8 transition-colors"
    aria-label="Settings"
    title="Settings"
  >
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="3"/>
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>
    </svg>
  </button>

  <!-- Bloom tuning button removed 2026-05-25 — global bloom controls
       replaced by per-table B2S adjustments (right sidebar in tables view)
       and image-bloom adjustments (also per-table). Global bloomState
       defaults still live in bloom.svelte.ts as fallback. -->

  <!-- Drag-region filler -->
  <div data-tauri-drag-region class="flex-1 h-full"></div>

  <!-- Window controls -->
  <div class="flex items-center -mr-3">
    <button
      onclick={() => appWindow.minimize()}
      class="w-11 h-9 flex items-center justify-center text-zinc-400 hover:bg-white/8 hover:text-zinc-100 transition-colors"
      aria-label="Minimize"
    >
      <svg width="10" height="1" viewBox="0 0 10 1"><rect width="10" height="1" fill="currentColor"/></svg>
    </button>
    <button
      onclick={() => appWindow.toggleMaximize()}
      class="w-11 h-9 flex items-center justify-center text-zinc-400 hover:bg-white/8 hover:text-zinc-100 transition-colors"
      aria-label="Maximize"
    >
      {#if isMaximized}
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1">
          <rect x="2.5" y="0.5" width="7" height="7" />
          <rect x="0.5" y="2.5" width="7" height="7" fill="#18181b" />
          <rect x="0.5" y="2.5" width="7" height="7" />
        </svg>
      {:else}
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1">
          <rect x="0.5" y="0.5" width="9" height="9" />
        </svg>
      {/if}
    </button>
    <button
      onclick={() => appWindow.close()}
      class="w-11 h-9 flex items-center justify-center text-zinc-400 hover:bg-red-500 hover:text-white transition-colors"
      aria-label="Close"
    >
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1">
        <path d="M 0 0 L 10 10 M 10 0 L 0 10" />
      </svg>
    </button>
  </div>
</div>

<SettingsModal bind:open={settingsOpen} />
