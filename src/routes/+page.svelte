<script lang="ts">
  import { sshTest, remoteDirSize, localCacheSize, fmtBytes } from "$lib/api";
  import { goto } from "$app/navigation";
  import Logo from "$lib/Logo.svelte";

  let ip = $state(localStorage.getItem("ppe.pi-ip") ?? "192.168.0.138");
  let busy = $state(false);
  let status = $state<{ kind: "idle" | "ok" | "err"; msg: string }>({
    kind: "idle",
    msg: ""
  });

  // Phase: connect → (probe size) → choose cache mode → enter app
  let phase = $state<"connect" | "cache-choice">("connect");
  let remoteSize = $state<number | null>(null);
  let localSize = $state<number | null>(null);
  let cacheEnabled = $state(localStorage.getItem("ppe.cache-enabled") === "true");

  async function connect() {
    if (!ip.trim()) return;
    busy = true;
    status = { kind: "idle", msg: "Reaching out…" };
    try {
      const ok = await sshTest(ip.trim());
      if (!ok) {
        status = { kind: "err", msg: "Reached host but command failed. Run ssh-copy-id pi@<ip> first." };
        return;
      }
      status = { kind: "ok", msg: "Scanning cabinet…" };
      localStorage.setItem("ppe.pi-ip", ip.trim());

      // Probe cabinet size + local cache size in parallel (cheap; ~1-2 sec)
      const [r, l] = await Promise.all([
        remoteDirSize(ip.trim(), "/home/pi/PinnerPi/media").catch(() => 0),
        localCacheSize(ip.trim()).catch(() => 0)
      ]);
      remoteSize = r;
      localSize = l;
      phase = "cache-choice";
    } catch (e) {
      status = { kind: "err", msg: String(e) };
    } finally {
      busy = false;
    }
  }

  function enterApp(enableCache: boolean) {
    cacheEnabled = enableCache;
    localStorage.setItem("ppe.cache-enabled", String(enableCache));
    // First-time-sync trigger. If the user chose to use the cache AND
    // there is currently NO local cache (localSize == 0), this is the
    // first connect to this cab — the /tables page reads this session
    // flag and shows the blocking FirstSyncModal that runs sync_pull_all
    // before the user starts navigating tables. Avoids the
    // "cold cache, every navigation falls through to SSH" experience.
    // Cleared by FirstSyncModal once the sync completes.
    if (enableCache && (localSize ?? 0) === 0) {
      sessionStorage.setItem("ppe.first-sync-host", ip.trim());
    }
    goto("/tables");
  }
</script>

<main class="min-h-screen flex items-center justify-center px-6">
  <div class="shell-in w-full max-w-md">
    <!-- Brand -->
    <div class="text-center mb-10">
      <div class="inline-flex items-center gap-3 mb-5">
        <Logo size={36} />
        <h1 class="text-3xl font-semibold tracking-tight">PP Doctor</h1>
      </div>
      <p class="text-sm text-zinc-400">House call for your PPEnhancer cabinet · media · b2s · updates</p>
    </div>

    {#if phase === "connect"}
    <!-- Connect card -->
    <div class="glass rounded-2xl p-7">
      <label for="ip" class="block text-xs uppercase tracking-wider text-zinc-400 mb-3">
        Pi IP address
      </label>
      <input
        id="ip"
        type="text"
        bind:value={ip}
        placeholder="192.168.0.138"
        autocomplete="off"
        spellcheck="false"
        disabled={busy}
        onkeydown={(e) => e.key === "Enter" && connect()}
        class="w-full px-4 py-3 rounded-xl
               bg-black/30 border border-white/8
               text-lg font-mono text-zinc-100 placeholder-zinc-600
               focus:border-amber-400/60 focus:bg-black/40
               transition-all duration-200"
      />

      <button
        onclick={connect}
        disabled={busy || !ip.trim()}
        class="mt-5 w-full py-3 rounded-xl font-medium
               bg-gradient-to-b from-amber-400 to-amber-500
               text-amber-950
               hover:from-amber-300 hover:to-amber-400
               disabled:opacity-50 disabled:cursor-not-allowed
               transition-all duration-200
               shadow-lg shadow-amber-500/20
               {busy ? 'pulse-accent' : ''}"
      >
        {busy ? "Connecting…" : "Connect"}
      </button>

      {#if status.msg}
        <div
          class="mt-5 text-sm px-3 py-2.5 rounded-lg
                 {status.kind === 'ok'
                   ? 'bg-emerald-500/10 text-emerald-300 border border-emerald-500/20'
                   : status.kind === 'err'
                   ? 'bg-red-500/10 text-red-300 border border-red-500/20'
                   : 'text-zinc-400 border border-white/5'}"
        >
          {status.msg}
        </div>
      {/if}
    </div>
    {:else}
    <!-- Cache choice card -->
    <div class="glass rounded-2xl p-7">
      <div class="text-center mb-5">
        <div class="text-emerald-300 text-sm mb-1">✓ Connected to {ip}</div>
        <div class="text-xs text-zinc-500">Choose how to manage media</div>
      </div>

      <!-- Cabinet stats -->
      <div class="grid grid-cols-2 gap-3 mb-5">
        <div class="rounded-lg border border-white/8 bg-black/20 p-3">
          <div class="text-[10px] uppercase tracking-wider text-zinc-500 mb-1">Cabinet</div>
          <div class="font-mono text-lg text-zinc-100">{remoteSize !== null ? fmtBytes(remoteSize) : "…"}</div>
          <div class="text-[10px] text-zinc-600">on Pi</div>
        </div>
        <div class="rounded-lg border border-white/8 bg-black/20 p-3">
          <div class="text-[10px] uppercase tracking-wider text-zinc-500 mb-1">Local cache</div>
          <div class="font-mono text-lg text-zinc-100">{localSize !== null ? fmtBytes(localSize) : "…"}</div>
          <div class="text-[10px] text-zinc-600">on this PC</div>
        </div>
      </div>

      <!-- Mode pickers -->
      <div class="space-y-2 mb-4">
        <button
          onclick={() => enterApp(false)}
          class="w-full text-left p-3 rounded-lg border border-white/8 hover:border-amber-400/40 hover:bg-white/3 transition-colors"
        >
          <div class="flex items-center justify-between mb-0.5">
            <span class="text-sm font-medium text-zinc-100">Lightweight</span>
            <span class="text-[10px] text-zinc-500">Recommended</span>
          </div>
          <div class="text-xs text-zinc-400">
            Metadata + thumbnails only (~50&nbsp;MB). Previews fetch on demand.
          </div>
        </button>

        <button
          onclick={() => enterApp(true)}
          class="w-full text-left p-3 rounded-lg border border-white/8 hover:border-amber-400/40 hover:bg-white/3 transition-colors"
        >
          <div class="flex items-center justify-between mb-0.5">
            <span class="text-sm font-medium text-zinc-100">Full local mirror</span>
            <span class="text-[10px] text-amber-300">{remoteSize !== null ? `~${fmtBytes(remoteSize)}` : ""}</span>
          </div>
          <div class="text-xs text-zinc-400">
            Pull every file to this PC for instant previews + offline editing. Initial sync may take a while.
          </div>
        </button>
      </div>

      <button
        onclick={() => (phase = "connect")}
        class="w-full py-2 text-xs text-zinc-500 hover:text-zinc-300 transition-colors"
      >
        ← Use a different cabinet
      </button>
    </div>
    {/if}

    <!-- Footer hint -->
    <p class="mt-8 text-center text-xs text-zinc-500">
      First time? Run <code class="font-mono text-zinc-400">ssh-copy-id pi@&lt;ip&gt;</code> on your PC to set up key auth.
    </p>
  </div>
</main>
