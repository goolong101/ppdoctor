<script lang="ts">
  import { sshApplyStoredCredentials } from "./api";

  let { open = $bindable(false) }: { open?: boolean } = $props();

  let ip = $state(localStorage.getItem("ppe.pi-ip") ?? "");
  let username = $state(localStorage.getItem("ppe.pi-user") ?? "pi");
  let password = $state(localStorage.getItem("ppe.pi-pw") ?? "pi");
  let cacheDir = $state(localStorage.getItem("ppe.cache-dir") ?? "");
  let showPw = $state(false);
  let saved = $state(false);

  let encodeFps = $state(localStorage.getItem("ppe.encode-fps") ?? "24");
  let encodeCrf = $state(localStorage.getItem("ppe.encode-crf") ?? "19");
  let encodeMaxrate = $state(localStorage.getItem("ppe.encode-maxrate") ?? "3M");

  async function save() {
    localStorage.setItem("ppe.pi-ip", ip.trim());
    localStorage.setItem("ppe.pi-user", username.trim() || "pi");
    localStorage.setItem("ppe.pi-pw", password);
    localStorage.setItem("ppe.cache-dir", cacheDir.trim());
    localStorage.setItem("ppe.encode-fps", encodeFps);
    localStorage.setItem("ppe.encode-crf", encodeCrf);
    localStorage.setItem("ppe.encode-maxrate", encodeMaxrate);
    // Push the new credentials into the native SSH pool so the very
    // next ssh_* command uses them. The pool's cache is keyed by
    // host:port so this just replaces the entry — no reconnect cost
    // unless the user also changed the IP.
    if (ip.trim()) {
      try { await sshApplyStoredCredentials(ip.trim()); } catch { /* ignore */ }
    }
    saved = true;
    setTimeout(() => (saved = false), 1500);
  }

  function close() { open = false; }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm flex items-center justify-center p-6"
    onclick={close}
  >
    <div
      class="glass rounded-2xl w-full max-w-md p-6 shadow-2xl shell-in"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="flex items-center justify-between mb-5">
        <h2 class="text-lg font-semibold tracking-tight">Cabinet settings</h2>
        <button
          onclick={close}
          class="w-7 h-7 flex items-center justify-center rounded-md text-zinc-500 hover:text-zinc-200 hover:bg-white/8 transition-colors"
          aria-label="Close"
        >
          <svg width="12" height="12" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M0 0L10 10M10 0L0 10"/></svg>
        </button>
      </div>

      <div class="space-y-4">
        <div>
          <label for="set-ip" class="block text-[10px] uppercase tracking-wider text-zinc-400 mb-1.5">Pi IP address</label>
          <input
            id="set-ip"
            type="text"
            bind:value={ip}
            placeholder="192.168.0.138"
            class="w-full px-3 py-2 rounded-lg bg-black/30 border border-white/8
                   text-base font-mono text-zinc-100 placeholder-zinc-600
                   focus:border-amber-400/60 focus:bg-black/40 transition-all"
          />
        </div>

        <div>
          <label for="set-user" class="block text-[10px] uppercase tracking-wider text-zinc-400 mb-1.5">SSH username</label>
          <input
            id="set-user"
            type="text"
            bind:value={username}
            placeholder="pi"
            class="w-full px-3 py-2 rounded-lg bg-black/30 border border-white/8
                   text-sm font-mono text-zinc-100 placeholder-zinc-600
                   focus:border-amber-400/60 focus:bg-black/40 transition-all"
          />
          <p class="text-[10px] text-zinc-500 mt-1">Default: pi</p>
        </div>

        <div>
          <label for="set-pw" class="block text-[10px] uppercase tracking-wider text-zinc-400 mb-1.5">SSH password</label>
          <div class="relative">
            <input
              id="set-pw"
              type={showPw ? "text" : "password"}
              bind:value={password}
              placeholder="pi"
              class="w-full px-3 py-2 pr-10 rounded-lg bg-black/30 border border-white/8
                     text-sm font-mono text-zinc-100 placeholder-zinc-600
                     focus:border-amber-400/60 focus:bg-black/40 transition-all"
            />
            <button
              type="button"
              onclick={() => (showPw = !showPw)}
              class="absolute right-2 top-1/2 -translate-y-1/2 px-1.5 py-0.5 rounded text-[10px] text-zinc-500 hover:text-zinc-200 hover:bg-white/8"
            >
              {showPw ? "hide" : "show"}
            </button>
          </div>
          <p class="text-[10px] text-zinc-500 mt-1">
            Default: pi. Stored locally — used only to bootstrap SSH key auth.
          </p>
        </div>

        <!-- separator -->
        <div class="border-t border-white/8 pt-4 mt-1">
          <div class="text-[10px] uppercase tracking-wider text-zinc-500 mb-3">Local storage</div>

          <div>
            <label for="set-cache" class="block text-[10px] uppercase tracking-wider text-zinc-400 mb-1.5">Media cache folder</label>
            <input
              id="set-cache"
              type="text"
              bind:value={cacheDir}
              placeholder="(default: %APPDATA%\PP Doctor\media-cache)"
              class="w-full px-3 py-2 rounded-lg bg-black/30 border border-white/8
                     text-sm font-mono text-zinc-100 placeholder-zinc-600
                     focus:border-amber-400/60 focus:bg-black/40 transition-all"
            />
            <p class="text-[10px] text-zinc-500 mt-1">
              Where the app stores cached media when "Full local mirror" is enabled. Leave blank for default.
            </p>
          </div>

        </div>

        <!-- separator -->
        <div class="border-t border-white/8 pt-4 mt-1">
          <div class="text-[10px] uppercase tracking-wider text-zinc-500 mb-3">Video encoding</div>

          <div class="grid grid-cols-3 gap-3">
            <div>
              <label for="set-fps" class="block text-[10px] uppercase tracking-wider text-zinc-400 mb-1.5">FPS</label>
              <select
                id="set-fps"
                bind:value={encodeFps}
                class="w-full px-2 py-2 rounded-lg bg-black/30 border border-white/8
                       text-sm text-zinc-100 focus:border-amber-400/60 transition-all"
              >
                <option value="24">24</option>
                <option value="25">25</option>
                <option value="30">30</option>
              </select>
            </div>
            <div>
              <label for="set-crf" class="block text-[10px] uppercase tracking-wider text-zinc-400 mb-1.5">CRF</label>
              <select
                id="set-crf"
                bind:value={encodeCrf}
                class="w-full px-2 py-2 rounded-lg bg-black/30 border border-white/8
                       text-sm text-zinc-100 focus:border-amber-400/60 transition-all"
              >
                <option value="17">17 (high)</option>
                <option value="19">19</option>
                <option value="21">21</option>
                <option value="23">23 (low)</option>
              </select>
            </div>
            <div>
              <label for="set-maxrate" class="block text-[10px] uppercase tracking-wider text-zinc-400 mb-1.5">Max rate</label>
              <select
                id="set-maxrate"
                bind:value={encodeMaxrate}
                class="w-full px-2 py-2 rounded-lg bg-black/30 border border-white/8
                       text-sm text-zinc-100 focus:border-amber-400/60 transition-all"
              >
                <option value="2M">2 Mbps</option>
                <option value="3M">3 Mbps</option>
                <option value="4M">4 Mbps</option>
                <option value="5M">5 Mbps</option>
              </select>
            </div>
          </div>
          <p class="text-[10px] text-zinc-500 mt-1.5">
            Pi Zero 2W decodes 1080p at ~26 fps. Use 24 fps for seamless loops.
          </p>
        </div>
      </div>

      <div class="flex items-center gap-2 mt-6">
        <button
          onclick={save}
          class="flex-1 py-2 rounded-lg font-medium text-sm
                 bg-gradient-to-b from-amber-400 to-amber-500 text-amber-950
                 hover:from-amber-300 hover:to-amber-400 transition-all
                 shadow-lg shadow-amber-500/20"
        >
          Save
        </button>
        <button
          onclick={close}
          class="px-4 py-2 rounded-lg text-sm text-zinc-300 border border-white/8 hover:bg-white/5 transition-colors"
        >
          Cancel
        </button>
      </div>

      {#if saved}
        <div class="mt-3 text-center text-xs text-emerald-300">Saved ✓</div>
      {/if}
    </div>
  </div>
{/if}
