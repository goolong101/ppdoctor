<script lang="ts">
  // PP Doctor + Pi update checker modal.
  //
  // Anonymous GitHub API (both repos public). Reuses checkSelfUpdate /
  // checkPiUpdate / installPiUpdate from api.ts. Status is rechecked
  // every time the modal opens so the user can refresh by closing+
  // reopening; the title-bar badge subscribes to the same data once at
  // app launch.
  import {
    checkSelfUpdate, checkPiUpdate, installPiUpdate,
    type UpdateCheckResult, type InstallReport,
  } from "./api";

  let { open = $bindable(false) } = $props<{ open?: boolean }>();

  let self: UpdateCheckResult | null = $state(null);
  let pi: UpdateCheckResult | null = $state(null);
  let selfError = $state("");
  let piError = $state("");
  let loadingSelf = $state(false);
  let loadingPi = $state(false);
  let installing = $state(false);
  let report: InstallReport | null = $state(null);
  let installError = $state("");

  $effect(() => {
    if (open) {
      void refresh();
    } else {
      // Reset transient state when closing.
      report = null;
      installError = "";
    }
  });

  async function refresh() {
    self = null; pi = null;
    selfError = ""; piError = "";

    loadingSelf = true;
    try { self = await checkSelfUpdate(); }
    catch (e) { selfError = String(e); }
    finally { loadingSelf = false; }

    const ip = localStorage.getItem("ppe.pi-ip") ?? "";
    if (!ip) {
      piError = "No Pi IP saved — open Settings to configure.";
      return;
    }
    loadingPi = true;
    try { pi = await checkPiUpdate(ip); }
    catch (e) { piError = String(e); }
    finally { loadingPi = false; }
  }

  async function installPi() {
    const ip = localStorage.getItem("ppe.pi-ip") ?? "";
    if (!ip) { installError = "No Pi IP saved."; return; }
    installing = true;
    installError = "";
    report = null;
    try {
      report = await installPiUpdate(ip);
      // Re-check so the success state reflects the new installed version.
      pi = await checkPiUpdate(ip);
    } catch (e) {
      installError = String(e);
    } finally {
      installing = false;
    }
  }
</script>

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
    onclick={() => (open = false)}
    onkeydown={(e) => e.key === "Escape" && (open = false)}
    role="presentation"
  >
    <div
      class="bg-zinc-900 border border-white/10 rounded-lg w-[560px] max-w-[90vw] max-h-[80vh] overflow-y-auto"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      role="dialog"
      aria-modal="true"
      aria-label="Updates"
      tabindex="-1"
    >
      <div class="flex items-center justify-between px-5 py-3 border-b border-white/8">
        <h2 class="text-sm font-semibold text-zinc-100">Updates</h2>
        <button
          onclick={() => (open = false)}
          class="text-zinc-400 hover:text-zinc-100 text-lg leading-none"
          aria-label="Close"
        >×</button>
      </div>

      <div class="px-5 py-4 space-y-4 text-sm">
        <!-- PP Doctor self-update -->
        <section>
          <div class="flex items-center justify-between mb-2">
            <h3 class="text-xs uppercase tracking-wider text-zinc-400">PP Doctor</h3>
            {#if loadingSelf}
              <span class="text-xs text-zinc-500">Checking…</span>
            {/if}
          </div>
          {#if selfError}
            <p class="text-xs text-zinc-500">No release info: {selfError}</p>
          {:else if self}
            <div class="flex items-center justify-between">
              <div>
                <div class="text-zinc-100">Installed: <span class="font-mono">{self.installed}</span></div>
                <div class="text-zinc-400">Latest: <span class="font-mono">{self.latest}</span></div>
              </div>
              {#if self.has_update}
                <a
                  href={self.release_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  class="px-3 py-1.5 rounded bg-amber-500 text-zinc-950 text-xs font-semibold hover:bg-amber-400"
                >Download installer</a>
              {:else}
                <span class="text-xs text-green-400">Up to date</span>
              {/if}
            </div>
          {/if}
        </section>

        <div class="border-t border-white/8"></div>

        <!-- Pi-side ppdoctor update -->
        <section>
          <div class="flex items-center justify-between mb-2">
            <h3 class="text-xs uppercase tracking-wider text-zinc-400">PPEnhancer (Pi Zero)</h3>
            {#if loadingPi}
              <span class="text-xs text-zinc-500">Checking…</span>
            {/if}
          </div>
          {#if piError}
            <p class="text-xs text-zinc-500">{piError}</p>
          {:else if pi}
            <div class="flex items-center justify-between">
              <div>
                <div class="text-zinc-100">Installed: <span class="font-mono">{pi.installed}</span></div>
                <div class="text-zinc-400">Latest: <span class="font-mono">{pi.latest}</span></div>
              </div>
              {#if pi.has_update}
                <button
                  onclick={installPi}
                  disabled={installing}
                  class="px-3 py-1.5 rounded bg-amber-500 text-zinc-950 text-xs font-semibold hover:bg-amber-400 disabled:opacity-50 disabled:cursor-not-allowed"
                >{installing ? "Installing…" : "Install update"}</button>
              {:else}
                <span class="text-xs text-green-400">Up to date</span>
              {/if}
            </div>
            {#if pi.release_notes}
              <details class="mt-3">
                <summary class="text-xs text-zinc-400 cursor-pointer hover:text-zinc-200">Release notes</summary>
                <pre class="mt-2 text-xs text-zinc-300 whitespace-pre-wrap font-sans">{pi.release_notes}</pre>
              </details>
            {/if}
          {/if}

          {#if installError}
            <p class="mt-3 text-xs text-red-400">Install failed: {installError}</p>
          {/if}

          {#if report}
            <div class="mt-3 p-3 rounded bg-green-500/10 border border-green-500/20 text-xs text-green-300 space-y-1">
              <div>Service restarted: {report.service_restarted ? "yes" : "no"}</div>
              <div>Final version: <span class="font-mono">{report.final_version}</span></div>
              {#if report.files_updated.length > 0}
                <div>Updated: <span class="font-mono">{report.files_updated.join(", ")}</span></div>
              {/if}
              {#if report.files_skipped.length > 0}
                <div class="text-zinc-400">Already current: <span class="font-mono">{report.files_skipped.join(", ")}</span></div>
              {/if}
            </div>
          {/if}
        </section>
      </div>

      <div class="px-5 py-3 border-t border-white/8 flex justify-between items-center">
        <button
          onclick={refresh}
          class="text-xs text-zinc-400 hover:text-amber-300"
        >Recheck</button>
        <button
          onclick={() => (open = false)}
          class="px-3 py-1.5 rounded bg-zinc-800 hover:bg-zinc-700 text-xs text-zinc-100"
        >Close</button>
      </div>
    </div>
  </div>
{/if}
