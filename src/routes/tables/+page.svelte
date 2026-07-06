<script lang="ts">
  import { sshRun, sshCatText, sshGetBase64, scpGetText, readLocalText, listLocalDirs, localPathExists,
           cacheGetBase64, dataUrlFor, log,
           dbOpen, dbUpsertTables, dbReplaceMedia, syncPullAll, takeScreenshot, writeStateDump, fmtBytes, type DbMediaFile } from "$lib/api";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import B2SCanvas from "$lib/B2SCanvas.svelte";
  import Logo from "$lib/Logo.svelte";
  import { bulbAlpha as b2sBulbAlpha, type AttractSpec as B2SAttractSpec, type B2SBulb } from "$lib/b2s";

  /** Local pinnerpi-b2s gitea clone — source of truth for .directb2s files.
   *  Settable in the title-bar Settings modal (key: ppe.b2s-repo). */
  const LOCAL_B2S_ROOT = localStorage.getItem("ppe.b2s-repo") || "C:/ai/pinnerpi-b2s/b2s";

  /** Map: 4-digit zero-padded ID → local folder name. Built once on connect. */
  let localFolderById = $state<Map<string, string>>(new Map());

  /** Media type winners (renderer priority order — drop = override). */
  type Slot = "bgra" | "png" | "jpg" | "webp" | "gif" | "b2s" | "video" | "none";

  type Table = {
    id: number;
    name: string;
    folder: string;       // Pi-side folder, "0057RedCup1" or "" if no media folder
    has: Set<Slot>;       // which slots are present on the Pi
    winner: Slot;         // what the Pi would actually display
    localDirectb2s: string | null;  // local file path to .directb2s in gitea clone
  };

  let tables = $state<Table[]>([]);
  let loading = $state(true);
  let err = $state("");
  let ip = $state(localStorage.getItem("ppe.pi-ip") ?? "");
  let q = $state("");

  let selectedId = $state<number | null>(null);

  // Keep the keyboard-selected row visible. The table list uses
  // content-visibility:auto to skip rendering off-screen rows (scales to 1000+
  // tables), so arrow-key selection can land on an unpainted row — scroll it
  // into view minimally. Deferred one frame so the DOM reflects the new
  // selection (and the row is realized) before we scroll to it.
  $effect(() => {
    const id = selectedId;
    if (id == null) return;
    requestAnimationFrame(() => {
      document.querySelector(`[data-tid="${id}"]`)?.scrollIntoView({ block: "nearest" });
    });
  });
  /** Unified "auto preview" toggle. ONE state drives whatever the active
   *  default is: B2S → render B2SCanvas with attract animation; video →
   *  auto-load the <video> element; image → bloom canvas animation
   *  already runs unconditionally. Replaces the prior split glowOn /
   *  videoAutoPreview pair where flipping one wouldn't clear the other,
   *  causing the b2s canvas to keep showing when you'd switched a video
   *  as active (2026-05-27).
   *
   *  Migrates from the older `ppe.video-auto-preview` and (implicit)
   *  glowOn defaults into a single localStorage key. */
  // Default ON. The pre-existing localStorage value still overrides
  // (so users who explicitly toggled it OFF stay OFF), but new sessions
  // start with auto-preview ENABLED so video tables don't require a
  // click-through to see the first frame.
  let autoPreview = $state(
    typeof localStorage !== "undefined"
      ? (localStorage.getItem("ppe.auto-preview") !== "false" &&
         localStorage.getItem("ppe.video-auto-preview") !== "false")
      : true
  );
  $effect(() => {
    try { localStorage.setItem("ppe.auto-preview", String(autoPreview)); } catch {}
  });
  // glowOn / videoAutoPreview kept as alias derivations so the render
  // logic below doesn't change. They both follow the unified toggle.
  let glowOn = $derived(autoPreview);
  let videoAutoPreview = $derived(autoPreview);

  /** Currently-active file name (the one the user picked, or the renderer winner by default). */
  let activeFile = $state<string | null>(null);

  /** B2S attract preview state — loaded on demand via the toggle.
   *  Priority: local .directb2s (full fidelity) → fall back to .b2scache
   *  (works on every cabinet since every Pi has it). */
  let b2sXml = $state<string>("");
  let b2sCacheBuf = $state<ArrayBuffer | null>(null);
  let b2sEventMapJson = $state<string | null>(null);
  /** LRU cache of recently-loaded b2s sources keyed by tableId. Avoids
   *  re-reading the 10MB cache file when arrow-keying back to a recently-
   *  visited table. Cap 12 entries (~120MB worst case). When the user
   *  rapid-fires arrow keys, this turns N>1th visit into instant. */
  type B2SCached = { xml: string; cacheBuf: ArrayBuffer | null; eventMapJson: string | null; bytes: number };
  // RAM budget. 128 MB holds ~6-12 recent tables — enough that arrow-key
  // back-and-forth on the SAME small group stays instant, but stops the
  // app from accumulating GB of evicted-but-not-yet-GC'd state during long
  // sessions. Lowered from 512 MB after a 3 GB heap caused multi-second
  // GC pauses + Not Responding (2026-05-27). Raw cache buffer dominates
  // the per-entry cost (5-30 MB); ImageBitmaps decoded inside
  // parseB2SCache stay alive via the WeakMap there as long as the buffer
  // is in the LRU — eviction frees bitmap memory too.
  const B2S_LRU_BUDGET_BYTES = 128 * 1024 * 1024;   // 128 MB
  let b2sLru = new Map<number, B2SCached>();
  let b2sLruBytes = 0;
  function lruGet(id: number): B2SCached | undefined {
    const v = b2sLru.get(id);
    if (v) {
      // Reinsert to mark as MRU (Map iteration order = insertion).
      b2sLru.delete(id);
      b2sLru.set(id, v);
    }
    return v;
  }
  function lruPut(id: number, partial: Omit<B2SCached, "bytes">) {
    const bytes = (partial.cacheBuf?.byteLength ?? 0)
                + (partial.xml?.length ?? 0)
                + (partial.eventMapJson?.length ?? 0);
    const v: B2SCached = { ...partial, bytes };
    const prev = b2sLru.get(id);
    if (prev) b2sLruBytes -= prev.bytes;
    b2sLru.delete(id);
    b2sLru.set(id, v);
    b2sLruBytes += bytes;
    // Evict oldest until under budget. Keeps the CURRENT entry no matter what
    // — if a single entry exceeds the budget, we'll still hold it (rare).
    while (b2sLruBytes > B2S_LRU_BUDGET_BYTES && b2sLru.size > 1) {
      const firstKey = b2sLru.keys().next().value;
      if (firstKey === undefined || firstKey === id) break;
      const evicted = b2sLru.get(firstKey)!;
      b2sLru.delete(firstKey);
      b2sLruBytes -= evicted.bytes;
    }
  }

  /** Pre-fetch the cache for tables adjacent to the current one so arrow-key
   *  navigation feels instant. Reads N tables above + N below into the LRU
   *  as a background task — doesn't block the foreground b2s render.
   *  Skips entries already in LRU. Bails out if user keeps navigating
   *  (AbortController resets on every loadB2SAttract call).
   *
   *  Asymmetric: more forward than back because user-observed nav patterns
   *  favor forward arrow keys. 10 back + 20 forward = 30 cached siblings.
   *  Combined with the current entry + revisits → up to ~40 hot in LRU. */
  // Adjacent-table cache prefetch counts. Dropped from 10/20 → 2/4 (2026-05-26)
  // because 30 simultaneous binary-IPC fetches per nav saturated the channel
  // and made the UI feel locked. With 2/4 + a yield between fetches, nav stays
  // responsive and arrow-key cycling still hits the LRU for nearby tables.
  // Prefetch disabled (set to 0 each) — was speculatively pre-decoding ±6
  // tables on every selection, which on long sessions accumulated bulb
  // sprite bitmaps in the LRU + WeakMap faster than GC could reclaim,
  // contributing to the 3 GB heap. Re-enable per-session if back-and-forth
  // arrow-key navigation becomes the dominant workflow; bumping these by
  // 1 each costs roughly one extra cache buffer (~5-30 MB) per direction.
  const PREFETCH_BACKWARD = 0;
  const PREFETCH_FORWARD  = 0;
  let prefetchAbort: AbortController | null = null;
  async function prefetchAdjacent(centerId: number) {
    prefetchAbort?.abort();
    const ac = new AbortController();
    prefetchAbort = ac;
    const ip = sshHost();
    const centerIdx = tables.findIndex(t => t.id === centerId);
    if (centerIdx < 0) return;
    // Interleave forward + backward, biased forward. Pattern:
    //   +1, -1, +2, -2, ..., +PREFETCH_BACKWARD, -PREFETCH_BACKWARD, +B+1, +B+2, ...
    // So the immediate neighbors come first, expanding outward.
    const toFetch: number[] = [];
    const radius = Math.max(PREFETCH_FORWARD, PREFETCH_BACKWARD);
    for (let d = 1; d <= radius; d++) {
      if (d <= PREFETCH_FORWARD) {
        const after = tables[centerIdx + d];
        if (after?.folder && !b2sLru.has(after.id)) toFetch.push(after.id);
      }
      if (d <= PREFETCH_BACKWARD) {
        const before = tables[centerIdx - d];
        if (before?.folder && !b2sLru.has(before.id)) toFetch.push(before.id);
      }
    }
    for (const id of toFetch) {
      if (ac.signal.aborted) return;
      const t = tables.find(x => x.id === id);
      if (!t?.folder) continue;
      // Yield to the event loop between prefetches so UI clicks / scroll
      // get serviced. Without this, sequential binary-IPC fetches saturate
      // the channel and the app feels locked while prefetch runs.
      await new Promise(r => setTimeout(r, 50));
      if (ac.signal.aborted) return;
      try {
        // Fast path: binary IPC. Falls back to base64 if unregistered.
        let bytes: Uint8Array | null = null;
        try {
          const got = await (await import("$lib/api")).cacheGetBinary(ip, t.folder, "default_image", "backglass.b2scache", cacheDir());
          if (got && got.byteLength > 0) bytes = got;
        } catch { /* fall through */ }
        if (!bytes) {
          // Don't fall back to base64 for prefetch — it's too slow and we'd
          // hog the IPC channel. If binary IPC isn't available, just skip
          // prefetch entirely; first-visit will pay the base64 cost once.
          continue;
        }
        if (ac.signal.aborted) return;
        const buf = bytes.buffer.byteLength === bytes.byteLength
          ? bytes.buffer
          : bytes.slice().buffer;
        // Also prefetch the event_map (small file, base64 OK)
        let emJson: string | null = null;
        try {
          const emB64 = await cacheGetBase64(ip, t.folder, "default_image", "b2s_event_map.json", cacheDir());
          if (emB64) emJson = atob(emB64);
        } catch { /* table may have no event_map */ }
        if (ac.signal.aborted) return;
        lruPut(id, { xml: "", cacheBuf: buf, eventMapJson: emJson });
        log("[b2s/prefetch]", `cached table=${id} bytes=${bytes.length}`);
      } catch (e) {
        // Quiet — prefetch is opportunistic, errors don't matter
      }
    }
  }
  let b2sLoading = $state(false);
  let b2sError = $state<string>("");

  async function loadB2SAttract() {
    if (!selected) return;
    const t0 = performance.now();
    b2sLoading = true;
    b2sError = "";
    b2sXml = "";
    b2sCacheBuf = null;
    b2sEventMapJson = null;

    // LRU fast-path: previously parsed this table → reuse the buffers
    // (no disk read, no IPC, no decode). Arrow-key navigation back to
    // a visited table is instant.
    const cached = lruGet(selected.id);
    if (cached) {
      b2sXml = cached.xml;
      b2sCacheBuf = cached.cacheBuf;
      b2sEventMapJson = cached.eventMapJson;
      b2sLoading = false;
      log("[b2s]", `LRU hit table=${selected.id} took=${Math.round(performance.now() - t0)}ms`);
      // Keep the prefetch window centered around the user's current pick
      void prefetchAdjacent(selected.id);
      return;
    }

    try {
      // ── Priority order (changed 2026-05-25 per user) ───────────────────
      // The Pi renders from .b2scache; whatever's in the cache is the
      // authoritative cabinet state. PP Doctor should preview the SAME thing
      // the user sees on the cabinet, so we try the cache FIRST (local
      // mirror copy, synced from Pi). Fall back to local .directb2s when
      // the cache isn't synced yet — directb2s gives full fidelity for
      // tables that haven't been deployed yet.
      // Background: local gitea .directb2s can drift from what's deployed
      // (e.g. CV's "SAM scaffold" rewrite on 2026-05-20 reduced its bulb set
      // while the Pi cache predates the rewrite by ~17 hours). Preferring
      // cache makes "preview matches cabinet" hold without manual diffs.
      if (!selected.folder) {
        b2sError = "Table has no media folder on the Pi.";
        return;
      }

      // ─── Detailed step-by-step timing for cache load ─────────────────
      // Goal: see EXACTLY where time goes. Log every phase so a slow load
      // can be diagnosed without code-reading. Each phase logs ms delta
      // from start, and the wall-clock delta of just that phase.
      let bytes: Uint8Array | null = null;
      const mark = (label: string, prevT: number, bytesIn?: number) => {
        const now = performance.now();
        const phaseMs = Math.round(now - prevT);
        const totalMs = Math.round(now - t0);
        const extra = bytesIn !== undefined ? ` bytes=${bytesIn}` : "";
        log("[b2s/perf]", `t+${totalMs}ms (+${phaseMs}ms) ${label}${extra}`);
        return now;
      };
      let phaseT = t0;

      // (0) — active.json override. Phase 2 (2026-05-29) replaced the
      // hardcoded backglass_PP.directb2s sidecar with a per-slot
      // active.json that names which file is currently live. Read it
      // first, and if it nominates a .directb2s, load that file by
      // name. Backwards-compat fallback to backglass_PP.directb2s if
      // active.json is missing (tables that pre-date this feature).
      try {
        const active = await readActiveConfig(selected.folder, "default_image");
        let activeName = active.directb2s;
        if (!activeName) {
          // Pre-active.json fallback — if backglass_PP.directb2s exists,
          // synthesize the same override behavior.
          const probe = await cacheGetBase64(ip, selected!.folder, "default_image", "backglass_PP.directb2s", cacheDir()).catch(() => "");
          if (probe) activeName = "backglass_PP.directb2s";
        }
        if (activeName) {
          const b2sB64 = await cacheGetBase64(ip, selected!.folder, "default_image", activeName, cacheDir());
          if (b2sB64) {
            const xml = atob(b2sB64);
            if (xml) {
              b2sXml = xml;
              phaseT = mark(`active directb2s hit=${activeName} chars=${xml.length}`, phaseT);
              const emB64 = await cacheGetBase64(ip, selected!.folder, "default_image", "b2s_event_map.json", cacheDir()).catch(() => "");
              if (emB64) b2sEventMapJson = atob(emB64);
              lruPut(selected.id, { xml: b2sXml, cacheBuf: null, eventMapJson: b2sEventMapJson });
              log("[b2s]", `load DONE (active=${activeName}) table=${selected.id} total=${Math.round(performance.now() - t0)}ms`);
              return;
            }
          }
        }
      } catch (e) {
        phaseT = mark(`active.json probe throw: ${String(e).slice(0,60)}`, phaseT);
      }

      // (1a) — fast binary path (needs Rust rebuild). Direct ArrayBuffer.
      let triedBinary = false;
      try {
        triedBinary = true;
        const got = await (await import("$lib/api")).cacheGetBinary(ip, selected!.folder, "default_image", "backglass.b2scache", cacheDir());
        if (got && got.byteLength > 0) bytes = got;
        phaseT = mark(`cacheGetBinary ${bytes ? "hit" : "miss/empty"}`, phaseT, bytes?.byteLength);
      } catch (e) {
        phaseT = mark(`cacheGetBinary throw (command unregistered — needs cargo build): ${String(e).slice(0,60)}`, phaseT);
      }
      // (1b) — base64 IPC fallback (slower, but works pre-rebuild).
      if (!bytes) {
        try {
          const tIpcStart = performance.now();
          const cacheB64 = await cacheGetBase64(ip, selected!.folder, "default_image", "backglass.b2scache", cacheDir());
          const tIpcEnd = performance.now();
          log("[b2s/perf]", `t+${Math.round(tIpcEnd - t0)}ms (+${Math.round(tIpcEnd - tIpcStart)}ms) cacheGetBase64 returned b64chars=${cacheB64.length}`);
          if (cacheB64) {
            const tAtob = performance.now();
            const bin = atob(cacheB64);
            const tCopy = performance.now();
            log("[b2s/perf]", `t+${Math.round(tCopy - t0)}ms (+${Math.round(tCopy - tAtob)}ms) atob done binChars=${bin.length}`);
            bytes = new Uint8Array(bin.length);
            for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
            phaseT = mark("char-copy → Uint8Array", tCopy, bytes.length);
          }
        } catch (e) {
          phaseT = mark(`cacheGetBase64 throw: ${String(e).slice(0,60)}`, phaseT);
        }
      }
      // (2) — SSH fetch (only if local mirror missing).
      if (!bytes) {
        try {
          const piPath = `${PI_MEDIA}/${selected.folder}/default_image/backglass.b2scache`;
          const cacheB64 = await sshGetBase64(ip, piPath);
          phaseT = mark(`sshGetBase64 returned b64chars=${cacheB64.length}`, phaseT);
          if (cacheB64) {
            const bin = atob(cacheB64);
            bytes = new Uint8Array(bin.length);
            for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
            phaseT = mark("ssh atob+copy", phaseT, bytes.length);
          }
        } catch (e) {
          phaseT = mark(`ssh fetch throw: ${String(e).slice(0,60)}`, phaseT);
        }
      }
      if (bytes) {
        // Hand off ArrayBuffer to B2SCanvas. Zero-copy when buffer is exact-size.
        b2sCacheBuf = bytes.buffer.byteLength === bytes.byteLength
          ? bytes.buffer
          : bytes.slice().buffer;
        phaseT = mark("b2sCacheBuf assigned (Svelte reactive write)", phaseT);

        // Event map: local mirror → SSH fallback.
        let emB64 = await (async () => {
          try { return await cacheGetBase64(ip, selected!.folder, "default_image", "b2s_event_map.json", cacheDir()); }
          catch { return ""; }
        })();
        phaseT = mark(`event_map local b64chars=${emB64.length}`, phaseT);
        if (!emB64) {
          try {
            const piPath = `${PI_MEDIA}/${selected.folder}/default_image/b2s_event_map.json`;
            emB64 = await sshGetBase64(ip, piPath);
            phaseT = mark(`event_map ssh b64chars=${emB64.length}`, phaseT);
          } catch { /* table may have no event_map */ }
        }
        if (emB64) {
          b2sEventMapJson = atob(emB64);
          phaseT = mark("event_map atob", phaseT);
        }
        // Cache for arrow-key revisits.
        lruPut(selected.id, { xml: "", cacheBuf: b2sCacheBuf, eventMapJson: b2sEventMapJson });
        // Background prefetch ±5 adjacent tables so the next arrow-key
        // hits LRU. No-op if binary IPC not built yet — base64 prefetch
        // would hog the IPC channel for a ~5s/file cold path.
        void prefetchAdjacent(selected.id);
        log("[b2s]", `load DONE (cache) table=${selected.id} total=${Math.round(performance.now() - t0)}ms`);
        return;
      }

      // Priority 2: local .directb2s from gitea clone (dev fallback when
      // cache isn't synced — useful for new tables or after relabel work).
      if (selected.localDirectb2s) {
        try {
          const xml = await readLocalText(selected.localDirectb2s);
          if (xml) {
            b2sXml = xml;
            log("[b2s]", `directb2s table=${selected.id} chars=${xml.length} (cache unavailable, fallback)`);
            const emPath = selected.localDirectb2s.replace(/backglass\.directb2s$/, "b2s_event_map.json");
            if (await localPathExists(emPath)) {
              b2sEventMapJson = await readLocalText(emPath);
            }
            lruPut(selected.id, { xml: b2sXml, cacheBuf: null, eventMapJson: b2sEventMapJson });
            log("[b2s]", `load DONE (directb2s) table=${selected.id} total=${Math.round(performance.now() - t0)}ms`);
            return;
          }
        } catch (e) {
          log("[b2s]", `directb2s read failed: ${e}`);
        }
      }

      b2sError = "No backglass.b2scache in local mirror AND no local .directb2s — run Pull this table from the cabinet to sync.";
      log("[b2s]", `no source for table=${selected.id} folder=${selected.folder}`);
    } catch (e) {
      b2sError = String(e);
      log("[b2s]", `load ERROR table=${selected?.id} err=${String(e)}`);
    } finally {
      b2sLoading = false;
    }
  }

  // Reload b2s whenever the selected table OR the toggle changes.
  // Note: deliberately does NOT read b2sXml/b2sLoading inside the effect, so
  // setting those during loadB2SAttract() doesn't retrigger the effect.
  // The B2S editor (genB2sActive) ALSO triggers the load: opening Edit B2S
  // with auto-preview off was leaving B2SCanvas with empty b2sXml/cacheBuf,
  // so the editor painted only a stub of the backglass. Treat genB2sActive
  // as an implicit "I want to see the b2s" signal.
  $effect(() => {
    const on = glowOn || genB2sActive;
    const sel = selected;
    if (!on || !sel) {
      b2sXml = ""; b2sCacheBuf = null; b2sEventMapJson = null; b2sError = "";
      return;
    }
    if (sel.has.has("b2s")) {
      loadB2SAttract();
    }
  });

  /** Files we hide from the user-facing file list — internal cache / config. */
  function isUserFacing(filename: string): boolean {
    const lower = filename.toLowerCase();
    // Support/derived files that must never show in the media list. Matched by
    // suffix (not exact name) because most are table-prefixed, e.g.
    // Bally_Black_Rose_PP_event_map.json / Bally_Black_Rose_PP.directb2s.
    if (lower.endsWith(".thumb.jpg")) return false;    // auto-thumbs (b2s + video)
    if (lower.endsWith("event_map.json")) return false; // b2s_event_map.json + <table>_event_map.json
    if (lower.endsWith(".b2scache")) return false;      // b2s cache, not source
    if (filename === "glow_config.json") return false;  // edited via UI later
    if (filename === "active.json") return false;       // renderer-side control file
    return true;
  }

  function fileSlot(filename: string): "img" | "vid" | "other" {
    const e = filename.toLowerCase().split(".").pop() ?? "";
    if (["bgra","png","jpg","jpeg","webp","bmp","gif"].includes(e)) return "img";
    if (["mp4","webm","mkv","mov","avi","ts","m4v"].includes(e)) return "vid";
    return "other";
  }

  /** Pick the file the renderer would actually use right now (the "winner" file). */
  function rendererWinner(imgs: string[], vids: string[]): string | null {
    const priority = ["bgra","png","jpg","jpeg","webp","bmp","gif"];
    for (const ext of priority) {
      const hit = imgs.find(f => f.toLowerCase().endsWith("." + ext) && isUserFacing(f));
      if (hit) return hit;
    }
    // No user image — video wins over b2s, matching the Pi renderer's
    // fallback priority (renderer.cpp:3062 "video → image → b2s"). So a
    // table with both a video and a b2s previews the video, like the cabinet.
    const v = vids.find(f => /\.(mp4|webm|mkv|mov|avi)$/i.test(f));
    if (v) return v;
    // Otherwise the B2S cache is the de-facto backglass; represent it by the
    // synthetic "__b2s__" line (handled in the template).
    if (imgs.includes("backglass.b2scache") || imgs.includes("backglass.directb2s")) {
      return "__b2s__";
    }
    return null;
  }

  /** Persisted per-table active-file selection. localStorage key is the
   *  table id; value is either a filename (jpg/mp4/etc.) or the literal
   *  "__b2s__" sentinel. Used by pickActiveFile to honor the user's
   *  selection across table-navigation cycles instead of falling back to
   *  the renderer-priority default on every load. */
  function activeFileKey(id: number): string { return `ppe.active-file.${id}`; }
  function loadActiveFile(id: number): string | null {
    const v = localStorage.getItem(activeFileKey(id));
    return v && v.length > 0 ? v : null;
  }
  /** Resolve the active file for a table: saved choice (if it still exists
   *  in the file lists) → renderer-priority default. Returns "__b2s__" for
   *  b2s selection, a filename otherwise, or null if the table has nothing. */
  function pickActiveFile(id: number, has: Set<Slot>, imgs: string[], vids: string[]): string | null {
    const saved = loadActiveFile(id);
    if (saved === "__b2s__" && has.has("b2s")) return "__b2s__";
    if (saved && (imgs.includes(saved) || vids.includes(saved))) return saved;
    return rendererWinner(imgs, vids);
  }

  async function setActive(filename: string) {
    activeFile = filename;
    if (selected) setActiveSaved(selected.id, filename);
    if (!selected?.folder) return;
    // Phase 2 (2026-05-29): tie the radio-button pick to active.json so
    // the Pi renderer + PP Doctor preview both honor the user's choice
    // when multiple candidate files exist in a folder. Sentinel
    // "__b2s__" doesn't map to a single file — it's a "show the b2s
    // backglass" toggle — so we skip writing active.json for it.
    if (filename && filename !== "__b2s__") {
      const lower = filename.toLowerCase();
      if (lower.endsWith(".directb2s")) {
        try {
          await writeActiveConfig(selected.id, selected.folder, "default_image", { directb2s: filename });
          b2sLru.delete(selected.id);
          await refreshDirtyTables();
          // Force a b2s reload so the preview picks up the new active
          // file immediately (the b2sLoad $effect normally only re-fires
          // on selected/glowOn changes; active.json swaps aren't a
          // reactive trigger).
          if (!glowOn) glowOn = true;
          await loadB2SAttract();
        } catch (e) { log("[active]", `write directb2s failed: ${e}`); }
      } else if (fileSlot(filename) === "img") {
        try {
          await writeActiveConfig(selected.id, selected.folder, "default_image", { image: filename });
          await refreshDirtyTables();
        } catch (e) { log("[active]", `write image failed: ${e}`); }
      } else if (fileSlot(filename) === "vid") {
        try {
          await writeActiveConfig(selected.id, selected.folder, "default_video", { video: filename });
          await refreshDirtyTables();
        } catch (e) { log("[active]", `write video failed: ${e}`); }
      }
    }

    /** Cache-first base64 fetch — falls back to SSH on miss. */
    async function fetchPreview(slotName: string, fname: string): Promise<string> {
      try {
        const b64 = await cacheGetBase64(ip, selected!.folder, slotName, fname, cacheDir());
        if (b64) return b64;
      } catch {}
      return await sshGetBase64(ip, `${PI_MEDIA}/${selected!.folder}/${slotName}/${fname}`);
    }

    if (filename === "__b2s__") {
      const b64 = await fetchPreview("default_image", "backglass.b2s_base.thumb.jpg");
      imageDataUrl = dataUrlFor("backglass.b2s_base.thumb.jpg", b64);
      videoDataUrl = "";
      return;
    }

    const slot = fileSlot(filename);
    if (slot === "img") {
      const b64 = await fetchPreview("default_image", filename);
      imageDataUrl = dataUrlFor(filename, b64);
      videoDataUrl = "";
    } else if (slot === "vid") {
      const b64 = await fetchPreview("default_video", filename);
      videoDataUrl = dataUrlFor(filename, b64);
      imageDataUrl = "";
    }
  }

  // Detail-pane state
  let detailLoading = $state(false);
  let imageFiles = $state<string[]>([]);
  let videoFiles = $state<string[]>([]);
  let activeImageFile = $state<string | null>(null);
  let activeVideoFile = $state<string | null>(null);
  let imageDataUrl = $state<string>("");
  let videoDataUrl = $state<string>("");
  /** Paired <stem>.thumb.jpg shown as the <video poster> so video tables
   *  display the first-frame preview immediately on table switch — no
   *  click-through needed even when video data is still loading. */
  let videoThumbUrl = $state<string | null>(null);

  // In-memory cache: revisiting a table is instant (no SSH roundtrip).
  type CachedDetail = {
    imageFiles: string[];
    videoFiles: string[];
    activeImageFile: string | null;
    activeVideoFile: string | null;
    imageDataUrl: string;
  };
  const detailCache = new Map<number, CachedDetail>();

  const PI_MEDIA = "/home/pi/PinnerPi/media";

  /** Renderer priority — drop-override beats b2s, b2s beats nothing. */
  function pickWinner(has: Set<Slot>): Slot {
    if (has.has("bgra")) return "bgra";
    if (has.has("png"))  return "png";
    if (has.has("jpg"))  return "jpg";
    if (has.has("webp")) return "webp";
    if (has.has("gif"))  return "gif";
    if (has.has("b2s"))  return "b2s";
    if (has.has("video")) return "video";
    return "none";
  }

  /** Reactive shadow of the per-table active-file choice. localStorage is the
   *  persistence layer; this map is what Svelte tracks for the left-list
   *  badge re-render. Reads from localStorage on first lookup (lazy), writes
   *  here on every setActive / drop / restore / reset. */
  let activeFileMap = $state<Record<number, string>>({});
  function setActiveSaved(id: number, value: string) {
    try { localStorage.setItem(activeFileKey(id), value); } catch {}
    activeFileMap = { ...activeFileMap, [id]: value };
  }
  function getActiveSaved(id: number): string | null {
    if (id in activeFileMap) return activeFileMap[id];
    const v = loadActiveFile(id);
    return v;
  }

  /** Slot to render on the left-list badge for table `t`. Honors the user's
   *  persisted active-file choice — if they picked a video as the cabinet
   *  default, the row shows VIDEO instead of B2S. Falls back to the
   *  renderer-priority winner when nothing is saved. Reactive via
   *  activeFileMap so the badge updates immediately on radio click / drop. */
  function badgeSlot(t: { id: number; has: Set<Slot>; winner: Slot }): Slot {
    const saved = getActiveSaved(t.id);
    if (!saved) return t.winner;
    if (saved === "__b2s__") return t.has.has("b2s") ? "b2s" : t.winner;
    const ext = saved.toLowerCase().split(".").pop() ?? "";
    if (ext === "mp4" || ext === "webm" || ext === "mkv" || ext === "mov" || ext === "m4v") {
      return t.has.has("video") ? "video" : t.winner;
    }
    const slot = extToSlot(saved);
    if (slot && t.has.has(slot)) return slot;
    return t.winner;
  }

  function extToSlot(filename: string): Slot | null {
    const e = filename.toLowerCase().split(".").pop() ?? "";
    if (e === "bgra") return "bgra";
    if (e === "png")  return "png";
    if (e === "jpg" || e === "jpeg") return "jpg";
    if (e === "webp") return "webp";
    if (e === "gif")  return "gif";
    return null;
  }

  /** SSH host string including username if non-default. */
  function sshHost(): string {
    const user = localStorage.getItem("ppe.pi-user") ?? "pi";
    return user && user !== "pi" ? `${user}@${ip}` : ip;
  }

  /** Cache dir override (or null for default). */
  function cacheDir(): string | null {
    return localStorage.getItem("ppe.cache-dir") || null;
  }

  /** Per-file synced state, keyed by `${tableId}:${slot}:${filename}`.
   *  Populated by sync:progress events with status="synced" and on table load
   *  from the DB's local_size field. */
  let syncedFiles = $state<Set<string>>(new Set());
  function syncKey(tableId: number, slot: string, filename: string) {
    return `${tableId}:${slot}:${filename}`;
  }
  function isFileSynced(slot: string, filename: string): boolean {
    if (!selected) return false;
    return syncedFiles.has(syncKey(selected.id, slot, filename));
  }

  /** Trigger a background sync of EVERY table in one unified pass.
   *  Title bar shows global progress (file N of total). Diff-skip means
   *  files already present in local cache are skipped instantly.
   *  Also merges .directb2s + event_map from the local gitea clone so the
   *  cache is self-contained even for tables whose .directb2s isn't on Pi. */
  async function pullAllTables() {
    try {
      await syncPullAll(sshHost(), cacheDir(), LOCAL_B2S_ROOT);
    } catch (e) {
      log("[sync]", `pull-all error ${e}`);
    }
    // Post-sync coverage audit — finds any table whose default_image/
    // is missing backglass.b2scache or its thumb. User-reported gap
    // 2026-05-27 (PBA World Champion Soccer had empty default_image
    // despite sync running). Surfaces silent sync drops so they can be
    // re-pulled instead of hitting the 60s directb2s-fallback path.
    try {
      const cd = cacheDir();
      if (cd) {
        const { dbAuditEssentials } = await import("$lib/api");
        const gaps = await dbAuditEssentials(sshHost(), cd);
        essentialsGaps = gaps;
        if (gaps.length > 0) {
          log("[sync/audit]", `${gaps.length} table${gaps.length === 1 ? '' : 's'} missing essentials: ${gaps.slice(0, 10).map(g => `${g.table_id}:${g.missing.join(',')}`).join("; ")}${gaps.length > 10 ? '…' : ''}`);
        } else {
          log("[sync/audit]", `all tables have local cache + thumb ✓`);
        }
      } else {
        // Lightweight mode (no local cache) — nothing to audit, the
        // "tables missing essentials" concept doesn't apply.
        essentialsGaps = [];
      }
    } catch (e) {
      log("[sync/audit]", `audit failed (Rust rebuild needed?): ${e}`);
    }
  }

  /** Tables found missing essential files by the post-sync audit. Empty
   *  list = no gaps. Surfaced in the UI as a banner so the user knows
   *  which tables would hit slow-load fallback instead of having the
   *  fast .b2scache path. */
  let essentialsGaps = $state<import("$lib/api").EssentialsGap[]>([]);
  let essentialsExpanded = $state(false);
  let essentialsPulling = $state(false);

  /** Re-pull just the tables flagged by the audit. Iterates the gap list
   *  and calls sync_pull_table for each (one-shot, no progress events).
   *  When done, re-runs the audit so the banner reflects the new state. */
  async function pullMissingEssentials() {
    if (essentialsPulling || essentialsGaps.length === 0) return;
    essentialsPulling = true;
    try {
      const { syncPullTable, dbAuditEssentials } = await import("$lib/api");
      const toPull = essentialsGaps.slice();   // snapshot — list will change as we go
      let ok = 0, fail = 0;
      const cd = cacheDir();
      if (!cd) {
        log("[sync/audit]", `lightweight mode — no cache dir, skipping pull-missing`);
        return;
      }
      for (const g of toPull) {
        try {
          await syncPullTable(sshHost(), g.table_id, g.folder, cd);
          ok++;
        } catch (e) {
          fail++;
          log("[sync/audit]", `pull failed table=${g.table_id} (${g.name}): ${e}`);
        }
      }
      essentialsGaps = await dbAuditEssentials(sshHost(), cd);
      log("[sync/audit]", `pull-missing done: ok=${ok} fail=${fail} remaining=${essentialsGaps.length}`);
      if (essentialsGaps.length === 0) essentialsExpanded = false;
    } catch (e) {
      log("[sync/audit]", `pull-missing error: ${e}`);
    } finally {
      essentialsPulling = false;
    }
  }

  /** Set of tableIds with at least one dirty (local-edited but not pushed)
   *  file. Drives the per-row sync icon on the left list. Refreshed on init
   *  and after every sync:progress "done" event. */
  let dirtyTableIds = $state<Set<number>>(new Set());
  async function refreshDirtyTables() {
    try {
      const { dbDirtyFiles } = await import("$lib/api");
      const rows = await dbDirtyFiles();
      const next = new Set<number>();
      for (const r of rows) next.add(r.table_id);
      dirtyTableIds = next;
    } catch (e) {
      log("[sync]", `refreshDirtyTables error ${e}`);
    }
  }

  /** Push just this table's dirty files to the Pi. Uses the same
   *  syncPushDirty command (pushes every dirty row in DB) — narrower
   *  scoping would require a new Tauri command, but in practice the
   *  per-row sync icon is meant for "I edited this one table, push it
   *  now" so pushing all-dirty is functionally equivalent. */
  async function syncTable(tableId: number) {
    try {
      const { syncPushDirty } = await import("$lib/api");
      await syncPushDirty(sshHost(), cacheDir());
      await refreshDirtyTables();
    } catch (e) {
      log("[sync]", `syncTable ${tableId} error ${e}`);
    }
  }

  // ── Drop-file ingest (image / video) ──────────────────────────────────
  // Drag a file from Explorer onto PP Doctor's preview area:
  //   - images (jpg/png/webp/gif/bgra)  → resize to ≤1920×1080 + reencode
  //     as JPEG q=0.92, save as backglass.jpg
  //   - videos (mp4/webm/mkv/mov)       → ffmpeg transcode to 1080p mp4
  //     (24 or 30 fps based on source, +faststart for seek)
  // Either path: backup existing primary into .versions/ (≤5 retained),
  // dbMarkDirty + refreshDirtyTables, then push.
  let isDragOver = $state(false);
  let dropBusy = $state(false);
  let dropStatus = $state<string>("");
  let dropError  = $state<string>("");
  let dropVersions = $state<import("$lib/api").CacheVersion[]>([]);
  let dropVersionsFor = $state<{ tableId: number | null; slot: string; filename: string }>({
    tableId: null, slot: "", filename: ""
  });

  const IMAGE_EXTS = new Set(["jpg","jpeg","png","webp","gif","bgra","bmp"]);
  const VIDEO_EXTS = new Set(["mp4","webm","mkv","mov","m4v"]);
  /** Per-slot "which file is currently active" config. Lives at
   *  <table>/<slot>/active.json, e.g.:
   *    default_image/active.json  →  {"image": "backglass.jpg", "directb2s": "Pinner1.directb2s"}
   *    default_video/active.json  →  {"video": "Bally_Black_Rose_PP.mp4"}
   *  Replaces the Pi renderer's pickRandomWithFallback so the user can
   *  control WHICH file in a folder with multiple is the live one.
   *  Phase 2 step 1 — wired up 2026-05-29. */
  type ActiveConfig = { image?: string; video?: string; directb2s?: string };

  async function readActiveConfig(folder: string, slot: string): Promise<ActiveConfig> {
    try {
      const { cacheGetBase64 } = await import("$lib/api");
      const b64 = await cacheGetBase64(sshHost(), folder, slot, "active.json", cacheDir());
      if (!b64) return {};
      const txt = atob(b64);
      return JSON.parse(txt) as ActiveConfig;
    } catch { return {}; }
  }

  async function writeActiveConfig(tableId: number, folder: string, slot: string, patch: ActiveConfig): Promise<void> {
    const { cacheWriteText, dbMarkDirty } = await import("$lib/api");
    const cur = await readActiveConfig(folder, slot);
    const next: ActiveConfig = { ...cur, ...patch };
    const txt = JSON.stringify(next, null, 2);
    await cacheWriteText(sshHost(), folder, slot, "active.json", txt, cacheDir());
    await dbMarkDirty(tableId, slot, "active.json");
  }

  function classifyDropExt(path: string): "image" | "video" | "directb2s" | "unsupported" {
    const ext = path.toLowerCase().split(".").pop() ?? "";
    if (ext === "directb2s") return "directb2s";
    if (IMAGE_EXTS.has(ext)) return "image";
    if (VIDEO_EXTS.has(ext)) return "video";
    return "unsupported";
  }

  async function refreshDropVersions(tableId: number, folder: string, slot: string, filename: string) {
    try {
      const { listCacheVersions } = await import("$lib/api");
      dropVersions = await listCacheVersions(sshHost(), folder, slot, filename, cacheDir());
      dropVersionsFor = { tableId, slot, filename };
    } catch (e) {
      log("[drop/versions]", `${e}`);
    }
  }

  /** Per-file delete (from the Files list row). Wipes the primary file +
   *  its .versions/ backups from local mirror AND the Pi. For videos,
   *  also removes the paired <stem>.thumb.jpg. Confirms before deleting. */
  async function deleteFile(slot: "default_image" | "default_video", filename: string) {
    const id = selected?.id;
    const folder = selected?.folder;
    if (id === undefined || id === null || !folder) return;
    const isVideo = slot === "default_video";
    if (!confirm(
      `Delete ${filename} from this table?\n\n` +
      `Removes from PP Doctor mirror AND the Pi (including .versions/ backups${isVideo ? ' and paired thumb' : ''}).\n\n` +
      `This cannot be undone.`
    )) return;
    try {
      const { deleteCacheFile, dbDeleteMedia } = await import("$lib/api");
      const removed = await deleteCacheFile(sshHost(), folder, slot, filename, isVideo, cacheDir());
      log("[delete-file]", `${slot}/${filename}: removed ${removed.length} entries`);
      // Wipe in-memory file list + the DB ROW (not just its dirty flag — else
      // navigating away and back re-reads the surviving row and the file
      // reappears) + the detailCache entry (same reason, in-memory).
      if (slot === "default_image") imageFiles = imageFiles.filter(f => f !== filename);
      else                          videoFiles = videoFiles.filter(f => f !== filename);
      try { await dbDeleteMedia(id, slot, filename); } catch {}
      detailCache.delete(id);
      // If we just deleted the active file, fall back to whatever
      // pickActiveFile picks (b2s if available, else first remaining file).
      if (activeFile === filename) {
        activeFile = pickActiveFile(id, selected?.has ?? new Set(), imageFiles, videoFiles);
        setActiveSaved(id, activeFile ?? "");
      }
      if (slot === "default_image" && activeImageFile === filename) {
        activeImageFile = null;
        imageDataUrl = "";
        bloomSourceUrl = ""; bloomSourceImg = null;
      }
      if (slot === "default_video" && activeVideoFile === filename) {
        activeVideoFile = null;
        videoDataUrl = "";
      }
      // Refresh the versions panel if we were viewing this file's history
      if (dropVersionsFor.tableId === id && dropVersionsFor.slot === slot && dropVersionsFor.filename === filename) {
        dropVersions = [];
        dropVersionsFor = { tableId: null, slot: "", filename: "" };
      }
      await refreshDirtyTables();
      dropStatus = `Deleted ${filename} (${removed.length} entries)`;
    } catch (e) {
      dropError = `Delete failed: ${e}`;
      log("[delete-file]", `error ${e}`);
    }
  }

  async function deleteAllVersions() {
    const { tableId, slot, filename } = dropVersionsFor;
    const folder = selected?.folder;
    if (tableId === null || !folder || !slot || !filename) return;
    const n = dropVersions.length;
    if (!confirm(
      `Delete all ${n} backup version(s) of ${filename}?\n\n` +
      `This wipes the .versions/ folder for this slot on the local mirror.\n` +
      `The active primary file stays untouched.\n\n` +
      `This cannot be undone.`
    )) return;
    try {
      const { deleteCacheVersions } = await import("$lib/api");
      const removed = await deleteCacheVersions(sshHost(), folder, slot, filename, cacheDir());
      dropVersions = [];
      dropStatus = `Removed ${removed} backup version(s)`;
      log("[versions]", `removed ${removed} for ${slot}/${filename}`);
    } catch (e) {
      dropError = `Delete versions failed: ${e}`;
    }
  }

  async function restoreDropVersion(version_filename: string) {
    const { tableId, slot, filename } = dropVersionsFor;
    const folder = selected?.folder;
    if (tableId === null || !folder || !slot || !filename) return;
    try {
      const { restoreCacheVersion, dbMarkDirty, syncPushDirty } = await import("$lib/api");
      await restoreCacheVersion(sshHost(), folder, slot, filename, version_filename, cacheDir());
      await dbMarkDirty(tableId, slot, filename);
      await refreshDirtyTables();
      await syncPushDirty(sshHost(), cacheDir());
      await refreshDirtyTables();
      await refreshDropVersions(tableId, folder, slot, filename);
      dropStatus = `Restored ${version_filename}`;
    } catch (e) {
      dropError = `restore failed: ${e}`;
    }
  }

  /** Resize+reencode pipeline for dropped images. */
  async function ingestImage(absPath: string, tableId: number, folder: string) {
    const { readLocalBytes, cacheWriteBinary, dbMarkDirty, syncPushDirty } = await import("$lib/api");
    dropStatus = "Reading image…";
    const bytes = await readLocalBytes(absPath);
    const ext = absPath.toLowerCase().split(".").pop() ?? "bin";
    const mime = ({jpg:"image/jpeg",jpeg:"image/jpeg",png:"image/png",webp:"image/webp",gif:"image/gif",bmp:"image/bmp"} as any)[ext] ?? "application/octet-stream";
    const blob = new Blob([bytes], { type: mime });
    let bitmap: ImageBitmap;
    try { bitmap = await createImageBitmap(blob); }
    catch (e) { throw new Error(`decode failed: ${e}`); }
    const srcW = bitmap.width, srcH = bitmap.height;
    // Fit within 1920×1080 preserving aspect; never upscale.
    const scale = Math.min(1, 1920 / srcW, 1080 / srcH);
    const dstW = Math.max(1, Math.round(srcW * scale));
    const dstH = Math.max(1, Math.round(srcH * scale));
    dropStatus = `Resizing ${srcW}×${srcH} → ${dstW}×${dstH}, encoding JPEG…`;
    const canvas = document.createElement("canvas");
    canvas.width = dstW; canvas.height = dstH;
    const ctx = canvas.getContext("2d", { alpha: false })!;
    ctx.fillStyle = "#000"; ctx.fillRect(0, 0, dstW, dstH);
    ctx.drawImage(bitmap, 0, 0, dstW, dstH);
    bitmap.close();
    const outBlob: Blob | null = await new Promise(res => canvas.toBlob(res, "image/jpeg", 0.92));
    if (!outBlob) throw new Error("toBlob returned null");
    const outBytes = new Uint8Array(await outBlob.arrayBuffer());
    const filename = "backglass.jpg";
    const slot = "default_image";
    dropStatus = `Writing ${outBytes.length} bytes to mirror…`;
    await cacheWriteBinary(sshHost(), folder, slot, filename, outBytes, cacheDir());
    await dbMarkDirty(tableId, slot, filename);
    await refreshDirtyTables();
    // Make the dropped file appear in the Files panel immediately (next
    // meta-scan will re-detect it from disk; this just avoids a stale UI).
    if (!imageFiles.includes(filename)) imageFiles = [...imageFiles, filename];
    // Force the preview to refresh with the new bytes (drop the source-url
    // cache, invalidate the bloom bitmap).
    bloomSourceUrl = "";
    bloomSourceImg = null;
    activeImageFile = filename;
    // User just dropped this — promote it AND persist so it survives table
    // navigation. Without the localStorage write, next loadDetail would
    // recompute via rendererWinner and revert to b2s for b2s-having tables.
    activeFile = filename;
    setActiveSaved(tableId, filename);
    // Invalidate detailCache so navigation away+back re-reads new files.
    detailCache.delete(tableId);
    // Re-fetch the data URL so the preview shows the new image.
    try {
      const { cacheGetBase64 } = await import("$lib/api");
      const b64 = await cacheGetBase64(sshHost(), folder, slot, filename, cacheDir());
      if (b64) imageDataUrl = dataUrlFor(filename, b64);
    } catch { /* preview stays on previous frame */ }
    dropStatus = "Pushing to cabinet…";
    await syncPushDirty(sshHost(), cacheDir());
    await refreshDirtyTables();
    await refreshDropVersions(tableId, folder, slot, filename);
    // Same fix as ingestVideo: bump the parent table's inventory + winner
    // so the left-side list shows the new image badge without a relaunch.
    const t = tables.find(x => x.id === tableId);
    if (t) {
      const newHas = new Set(t.has);
      newHas.add("jpg");
      t.has = newHas;
      t.winner = pickWinner(newHas);
      tables = [...tables];
    }
    dropStatus = `Done — ${dstW}×${dstH} JPEG pushed (${(outBytes.length/1024).toFixed(0)} KB)`;
  }

  /** Drop a .directb2s file (typically a scaffold from tools/
   *  scaffold_b2s_from_png.py). Lands at default_image/backglass_PP.
   *  directb2s — a SIDECAR rather than overwriting the original
   *  backglass.directb2s. The Pi renderer prefers _PP.directb2s when
   *  present (see renderer.cpp tryLoadB2SBase 2026-05-27 fix), so the
   *  user gets the dropped file as the live backglass without losing
   *  the original. The Pi will regenerate backglass.b2scache on next
   *  load — stat-based cache validation auto-invalidates on source
   *  swap. To revert: delete backglass_PP.directb2s from the local
   *  cache + the Pi. */
  async function ingestDirectb2s(absPath: string, tableId: number, folder: string) {
    const { readLocalBytes, cacheWriteBinary, dbMarkDirty, syncPushDirty } = await import("$lib/api");
    dropStatus = "Reading .directb2s…";
    const bytes = await readLocalBytes(absPath);
    // Preserve the source filename — user-requested 2026-05-29 so
    // multiple drops coexist (Pinner1.directb2s + pinner2.directb2s)
    // and the Files panel shows what was actually authored. Which one
    // is LIVE is now recorded in default_image/active.json (Phase 2);
    // PP Doctor's loadB2SAttract + the Pi renderer both read that to
    // resolve the active .directb2s (no more random pick, no more
    // backglass_PP symlink).
    const srcLeaf = absPath.split(/[/\\]/).pop() ?? "backglass.directb2s";
    const namedFilename = srcLeaf.toLowerCase().endsWith(".directb2s")
      ? srcLeaf
      : `${srcLeaf}.directb2s`;
    const slot = "default_image";
    dropStatus = `Writing ${(bytes.length/1024).toFixed(0)} KB as ${namedFilename}…`;
    await cacheWriteBinary(sshHost(), folder, slot, namedFilename, bytes, cacheDir());
    // Cache-only cabinet: the .directb2s stays in the local mirror for editing but
    // is NOT marked dirty (not pushed to the Pi). PP Doctor generates
    // backglass.b2scache here on the PC and pushes THAT — keeping the source off
    // the cabinet is what lets the Pi trust the PC-made cache without a mtime
    // re-check. See docs/b2scache_writer_spec.md.
    try {
      dropStatus = "Generating b2scache…";
      const { parseDirectB2S } = await import("$lib/b2s");
      const { generateAndWriteB2sCache } = await import("$lib/b2s_cache_writer");
      const doc = parseDirectB2S(new TextDecoder().decode(bytes));
      const n = await generateAndWriteB2sCache(doc, bytes, tableId, sshHost(), folder, cacheDir());
      log("[b2scache]", `generated ${Math.round(n / 1024)} KB for table ${tableId}`);
    } catch (e) {
      log("[b2scache]", `generation failed: ${e}`);
      dropError = `b2scache generation failed: ${e}`;
    }
    dropStatus = `Marking ${namedFilename} as active…`;
    await writeActiveConfig(tableId, folder, slot, { directb2s: namedFilename });
    await refreshDirtyTables();
    if (!imageFiles.includes(namedFilename)) imageFiles = [...imageFiles, namedFilename];
    if (!imageFiles.includes("active.json")) imageFiles = [...imageFiles, "active.json"];
    const filename = namedFilename;
    // Bump the table's has/winner so the left list updates its b2s badge
    // immediately for tables that previously had none.
    const t = tables.find(x => x.id === tableId);
    if (t) {
      const newHas = new Set(t.has);
      newHas.add("b2s");
      t.has = newHas;
      t.winner = pickWinner(newHas);
      tables = [...tables];
    }
    // Invalidate detailCache so a re-navigation picks up the new file.
    detailCache.delete(tableId);
    // ── Promote the new b2s as the table's ACTIVE preview ──
    // Without this, the preview pane keeps showing whatever was active
    // before (image/video) and the user sees no visible change after
    // the drop — exact bug user reported 2026-05-29. Mirrors how
    // ingestImage promotes the dropped JPEG to activeFile.
    //   - "__b2s__" is the sentinel for "show the B2S backglass".
    //   - lruDelete drops the stale parsed b2s so loadB2SAttract
    //     re-reads from local cache (which now has the just-dropped
    //     bytes).
    //   - glowOn=true triggers the B2S branch immediately rather than
    //     waiting for auto-preview toggle.
    b2sLru.delete(tableId);
    if (selected?.id === tableId) {
      activeImageFile = "__b2s__";
      activeFile = "__b2s__";
      setActiveSaved(tableId, "__b2s__");
      if (!glowOn) glowOn = true;
    }
    dropStatus = "Pushing to cabinet…";
    await syncPushDirty(sshHost(), cacheDir());
    await refreshDirtyTables();
    await refreshDropVersions(tableId, folder, slot, filename);
    dropStatus = `Done — ${(bytes.length/1024).toFixed(0)} KB sidecar pushed`;
  }

  /** ffmpeg transcode pipeline for dropped videos. */
  async function ingestVideo(absPath: string, tableId: number, folder: string) {
    const { ffmpegAvailable, transcodeVideoToCache, copyFileToCache, dbMarkDirty, syncPushDirty } = await import("$lib/api");
    const slot = "default_video";
    const titleStem = sanitizeFilenameStem(selected?.name ?? "backglass");
    // ffmpeg always emits mp4 (transcode). Raw-copy fallback preserves
    // the source extension since we're not changing the container.
    const srcExt = (absPath.toLowerCase().split(".").pop() ?? "mp4");
    const hasFfmpeg = await ffmpegAvailable();
    const filename = hasFfmpeg
      ? `${titleStem}_PP.mp4`
      : `${titleStem}.${srcExt}`;
    if (hasFfmpeg) {
      dropStatus = "Transcoding video (ffmpeg)…";
      await transcodeVideoToCache(sshHost(), folder, slot, filename, absPath, cacheDir());
    } else {
      dropStatus = "ffmpeg not found — copying as-is (may stutter if >1080p)…";
      await copyFileToCache(sshHost(), folder, slot, filename, absPath, cacheDir());
    }
    await dbMarkDirty(tableId, slot, filename);
    // Mark the paired thumb dirty too (only present when ffmpeg ran the
    // transcode — raw-copy fallback doesn't generate one). Pi's
    // preloadThumbs picks up <stem>.thumb.jpg next to the video on the
    // next restart; until the renderer thumb-first-paint change ships,
    // this file sits unused but harmless.
    if (hasFfmpeg) {
      const dot2 = filename.lastIndexOf(".");
      const fstem = dot2 > 0 ? filename.slice(0, dot2) : filename;
      const thumbName = `${fstem}.thumb.jpg`;
      try { await dbMarkDirty(tableId, slot, thumbName); } catch (e) { log("[drop]", `mark thumb dirty ${e}`); }
    }
    await refreshDirtyTables();
    // Make the dropped video show up in the Files panel and become the
    // active video. User can still click another radio to override.
    if (!videoFiles.includes(filename)) videoFiles = [...videoFiles, filename];
    activeVideoFile = filename;
    // Promote AND persist — same reasoning as ingestImage.
    activeFile = filename;
    setActiveSaved(tableId, filename);
    // Invalidate the detailCache entry so navigating away and back
    // re-reads the file list from DB (otherwise the stale cached
    // imageFiles/videoFiles hide the new drop).
    detailCache.delete(tableId);
    videoDataUrl = "";  // force reload from new bytes
    dropStatus = "Pushing to cabinet…";
    await syncPushDirty(sshHost(), cacheDir());
    await refreshDirtyTables();
    await refreshDropVersions(tableId, folder, slot, filename);
    // Update the parent table's slot inventory + winner so the left-side
    // list shows the new video badge without a full reload. Without this
    // patch the file IS on the Pi but tables[].has is stale and the list
    // doesn't re-render until next launch.
    const t = tables.find(x => x.id === tableId);
    if (t) {
      const newHas = new Set(t.has);
      newHas.add("video");
      t.has = newHas;
      t.winner = pickWinner(newHas);
      tables = [...tables];  // new array ref → triggers list re-render
    }
    dropStatus = `Done — ${filename} pushed`;
  }

  async function handleDroppedFiles(paths: string[]) {
    dropError = "";
    if (!selected || !selected.folder || selected.id === undefined) {
      dropError = "Select a table first."; return;
    }
    if (paths.length === 0) return;
    if (paths.length > 1) {
      dropError = "Drop one file at a time."; return;
    }
    const path = paths[0];
    const kind = classifyDropExt(path);
    if (kind === "unsupported") {
      dropError = `Unsupported file type: ${path.split(".").pop()}`; return;
    }
    dropBusy = true;
    try {
      if (kind === "image")          await ingestImage(path, selected.id, selected.folder);
      else if (kind === "directb2s") await ingestDirectb2s(path, selected.id, selected.folder);
      else                           await ingestVideo(path, selected.id, selected.folder);
    } catch (e) {
      dropError = `${e}`;
      log("[drop]", `error ${e}`);
    } finally {
      dropBusy = false;
    }
  }

  // Refresh the per-table version list when selection or thumb file changes.
  $effect(() => {
    const id = selected?.id;
    const folder = selected?.folder;
    if (id === undefined || id === null || !folder) {
      dropVersions = [];
      dropVersionsFor = { tableId: null, slot: "", filename: "" };
      return;
    }
    // Pick the slot/filename based on what's actively shown:
    //   - video active → versions of backglass.mp4 in default_video
    //   - else (image / b2s thumb) → versions of backglass.jpg in default_image
    const slot = showingVideo ? "default_video" : "default_image";
    const filename = showingVideo ? "backglass.mp4" : "backglass.jpg";
    refreshDropVersions(id, folder, slot, filename);
  });

  // Reset-to-b2s-default button state — clears user-dropped media from the
  // local mirror AND the Pi, preserves b2s assets.
  let resetBusy = $state(false);
  async function resetThisTableToB2s() {
    const id = selected?.id;
    const folder = selected?.folder;
    if (id === undefined || id === null || !folder) return;
    const dropCount = userImages.length + userVideos.length;
    if (!confirm(
      `Delete ${dropCount} dropped file(s) from this table on both PP Doctor and the Pi?\n\n` +
      `Kept: b2scache, event_map, thumb, glow.\n` +
      `Deleted: backglass.jpg/png/mp4/etc. + .versions/ backups.\n\n` +
      `This cannot be undone.`
    )) return;
    resetBusy = true;
    try {
      const { resetToB2sDefault, dbDeleteMedia } = await import("$lib/api");
      const removed = await resetToB2sDefault(sshHost(), folder, cacheDir());
      log("[reset]", `table=${id} removed ${removed.length} entries`);
      // Delete the DB rows for the removed files (not just clear their dirty
      // flag — else navigating away and back re-reads the rows and the files
      // reappear) + invalidate the in-memory detailCache for the same reason.
      for (const fname of [...imageFiles, ...videoFiles]) {
        const slot = imageFiles.includes(fname) ? "default_image" : "default_video";
        if (/\.(jpg|jpeg|png|webp|gif|bgra|bmp|mp4|webm|mkv|mov|m4v)$/i.test(fname)) {
          try { await dbDeleteMedia(id, slot, fname); } catch {}
        }
      }
      detailCache.delete(id);
      // Wipe in-memory file lists, reset active to b2s
      imageFiles = imageFiles.filter(f => !/\.(jpg|jpeg|png|webp|gif|bgra|bmp)$/i.test(f));
      videoFiles = videoFiles.filter(f => !/\.(mp4|webm|mkv|mov|m4v)$/i.test(f));
      activeImageFile = null;
      activeVideoFile = null;
      activeFile = selected?.has?.has("b2s") ? "__b2s__" : null;
      setActiveSaved(id, activeFile ?? "");
      imageDataUrl = ""; videoDataUrl = "";
      bloomSourceUrl = ""; bloomSourceImg = null;
      dropVersions = [];
      dropStatus = `Reset complete — ${removed.length} entries removed`;
      await refreshDirtyTables();
    } catch (e) {
      dropError = `Reset failed: ${e}`;
      log("[reset]", `error ${e}`);
    } finally {
      resetBusy = false;
    }
  }

  let unlistenSync: any = null;
  let unlistenDrop: any = null;
  let cacheEnabled = $state(localStorage.getItem("ppe.cache-enabled") === "true");

  // Vite HMR dispose — fires BEFORE the module is replaced. Without this,
  // every HMR cycle stacks a new Tauri onDragDropEvent + sync listener
  // and a setInterval, while the previous ones stay attached. Verified
  // 2026-05-27: a single physical drop fired N times with stale `selected`
  // captured per listener, marking N tables' files dirty.
  if (typeof import.meta !== "undefined" && (import.meta as any).hot) {
    (import.meta as any).hot.dispose(() => {
      try {
        const d = (window as any).__ppe_unlistenDrop; if (typeof d === "function") d();
        (window as any).__ppe_unlistenDrop = null;
      } catch {}
      try {
        const s = (window as any).__ppe_unlistenSync; if (typeof s === "function") s();
        (window as any).__ppe_unlistenSync = null;
      } catch {}
      try {
        const r = (window as any).__ppe_offlineRetryHandle; if (r !== undefined) clearInterval(r);
        (window as any).__ppe_offlineRetryHandle = undefined;
      } catch {}
    });
  }
  /** True while a sync is actively transferring — used to skip the SSH
   *  fallback in loadDetail so table clicks stay instant during sync. */
  let isSyncing = $state(false);

  onMount(async () => {
    if (!ip) return;
    log("[init]", `onMount START ip=${ip}`);
    const tInit = performance.now();

    // ── FAST PATH: render from cached DB immediately so the user sees the
    //    tables list without waiting for the Pi roundtrip. The meta scan
    //    runs in BACKGROUND after, refreshing rows as it goes.
    try {
      await dbOpen(ip);
      const { dbGetTables, dbGetAllMedia } = await import("$lib/api");
      const cachedDbTables = await dbGetTables();
      if (cachedDbTables.length > 0) {
        // Bulk-fetch media rows in ONE IPC call (was 233 individual calls
        // × 130ms = 30s before, now ~50ms total — 2026-05-26).
        const allMedia = await dbGetAllMedia();
        const mediaByTable = new Map<number, typeof allMedia>();
        for (const r of allMedia) {
          const arr = mediaByTable.get(r.table_id);
          if (arr) arr.push(r); else mediaByTable.set(r.table_id, [r]);
        }
        const cachedTables: Table[] = cachedDbTables.map(t => ({
          id: t.id,
          name: t.name,
          folder: t.pi_folder ?? "",
          has: new Set<Slot>(),  // re-derived after meta scan
          winner: "none" as Slot,
          localDirectb2s: null,  // will populate during meta scan
        }));
        // Derive has/winner from in-memory media map — no per-table IPC.
        for (const t of cachedTables) {
          if (!t.folder) continue;
          const rows = mediaByTable.get(t.id) ?? [];
          for (const r of rows) {
            if (r.slot === "default_image") {
              if (r.filename === "backglass.b2scache" || r.filename === "backglass.directb2s") {
                t.has.add("b2s");
              } else {
                const slot = extToSlot(r.filename);
                if (slot && !r.filename.endsWith(".thumb.jpg")) t.has.add(slot);
              }
            } else if (r.slot === "default_video") {
              if (/\.(mp4|webm|mkv|mov|avi|ts|m4v)$/i.test(r.filename)) t.has.add("video");
            }
            if (r.local_size !== null) {
              syncedFiles.add(syncKey(r.table_id, r.slot, r.filename));
            }
          }
          t.winner = pickWinner(t.has);
        }
        syncedFiles = new Set(syncedFiles);
        tables = cachedTables;
        // Initial seed only — leave existing selection alone if onMount
        // somehow re-fires (HMR, route re-entry).
        if (selectedId === null || !tables.some(t => t.id === selectedId)) {
          const first = tables.find(t => t.folder);
          if (first) selectedId = first.id;
        }
        loading = false;
        log("[init]", `instant render from DB cache: ${tables.length} tables in ${Math.round(performance.now() - tInit)}ms`);
      }
    } catch (e) { log("[init]", `db cache read failed ${e}`); }

    try {

      // Listen for sync events — flip per-file synced badges in real time,
      // but BATCH updates to once per animation frame so we don't rebuild
      // the syncedFiles Set 50× per second during diff-skip (which would
      // freeze the file list re-rendering and block user clicks).
      const { onSyncProgress } = await import("$lib/api");
      const pendingSyncKeys: string[] = [];
      let syncedRaf: number | null = null;
      function flushSyncedKeys() {
        syncedRaf = null;
        if (pendingSyncKeys.length === 0) return;
        const next = new Set(syncedFiles);
        for (const k of pendingSyncKeys) next.add(k);
        pendingSyncKeys.length = 0;
        syncedFiles = next;
      }
      // Debounced dirty-tables refresh — was firing on every "done" event
      // (once per table during sync_pull_all = 233 SQL queries × 50ms =
      // 10-20s of UI churn during a big pull). Now coalesces to one query
      // after the sync settles. The final global "done" still triggers it
      // explicitly (file === "" marks the global termination event).
      let refreshDirtyRaf: ReturnType<typeof setTimeout> | null = null;
      function scheduleDirtyRefresh() {
        if (refreshDirtyRaf !== null) clearTimeout(refreshDirtyRaf);
        refreshDirtyRaf = setTimeout(() => { refreshDirtyRaf = null; refreshDirtyTables(); }, 400);
      }

      // HMR cleanup — same accumulation hazard as the drop listener.
      const priorSync = (window as any).__ppe_unlistenSync as (null | (() => void));
      if (typeof priorSync === "function") {
        try { priorSync(); } catch {}
        (window as any).__ppe_unlistenSync = null;
      }
      unlistenSync = await onSyncProgress((p) => {
        // Track active sync state so loadDetail can skip the SSH fallback.
        // The GLOBAL "done" (file === "") flips isSyncing off; per-table
        // "done" events (sync_pull_table emits its own at each completion)
        // are leaf events — don't let them collapse the global flag mid-run.
        if (p.status === "done") {
          if (!p.file || p.file === "") {
            isSyncing = false;
            // Authoritative final refresh — clear any pending debounce.
            if (refreshDirtyRaf !== null) { clearTimeout(refreshDirtyRaf); refreshDirtyRaf = null; }
            refreshDirtyTables();
          } else {
            // Per-table done: debounce a refresh, don't fire one per table.
            scheduleDirtyRefresh();
          }
        } else if (p.current < p.total) {
          isSyncing = true;
        }

        if (p.status === "synced" && p.table_id !== null && p.slot && p.file) {
          pendingSyncKeys.push(syncKey(p.table_id, p.slot, p.file));
          if (syncedRaf === null) {
            syncedRaf = requestAnimationFrame(flushSyncedKeys);
          }
        }
      });
      (window as any).__ppe_unlistenSync = unlistenSync;

      // Populate the per-row sync-needed indicator.
      refreshDirtyTables();

      // Offline-resilient resync. Drops/saves work locally even when the Pi
      // is unreachable (dbMarkDirty persists the intent regardless of SCP
      // success). This interval polls every 60s: if there's anything dirty
      // AND no explicit sync is already running AND the Pi answers an echo,
      // flush the dirty queue. Stops sweeping once nothing's dirty.
      // Clear the prior interval before scheduling a new one — without this
      // each HMR cycle stacks another timer that all fire concurrently
      // (same hazard as the drop / sync listeners above).
      const priorRetry = (window as any).__ppe_offlineRetryHandle;
      if (priorRetry !== undefined) {
        try { clearInterval(priorRetry); } catch {}
      }
      const offlineRetryHandle = setInterval(async () => {
        try {
          if (isSyncing) return;
          const { dbDirtyCount, sshTest, syncPushDirty } = await import("$lib/api");
          const n = await dbDirtyCount();
          if (n === 0) return;
          const reachable = await sshTest(sshHost());
          if (!reachable) {
            log("[resync]", `${n} dirty file(s) pending; Pi unreachable, will retry`);
            return;
          }
          log("[resync]", `Pi back online — flushing ${n} dirty file(s)`);
          await syncPushDirty(sshHost(), cacheDir());
          await refreshDirtyTables();
        } catch (e) {
          log("[resync]", `error ${e}`);
        }
      }, 60_000);
      // Best-effort cleanup if onMount somehow re-runs.
      (window as any).__ppe_offlineRetryHandle = offlineRetryHandle;

      // Webview drag-drop — fires for files dragged from Explorer onto the
      // PP Doctor window. Tauri gives us absolute host paths directly
      // (HTML5 <input type=file> would give a sandboxed File object with
      // no path). Distinguishes between "enter/over" (just show overlay)
      // and "drop" (actually ingest).
      //
      // CRITICAL: stash + cleanup on window so Vite HMR doesn't accumulate
      // listeners. Without this, every code-change-triggered re-mount adds
      // ANOTHER drag-drop listener — each with stale `selected` captured —
      // and a single physical drop fires N times, each ingesting against a
      // different stale table. Verified 2026-05-27: 4 listeners had piled
      // up and were marking 4 tables' files dirty on a single drop.
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const webview = getCurrentWebview();
        // First: kill any prior listener from a previous HMR cycle.
        const prior = (window as any).__ppe_unlistenDrop as (null | (() => void));
        if (typeof prior === "function") {
          try { prior(); } catch {}
          (window as any).__ppe_unlistenDrop = null;
        }
        unlistenDrop = await webview.onDragDropEvent((e: any) => {
          const t = e.payload?.type;
          if (t === "enter" || t === "over") {
            isDragOver = true;
          } else if (t === "leave") {
            isDragOver = false;
          } else if (t === "drop") {
            isDragOver = false;
            const paths: string[] = e.payload.paths ?? [];
            handleDroppedFiles(paths);
          }
        });
        (window as any).__ppe_unlistenDrop = unlistenDrop;
      } catch (e) {
        log("[drop]", `register error ${e}`);
      }

      // Tray "Snapshot for AI debug" handler — dumps state + screenshot.
      const unlistenSnap = await listen("tray:snapshot-requested", async () => {
        try {
          await takeScreenshot("C:/tmp/ppdoctor_screen.png");
          const state = {
            ts: Date.now(),
            ip,
            cacheEnabled,
            selectedId,
            selectedTable: selected ? {
              id: selected.id, name: selected.name, folder: selected.folder,
              winner: selected.winner, has: Array.from(selected.has),
              localDirectb2s: selected.localDirectb2s,
            } : null,
            view: { glowOn, activeFile, b2sLoading, b2sError, b2sXmlChars: b2sXml.length },
            counts: {
              tables: tables.length,
              imageFiles: imageFiles.length,
              videoFiles: videoFiles.length,
              syncedFiles: syncedFiles.size,
            },
            files: { imageFiles, videoFiles, activeImageFile, activeVideoFile },
            err,
          };
          await writeStateDump("C:/tmp/ppdoctor_state.json", JSON.stringify(state, null, 2));
          log("[snap]", "wrote screenshot + state dump");
        } catch (e) {
          log("[snap]", `error ${e}`);
        }
      });

      // 0. Local b2s gitea clone
      try {
        if (await localPathExists(LOCAL_B2S_ROOT)) {
          const dirs = await listLocalDirs(LOCAL_B2S_ROOT);
          for (const d of dirs) {
            const m = d.match(/^(\d{4})/);
            if (m) localFolderById.set(m[1], d);
          }
          log("[init]", `local b2s clone has ${localFolderById.size} folders`);
        } else {
          log("[init]", `no local b2s clone at ${LOCAL_B2S_ROOT}`);
        }
      } catch (e) { log("[init]", `local scan failed ${e}`); }

      // ── HASH SHORTCIRCUIT ───────────────────────────────────────────────
      // Pi-side: list the full media inventory (size+mtime+path of every
      // file). Hash the listing CLIENT-SIDE (Web Crypto) so the Pi only
      // runs find + sort once. On mismatch we REUSE this listing as the
      // full-scan source below instead of running the same find again
      // (used to take ~30s on cache-miss; now ~15s).
      let cachedListing: string | null = null;
      try {
        const { dbGetSetting } = await import("$lib/api");
        const tHash = performance.now();
        // ONE find — captures the inventory string. Hash is computed
        // locally below; Pi only walks the tree once.
        const findCmd = `cd ${PI_MEDIA} && find . -mindepth 3 -maxdepth 3 -type f \\( -path './[0-9]*/default_image/*' -o -path './[0-9]*/default_video/*' \\) -printf '%s\\t%T@\\t%P\\n' 2>/dev/null | sort`;
        const findResult = await sshRun(ip, findCmd);
        const listing = findResult.ok ? findResult.stdout : "";
        let remoteHash = "";
        if (listing) {
          const buf = new TextEncoder().encode(listing);
          const h = await crypto.subtle.digest("SHA-256", buf);
          remoteHash = Array.from(new Uint8Array(h)).map(b => b.toString(16).padStart(2, "0")).join("");
        }
        const storedHash = await dbGetSetting("media_scan_hash");
        log("[init]", `media list+hash took ${Math.round(performance.now() - tHash)}ms remote=${remoteHash.slice(0,8)} stored=${(storedHash||"").slice(0,8)}`);

        if (remoteHash && remoteHash === storedHash && tables.length > 0) {
          // Cache hit — DB already loaded at top of onMount. Just patch in
          // localDirectb2s from the local gitea clone (the only field not
          // already in the DB rows).
          for (const t of tables) {
            const padded = String(t.id).padStart(4, "0");
            const localFolder = localFolderById.get(padded);
            t.localDirectb2s = localFolder
              ? `${LOCAL_B2S_ROOT}/${localFolder}/backglass.directb2s`
              : null;
          }
          tables = tables;  // trigger reactivity
          log("[init]", `onMount HASH-SHORTCIRCUIT tables=${tables.length} took ${Math.round(performance.now() - tInit)}ms`);

          // Intentionally NO auto-mirror here. Pi-side hash matches stored, so
          // every file the Pi has is already either in the cache (and matches
          // size+mtime) or genuinely missing from the cache for reasons SCP
          // can't fix on its own. Running the mirror would walk 695 files for
          // no reason. User can press the status-bar "Pull from cabinet" if
          // they actually want to backfill missing files.
          sessionStorage.setItem("ppe.auto-mirror-done", "1");
          return;  // skip the full scan path entirely
        }
        // Mismatch (or first run) — fall through to full scan, store hash at end.
        (globalThis as any).__ppd_pending_hash = remoteHash;
        cachedListing = listing;  // reuse in the full-scan path below — skip the duplicate find
      } catch (e) { log("[init]", `hash probe failed ${e}`); }

      // 1. Tables list
      const txt = await sshCatText(ip, "/home/pi/PinnerPi/pinball_tables.json");
      const json = JSON.parse(txt);
      const arr: any[] = Array.isArray(json) ? json : (json.tables ?? []);

      // 2. All files under media/<NNNN_*>/default_(image|video)/* in one query.
      //    Event folders (smack, jackpot, multiball, etc.) are intentionally
      //    NOT synced — they're cabinet-side game-event media.
      //    Thumbs and glow caches ARE included so they can be force-regenerated
      //    locally later.
      //    Output:   <size>\t<mtime_epoch>\t<folder>/<sub>/<file>
      //    Note: must start find from '.' so %P keeps the table folder in the
      //    path — using the subfolders as starting points strips it.
      // Reuse the inventory captured in the hash phase above when available
      // (saves ~15-30s of duplicate Pi-side find I/O on cache-miss). Fall
      // back to running find again if the hash phase didn't run / failed.
      const mediaScan = cachedListing !== null
        ? { ok: true as const, stdout: cachedListing, stderr: "" }
        : await sshRun(
            ip,
            `cd ${PI_MEDIA} && find . -mindepth 3 -maxdepth 3 -type f \\( -path './[0-9]*/default_image/*' -o -path './[0-9]*/default_video/*' \\) -printf '%s\\t%T@\\t%P\\n' 2>/dev/null`
          );
      const fileLines = mediaScan.ok
        ? mediaScan.stdout.split("\n").map(s => s.trim()).filter(Boolean)
        : [];

      // Parse: { folder, sub, fname, size, mtime }
      const parsedFiles: { folder: string; sub: string; fname: string; size: number; mtime: number }[] = [];
      for (const line of fileLines) {
        const parts = line.split("\t");
        if (parts.length < 3) continue;
        const size = parseInt(parts[0], 10) || 0;
        const mtime = Math.floor(parseFloat(parts[1]) || 0);
        const rel = parts[2];
        const segs = rel.split("/");
        if (segs.length < 3) continue;
        parsedFiles.push({
          folder: segs[0],
          sub: segs[1],       // default_image | default_video
          fname: segs[segs.length - 1],
          size, mtime
        });
      }

      // Build folder → {imgs, vids, has-b2s, has-bgra/png/jpg/...} map
      const slotMap = new Map<string, Set<Slot>>();
      const directb2sSet = new Set<string>();
      for (const pf of parsedFiles) {
        const folder = pf.folder;
        const sub = pf.sub;        // default_image | default_video
        const fname = pf.fname;

        if (!slotMap.has(folder)) slotMap.set(folder, new Set());
        const has = slotMap.get(folder)!;

        if (sub === "default_video") {
          // any file in default_video counts as video presence for our purposes
          if (/\.(mp4|webm|mkv|mov|avi|ts|m4v)$/i.test(fname)) has.add("video");
        } else if (sub === "default_image") {
          // B2S presence — only the source / cache files count. The thumb is
          // a derived cache artifact and shouldn't qualify on its own.
          if (fname === "backglass.b2scache" || fname === "backglass.directb2s") {
            has.add("b2s");
            if (fname === "backglass.directb2s") directb2sSet.add(folder);
          } else if (fname.endsWith(".thumb.jpg")) {
            // skip cache thumbs from slot detection
          } else {
            const slot = extToSlot(fname);
            if (slot) has.add(slot);
          }
        }
      }

      // 3. Pi-side folder list (to map id → folder name)
      const folderListResult = await sshRun(
        ip,
        `find ${PI_MEDIA} -maxdepth 1 -type d -name '[0-9]*' -printf '%f\\n' 2>/dev/null | sort`
      );
      const folderDirs = folderListResult.ok
        ? folderListResult.stdout.split("\n").map(s => s.trim()).filter(Boolean)
        : [];

      // 4. Build typed table list
      tables = arr
        .filter(t => typeof t.id === "number" && t.name)
        .map(t => {
          const padded = String(t.id).padStart(4, "0");
          const folder = folderDirs.find(d => d.startsWith(padded)) ?? "";
          const has = slotMap.get(folder) ?? new Set<Slot>();
          const localFolder = localFolderById.get(padded);
          const localDirectb2s = localFolder
            ? `${LOCAL_B2S_ROOT}/${localFolder}/backglass.directb2s`
            : null;
          return {
            id: t.id,
            name: String(t.name),
            folder,
            has,
            winner: pickWinner(has),
            localDirectb2s
          };
        })
        .sort((a, b) => a.id - b.id);

      // Only seed selectedId on first load — if the user already navigated
      // to a table during the scan/sync, preserve their selection. Without
      // this guard the meta scan finishing mid-sync would reset the user
      // back to table 0 every time. Also revalidate: if the previously
      // selected table no longer exists (e.g. removed from pinball_tables.json),
      // fall through to the first available.
      if (selectedId === null || !tables.some(t => t.id === selectedId)) {
        const first = tables.find(t => t.folder);
        if (first) selectedId = first.id;
      }

      // Hydrate activeFileMap from cached active.json so left-column badges
      // reflect the Pi's actual state, not stale localStorage. Runs in the
      // background — UI shows winner-priority badges first, then upgrades
      // as each table's active.json is read from the local cache. Reactive
      // via the activeFileMap reassignment so badges refresh in place.
      void (async () => {
        const updates: Record<number, string> = {};
        // Parallelize but cap concurrency so the IPC channel doesn't choke.
        const POOL = 16;
        let cursor = 0;
        async function worker() {
          while (cursor < tables.length) {
            const t = tables[cursor++];
            if (!t.folder) continue;
            // Video active.json takes precedence — that's the same priority
            // the renderer applies when picking a winner.
            try {
              const vCfg = await readActiveConfig(t.folder, "default_video");
              if (vCfg.video) { updates[t.id] = vCfg.video; continue; }
            } catch {}
            try {
              const iCfg = await readActiveConfig(t.folder, "default_image");
              if (iCfg.directb2s) { updates[t.id] = "__b2s__"; continue; }
              if (iCfg.image)     { updates[t.id] = iCfg.image; continue; }
            } catch {}
          }
        }
        await Promise.all(Array.from({ length: POOL }, worker));
        // Single reassignment triggers reactivity once instead of N times.
        // Merge with existing in-memory choices so a freshly-clicked radio
        // (post-scan, in-memory) isn't clobbered by a stale cache read.
        activeFileMap = { ...updates, ...activeFileMap };
        log("[active/hydrate]", `synced ${Object.keys(updates).length} table active.json values from cache`);
      })();

      // Persist to SQLite so next session loads instantly (and so the sync
      // engine has a media_files inventory to walk).
      try {
        const nowSec = Math.floor(Date.now() / 1000);
        await dbUpsertTables(tables.map(t => ({
          id: t.id,
          name: t.name,
          pi_folder: t.folder || null,
          local_folder: t.localDirectb2s ? t.localDirectb2s.replace(/\/backglass\.directb2s$/, "") : null,
          last_synced_ts: nowSec
        })));
        // Write media_files rows for each table — WITH pi_size + pi_mtime
        // so the sync engine can diff-skip unchanged files.
        let mediaWritten = 0;
        for (const t of tables) {
          if (!t.folder) continue;
          const rows: DbMediaFile[] = parsedFiles
            .filter(pf => pf.folder === t.folder)
            .map(pf => ({
              table_id: t.id,
              slot: pf.sub,
              filename: pf.fname,
              pi_size: pf.size,
              pi_mtime: pf.mtime,
              local_size: null, local_mtime: null,
              dirty: false
            }));
          if (rows.length) {
            await dbReplaceMedia(t.id, rows);
            mediaWritten += rows.length;
          }
        }
        log("[init]", `db wrote tables=${tables.length} media=${mediaWritten}`);
      } catch (e) {
        log("[init]", `db persist failed ${e}`);
      }

      // Seed synced-files set from DB (files with non-null local_size)
      try {
        const { dbGetMedia } = await import("$lib/api");
        for (const t of tables) {
          const rows = await dbGetMedia(t.id);
          for (const r of rows) {
            if (r.local_size !== null) {
              syncedFiles.add(syncKey(r.table_id, r.slot, r.filename));
            }
          }
        }
        syncedFiles = new Set(syncedFiles);
      } catch (e) { log("[init]", `db read synced failed ${e}`); }

      // Persist the hash we probed above so the next launch can short-circuit.
      try {
        const pending = (globalThis as any).__ppd_pending_hash as string | undefined;
        if (pending) {
          const { dbSetSetting } = await import("$lib/api");
          await dbSetSetting("media_scan_hash", pending);
          delete (globalThis as any).__ppd_pending_hash;
        }
      } catch (e) { log("[init]", `hash persist failed ${e}`); }

      log("[init]", `onMount DONE tables=${tables.length} took ${Math.round(performance.now() - tInit)}ms`);

      // Auto-mirror DISABLED 2026-05-25 — kicked off a 695-file SCP sync
      // that consistently froze the app while the Rust binary still had the
      // broken ControlMaster scp code. User explicit sync via status-bar
      // "Pull this table" / "Pull all" is the only path that runs SCP now.
      // Re-enable here ONLY after verifying the Rust binary's sync_pull_all
      // can complete in <2 minutes without UI freeze.
      // if (cacheEnabled && !sessionStorage.getItem("ppe.auto-mirror-done")) {
      //   sessionStorage.setItem("ppe.auto-mirror-done", "1");
      //   pullAllTables();
      // }
      log("[init]", "auto-mirror skipped — user-triggered sync only");
    } catch (e) {
      err = String(e);
      log("[init]", `onMount ERROR ${e}`);
    } finally {
      loading = false;
    }
  });

  let filtered = $derived(
    q.trim()
      ? tables.filter(t =>
          t.name.toLowerCase().includes(q.toLowerCase()) ||
          String(t.id).includes(q)
        )
      : tables
  );

  let selected = $derived(tables.find(t => t.id === selectedId) ?? null);

  // Per-table derived values used by the detail-pane template. Computing in
  // $derived (not template {@const}) so they can be referenced directly under
  // a <div> — Svelte 5 restricts {@const} to immediate children of control
  // blocks ({#if}, {#each}, etc.).
  let userImages = $derived(imageFiles.filter(isUserFacing));
  // Also drop the video-slot .directb2s (a video-companion b2s, shown via the
  // B2S row) — but NOT the regular backglass .directb2s in the image slot.
  let userVideos = $derived(
    videoFiles.filter(f => isUserFacing(f) && !f.toLowerCase().endsWith(".directb2s"))
  );
  // showingVideo decides whether the preview pane renders the <video>.
  // Priority:
  //   1. User explicitly picked a video file as active (radio click /
  //      persistent saved choice) → honor it, even on b2s tables
  //   2. Renderer-default winner is "video"
  //   3. Table has only video (no user images) — fallback
  let showingVideo = $derived(
    selected?.folder
      ? (
          // User's explicit pick wins — "__b2s__" sentinel means the user
          // clicked the B2S radio, so the video preview must stand down
          // regardless of selected.winner (which still says "video" for
          // tables like Attack from Mars that default to video). Bug user
          // reported 2026-05-27: choosing the default-image b2s on AfM
          // kept the video playing under the b2s preview.
          activeFile === "__b2s__"
            ? false
            : (
                (activeFile != null && /\.(mp4|webm|mkv|mov|m4v)$/i.test(activeFile)) ||
                selected.winner === "video" ||
                (userImages.length === 0 && userVideos.length > 0)
              )
        )
      : false
  );

  // ── Per-table B2S adjustments ────────────────────────────────────────────
  // All settings stored together as one localStorage entry per table id.
  // Slider/input edits hit `current`; preview reflects live. User commits
  // with Save (writes `current` to `saved` with timestamp) or rolls back
  // with Revert. Save and Revert sit at the bottom of the sidebar so they
  // cover ALL controls in one shot.
  //
  // baseBrightness  — preview-only (no Pi equivalent). Tunes the visible
  //   base bitmap; doesn't affect cabinet.
  // attractMinAlpha / attractMaxAlpha / attractCycleSeconds / attractMotion
  // / attractTail — write to b2s_event_map.json on Save (saveB2SSettings)
  //   and push to the Pi via syncPushDirty. Defaults come from the loaded
  //   event_map; the local override supersedes for preview until Save.
  type B2SSettings = {
    baseBrightness: number;        // 0.3..2.5
    attractMinAlpha: number;       // 0..255
    attractMaxAlpha: number;       // 0..255
    attractCycleSeconds: number;   // 0.5..10
    attractMotion: import("$lib/b2s").AttractMotion;
    attractTail: number;           // 1..12  (RUNNER only)
  };
  const DEFAULT_SETTINGS: B2SSettings = {
    baseBrightness: 1.0,
    attractMinAlpha: 230,         // Pi default (renderer.cpp:2991)
    attractMaxAlpha: 255,
    attractCycleSeconds: 3.0,
    attractMotion: "wave",
    attractTail: 3,
  };
  type SavedB2SSettings = { value: B2SSettings; savedAt: string };

  function settingsKey(id: number): string { return `ppe.b2s-settings.${id}`; }

  // What's CURRENTLY applied to the preview (slider positions).
  let b2sCurrent = $state<B2SSettings>({ ...DEFAULT_SETTINGS });
  // What was last committed (for revert + dirty detection).
  let b2sSaved   = $state<B2SSettings>({ ...DEFAULT_SETTINGS });
  let b2sSavedAt = $state<string | null>(null);

  function loadSavedSettings(id: number, eventMap: any | null): SavedB2SSettings {
    // Detect schema by the presence of per-bulb fields under
    // attract_animation. If present, this is a VIDEO-PAIRED event_map
    // (PP Doctor saved it) and the panel sliders should read from
    // attract_animation.{brightness, min_brightness, speed} — same
    // fields the rAF preview reads, so the slider position round-trips.
    // Otherwise it's an authored b2s where the sliders are layer pulse.
    const a = eventMap?.attract_animation ?? {};
    const isVideoPairedSchema = (
      typeof a.brightness === "number" ||
      typeof a.min_brightness === "number" ||
      typeof a.speed === "number"
    );
    const fromEventMap: B2SSettings = isVideoPairedSchema
      ? {
          ...DEFAULT_SETTINGS,
          attractMaxAlpha:     a.brightness     ?? DEFAULT_SETTINGS.attractMaxAlpha,
          attractMinAlpha:     a.min_brightness ?? DEFAULT_SETTINGS.attractMinAlpha,
          attractCycleSeconds: typeof a.speed === "number"
            ? Math.max(0.1, a.speed / 1000)
            : DEFAULT_SETTINGS.attractCycleSeconds,
          attractMotion: (a.motion ?? DEFAULT_SETTINGS.attractMotion) as B2SSettings["attractMotion"],
          attractTail:   Math.max(1, a.tail ?? DEFAULT_SETTINGS.attractTail),
        }
      : {
          ...DEFAULT_SETTINGS,
          attractMinAlpha:     eventMap?.attract_min_alpha     ?? DEFAULT_SETTINGS.attractMinAlpha,
          attractMaxAlpha:     eventMap?.attract_max_alpha     ?? DEFAULT_SETTINGS.attractMaxAlpha,
          attractCycleSeconds: eventMap?.attract_cycle_seconds ?? DEFAULT_SETTINGS.attractCycleSeconds,
          attractMotion: (a.motion ?? DEFAULT_SETTINGS.attractMotion) as B2SSettings["attractMotion"],
          attractTail:   Math.max(1, a.tail ?? DEFAULT_SETTINGS.attractTail),
        };
    const raw = localStorage.getItem(settingsKey(id));
    if (!raw) {
      // Backward-compat: brightness was previously its own key.
      const oldBright = localStorage.getItem(`ppe.b2s-brightness.${id}`);
      if (oldBright) {
        try {
          const parsed = oldBright.startsWith("{") ? JSON.parse(oldBright) : { value: parseFloat(oldBright), savedAt: "" };
          fromEventMap.baseBrightness = Math.max(0.1, Math.min(3, parsed.value ?? 1.0));
        } catch { /* ignore */ }
      }
      return { value: fromEventMap, savedAt: "" };
    }
    try {
      const parsed = JSON.parse(raw) as SavedB2SSettings;
      return {
        value: { ...fromEventMap, ...parsed.value },
        savedAt: parsed.savedAt ?? "",
      };
    } catch {
      return { value: fromEventMap, savedAt: "" };
    }
  }

  // Reload settings when selection (or event_map) changes.
  $effect(() => {
    const id = selected?.id;
    if (id === undefined || id === null) {
      b2sCurrent = { ...DEFAULT_SETTINGS };
      b2sSaved   = { ...DEFAULT_SETTINGS };
      b2sSavedAt = null;
      return;
    }
    let em: any = null;
    try { em = b2sEventMapJson ? JSON.parse(b2sEventMapJson) : null; } catch { em = null; }
    const s = loadSavedSettings(id, em);
    b2sCurrent = { ...s.value };
    b2sSaved   = { ...s.value };
    b2sSavedAt = s.savedAt || null;
  });

  let b2sSettingsDirty = $derived(
    Math.abs(b2sCurrent.baseBrightness      - b2sSaved.baseBrightness)      > 1e-4 ||
    b2sCurrent.attractMinAlpha             !== b2sSaved.attractMinAlpha     ||
    b2sCurrent.attractMaxAlpha             !== b2sSaved.attractMaxAlpha     ||
    Math.abs(b2sCurrent.attractCycleSeconds - b2sSaved.attractCycleSeconds) > 1e-4 ||
    b2sCurrent.attractMotion               !== b2sSaved.attractMotion       ||
    b2sCurrent.attractTail                 !== b2sSaved.attractTail
  );

  /** Save flow:
   *  1. Snapshot to localStorage (for revert + dirty detection).
   *  2. Patch the mirror's b2s_event_map.json so the on-disk authored
   *     defaults match what we just saved.
   *  3. Mark the file dirty in the DB and trigger a push to the Pi.
   *  baseBrightness is preview-only (no Pi equivalent) and never written
   *  to event_map. */
  async function saveB2SSettings() {
    const id = selected?.id;
    const folder = selected?.folder;
    if (id === undefined || id === null || !folder) return;

    const savedAt = new Date().toISOString();
    const entry: SavedB2SSettings = { value: { ...b2sCurrent }, savedAt };
    localStorage.setItem(settingsKey(id), JSON.stringify(entry));
    b2sSaved   = { ...b2sCurrent };
    b2sSavedAt = savedAt;

    try {
      const { cacheWriteText, cacheGetBinary, dbMarkDirty, syncPushDirty } = await import("$lib/api");
      // Determine target file strictly from what's actively selected:
      //   - activeFile == "__b2s__" (b2s active) → write to
      //     default_image/b2s_event_map.json (drives the authored b2s).
      //   - activeFile is a video AND a paired <stem>.directb2s exists
      //     in default_video/ → write to default_video/<stem>_event_map.json
      //     (drives the generated PP overlay).
      //   - Any other case (video active but no paired b2s, image
      //     active, nothing selected) → ABORT. Don't silently update
      //     the authored b2s when the user is looking at something else.
      let targetSlot: string | null = null;
      let targetFile: string | null = null;
      if (activeFile === "__b2s__" || (activeFile && activeFile.toLowerCase().endsWith(".directb2s"))) {
        // Either the b2s sentinel OR a real .directb2s filename selected
        // from the Files panel (post-Phase-2 active.json picks). Both
        // drive the same default_image/b2s_event_map.json — that's the
        // event-map the Pi loads for whichever .directb2s active.json
        // currently points at.
        targetSlot = "default_image";
        targetFile = "b2s_event_map.json";
      } else if (activeFile && /\.(mp4|webm|mkv|mov|m4v)$/i.test(activeFile)) {
        const dot = activeFile.lastIndexOf(".");
        const videoStem = dot > 0 ? activeFile.slice(0, dot) : activeFile;
        // Check that the paired b2s exists before writing the event_map.
        // Without a .directb2s to pair with, the event_map is orphaned and
        // the Pi has nothing to apply it to.
        let pairedExists = false;
        try {
          const bytes = await cacheGetBinary(sshHost(), folder, "default_video", `${videoStem}.directb2s`, cacheDir());
          pairedExists = !!bytes && bytes.length > 0;
        } catch { pairedExists = false; }
        if (pairedExists) {
          targetSlot = "default_video";
          targetFile = `${videoStem}_event_map.json`;
        } else {
          log("[b2s/save]", `aborting: video '${activeFile}' has no paired .directb2s — generate one first`);
          // Surface to user; uses the existing dropError toast lane.
          dropError = `Can't save B2S settings: ${activeFile} has no paired .directb2s. Use "Edit B2S" first to author lamps + Save.`;
          return;
        }
      } else {
        log("[b2s/save]", `aborting: activeFile=${activeFile} has no b2s target`);
        return;
      }
      // Start from whatever the mirror currently holds (preserves keys we
      // don't manage here — bulb metadata, sprite hints, etc.).
      let em: any = {};
      try { em = b2sEventMapJson ? JSON.parse(b2sEventMapJson) : {}; } catch { em = {}; }
      em.attract_animation = em.attract_animation ?? {};
      em.attract_animation.motion = b2sCurrent.attractMotion;
      em.attract_animation.tail   = b2sCurrent.attractTail;

      if (targetSlot === "default_video") {
        // VIDEO-PAIRED b2s: PP Doctor's panel sliders drive PER-BULB
        // motion (matches the preview rAF loop which builds AttractSpec
        // directly from these). So min/max/cycle live under
        // attract_animation.{min_brightness, brightness, speed} — what
        // b2s.ts::bulbAlpha + Pi's b2s_motion.cpp actually read for per-
        // bulb math. Layer-pulse stays at Pi defaults (230/255/3.0s).
        em.attract_animation.brightness     = b2sCurrent.attractMaxAlpha;
        em.attract_animation.min_brightness = b2sCurrent.attractMinAlpha;
        em.attract_animation.speed          = Math.round(b2sCurrent.attractCycleSeconds * 1000);
        // Preserve / set sensible layer-pulse defaults (Pi renderer.cpp:2991).
        em.attract_min_alpha     = em.attract_min_alpha     ?? 230;
        em.attract_max_alpha     = em.attract_max_alpha     ?? 255;
        em.attract_cycle_seconds = em.attract_cycle_seconds ?? 3.0;
        // Ensure lamps array exists — Pi needs to know which bulbs are in
        // the motion set. For a video-paired b2s, every authored bulb
        // participates; default to a synthesized 1..N list if the user
        // hasn't curated one. The .directb2s itself enumerates the bulbs
        // by RomID 1..N when we wrote it via buildDirectB2sXml.
        if (!Array.isArray(em.attract_animation.lamps) || em.attract_animation.lamps.length === 0) {
          const count = genB2sBulbs.filter(b => b.kept && b.mask).length;
          em.attract_animation.lamps = Array.from({ length: count }, (_, i) => i + 1);
        }
      } else {
        // AUTHORED b2s (default_image): the sliders are the LAYER-PULSE
        // params Pi already applies to the whole FxB layer. attract_
        // animation.{motion, tail} from above is the only per-bulb tweak.
        em.attract_min_alpha     = b2sCurrent.attractMinAlpha;
        em.attract_max_alpha     = b2sCurrent.attractMaxAlpha;
        em.attract_cycle_seconds = b2sCurrent.attractCycleSeconds;
      }

      const json = JSON.stringify(em, null, 2);
      await cacheWriteText(sshHost(), folder, targetSlot, targetFile, json, cacheDir());
      // Only update in-memory b2sEventMapJson when writing to the
      // authored-image path — the generated-video path is a separate
      // event_map that doesn't drive the existing B2SCanvas preview.
      if (targetSlot === "default_image") b2sEventMapJson = json;

      await dbMarkDirty(id, targetSlot, targetFile);
      await refreshDirtyTables();

      // Push immediately. syncPushDirty pushes all dirty rows; per-row scope
      // would need a new Rust command. In practice the user just hit Save,
      // they expect their change on the Pi without an extra click.
      await syncPushDirty(sshHost(), cacheDir());
      await refreshDirtyTables();
      log("[b2s/save]", `pushed ${targetSlot}/${targetFile} table=${id} folder=${folder}`);
    } catch (e) {
      log("[b2s/save]", `push failed: ${e}`);
    }
  }
  function revertB2SSettings() {
    b2sCurrent = { ...b2sSaved };
  }
  function formatSavedAt(iso: string | null): string {
    if (!iso) return "never";
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    const yyyy = d.getFullYear();
    const mm = String(d.getMonth() + 1).padStart(2, "0");
    const dd = String(d.getDate()).padStart(2, "0");
    const hh = String(d.getHours()).padStart(2, "0");
    const min = String(d.getMinutes()).padStart(2, "0");
    return `${yyyy}-${mm}-${dd} ${hh}:${min}`;
  }

  // Convenience accessor used by the existing canvas brightness prop.
  let baseBrightness = $derived(b2sCurrent.baseBrightness);

  // ── Per-table IMAGE-bloom adjustments (only when previewing a still
  // image — JPG/PNG/WEBP/GIF/BGRA). Mirrors the B2S lamp-bloom shape so the
  // image acts as one big "sprite": bright pixels above `threshold` are
  // blurred and additively blended back over the base; the blend alpha is
  // animated by `motion` (wave/flash/strobe/random/all_on) between
  // min/max alpha at `cycleSeconds`. Preview-only — never pushed to Pi
  // (the source artwork is byte-identical on the cabinet's media partition). -->
  type BloomMotion =
    | "wave" | "flash" | "strobe" | "random" | "all_on"
    | "runner" | "sweep" | "ripple";
  type ImageBloom = {
    baseBrightness: number;        // 0.3..2.5 — CSS brightness on base draw
    threshold: number;             // 0..1 — luminance cutoff for glow source
    minAlpha: number;              // 0..255 — bloom layer floor
    maxAlpha: number;              // 0..255 — bloom layer peak
    cycleSeconds: number;          // 0.5..10 — pulse period
    motion: BloomMotion;
  };
  const DEFAULT_IMAGE_BLOOM: ImageBloom = {
    baseBrightness: 1.0,
    threshold: 0.55,
    minAlpha: 60,
    maxAlpha: 200,
    cycleSeconds: 3.0,
    motion: "wave",
  };
  // Bloom halo blur radius is fixed; not user-tunable since the B2S
  // equivalent doesn't expose one either. Bump here if needed.
  const BLOOM_RADIUS_PX = 12;
  type SavedImageBloom = { value: ImageBloom; savedAt: string };
  function imageBloomKey(id: number): string { return `ppe.image-bloom.${id}`; }
  let imgCurrent = $state<ImageBloom>({ ...DEFAULT_IMAGE_BLOOM });
  let imgSaved   = $state<ImageBloom>({ ...DEFAULT_IMAGE_BLOOM });
  let imgSavedAt = $state<string | null>(null);
  function loadSavedImageBloom(id: number): SavedImageBloom {
    const raw = localStorage.getItem(imageBloomKey(id));
    if (!raw) return { value: { ...DEFAULT_IMAGE_BLOOM }, savedAt: "" };
    try {
      const parsed = JSON.parse(raw) as SavedImageBloom;
      return { value: { ...DEFAULT_IMAGE_BLOOM, ...parsed.value }, savedAt: parsed.savedAt ?? "" };
    } catch { return { value: { ...DEFAULT_IMAGE_BLOOM }, savedAt: "" }; }
  }
  $effect(() => {
    const id = selected?.id;
    if (id === undefined || id === null) {
      imgCurrent = { ...DEFAULT_IMAGE_BLOOM };
      imgSaved   = { ...DEFAULT_IMAGE_BLOOM };
      imgSavedAt = null;
      return;
    }
    const s = loadSavedImageBloom(id);
    imgCurrent = { ...s.value };
    imgSaved   = { ...s.value };
    imgSavedAt = s.savedAt || null;
  });
  let imageBloomDirty = $derived(
    Math.abs(imgCurrent.baseBrightness - imgSaved.baseBrightness) > 1e-4 ||
    Math.abs(imgCurrent.threshold      - imgSaved.threshold)      > 1e-4 ||
    imgCurrent.minAlpha               !== imgSaved.minAlpha       ||
    imgCurrent.maxAlpha               !== imgSaved.maxAlpha       ||
    Math.abs(imgCurrent.cycleSeconds   - imgSaved.cycleSeconds)   > 1e-4 ||
    imgCurrent.motion                 !== imgSaved.motion
  );
  function saveImageBloom() {
    const id = selected?.id;
    if (id === undefined || id === null) return;
    const savedAt = new Date().toISOString();
    localStorage.setItem(imageBloomKey(id), JSON.stringify({ value: { ...imgCurrent }, savedAt }));
    imgSaved   = { ...imgCurrent };
    imgSavedAt = savedAt;
  }
  function revertImageBloom() { imgCurrent = { ...imgSaved }; }

  // ── JPG/PNG bloom render pipeline ────────────────────────────────────────
  // Canvas-based bloom modeled on the B2S FxB layer:
  //   1. Draw base image with CSS brightness filter.
  //   2. Maintain a cached "bright mask" (pixels above `threshold` set to
  //      black for everyone else) — rebuilt only when source or threshold
  //      changes; threshold sweeps are still cheap because the rebuild only
  //      runs once per change.
  //   3. Every frame: additively composite the blurred bright mask back
  //      with alpha modulated by the motion type (wave/flash/strobe/
  //      random/all_on) between minAlpha and maxAlpha.
  let bloomCanvas = $state<HTMLCanvasElement | null>(null);
  let bloomSourceImg: HTMLImageElement | null = null;
  let bloomSourceUrl: string = "";
  let bloomBrightMask: ImageBitmap | null = null;
  let bloomMaskThreshold = -1;
  let bloomLoadToken = 0;
  let bloomRaf: number | null = null;
  let bloomStartMs = 0;
  // Cached strobe random hash state (regenerated each strobe slot).
  let bloomStrobeSlot = -1;
  let bloomStrobeOn = false;

  async function rebuildBrightMask(img: HTMLImageElement, threshold: number) {
    const w = img.naturalWidth, h = img.naturalHeight;
    if (!w || !h) return;
    const off = document.createElement("canvas");
    off.width = w; off.height = h;
    const ctx = off.getContext("2d", { willReadFrequently: true })!;
    ctx.drawImage(img, 0, 0);
    const id = ctx.getImageData(0, 0, w, h);
    const d = id.data;
    const thr = threshold * 255;
    for (let i = 0; i < d.length; i += 4) {
      const luma = 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2];
      if (luma < thr) { d[i] = d[i + 1] = d[i + 2] = 0; }
    }
    ctx.putImageData(id, 0, 0);
    if (bloomBrightMask) bloomBrightMask.close();
    bloomBrightMask = await createImageBitmap(off);
    bloomMaskThreshold = threshold;
  }

  /** Compute bloom layer alpha (0..1) given current motion settings + time. */
  function bloomLayerAlpha(nowMs: number): number {
    const minA = Math.max(0, Math.min(255, imgCurrent.minAlpha)) / 255;
    const maxA = Math.max(0, Math.min(255, imgCurrent.maxAlpha)) / 255;
    const span = maxA - minA;
    const elapsed = nowMs - bloomStartMs;
    const cycleMs = Math.max(100, imgCurrent.cycleSeconds * 1000);
    switch (imgCurrent.motion) {
      case "all_on": return maxA;
      case "flash":  return maxA;
      case "strobe": {
        const half = Math.max(50, cycleMs / 2);
        const on = Math.floor(elapsed / half) % 2 === 0;
        return on ? maxA : minA;
      }
      case "random": {
        const slot = Math.floor(elapsed / Math.max(100, cycleMs / 4));
        if (slot !== bloomStrobeSlot) {
          bloomStrobeSlot = slot;
          // PRNG hash on slot — same shape as b2s.ts random motion.
          let h = (slot * 0x9e3779b1) >>> 0;
          h = (h ^ (h >>> 15)) >>> 0;
          bloomStrobeOn = (h & 0xff) < 128;
        }
        return bloomStrobeOn ? maxA : minA;
      }
      // Image bloom has no per-bulb concept, so runner/sweep/ripple all
      // share the wave path. Listed in the dropdown for parity with B2S
      // (and to keep saved configs forward-compatible if we ever add
      // per-pixel directional bloom).
      case "runner":
      case "sweep":
      case "ripple":
      case "wave":
      default: {
        const s = 0.5 + 0.5 * Math.sin((elapsed / cycleMs) * Math.PI * 2);
        return minA + s * span;
      }
    }
  }

  async function ensureBloomReady(): Promise<boolean> {
    const url = imageDataUrl;
    if (!url) return false;
    const token = ++bloomLoadToken;
    if (url !== bloomSourceUrl || !bloomSourceImg) {
      const img = new Image();
      img.src = url;
      try { await img.decode(); } catch { return false; }
      if (token !== bloomLoadToken) return false;
      bloomSourceImg = img;
      bloomSourceUrl = url;
      bloomBrightMask?.close();
      bloomBrightMask = null;
      bloomMaskThreshold = -1;
    }
    if (!bloomBrightMask || Math.abs(bloomMaskThreshold - imgCurrent.threshold) > 1e-4) {
      await rebuildBrightMask(bloomSourceImg!, imgCurrent.threshold);
      if (token !== bloomLoadToken) return false;
    }
    return true;
  }

  function drawBloomFrame(nowMs: number) {
    const canvas = bloomCanvas;
    const img = bloomSourceImg;
    if (!canvas || !img) return;
    const w = img.naturalWidth, h = img.naturalHeight;
    if (!w || !h) return;
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
    const ctx = canvas.getContext("2d")!;
    ctx.globalCompositeOperation = "source-over";
    ctx.globalAlpha = 1;
    ctx.filter = `brightness(${imgCurrent.baseBrightness})`;
    ctx.clearRect(0, 0, w, h);
    ctx.drawImage(img, 0, 0);
    ctx.filter = "none";
    if (!bloomBrightMask) return;
    const a = bloomLayerAlpha(nowMs);
    if (a <= 0.005) return;
    ctx.save();
    ctx.filter = `blur(${BLOOM_RADIUS_PX}px)`;
    ctx.globalCompositeOperation = "lighter";
    ctx.globalAlpha = a;
    ctx.drawImage(bloomBrightMask, 0, 0);
    ctx.restore();
  }

  function startBloomLoop() {
    if (bloomRaf !== null) return;
    bloomStartMs = performance.now();
    bloomStrobeSlot = -1;
    const tick = (t: number) => {
      bloomRaf = null;
      drawBloomFrame(t);
      // Only animate if motion is non-static.
      if (imgCurrent.motion !== "all_on" && imgCurrent.motion !== "flash") {
        bloomRaf = requestAnimationFrame(tick);
      }
    };
    bloomRaf = requestAnimationFrame(tick);
  }

  function stopBloomLoop() {
    if (bloomRaf !== null) { cancelAnimationFrame(bloomRaf); bloomRaf = null; }
  }

  // Drive the pipeline reactively. The canvas ALWAYS shows the base image
  // when imageDataUrl is set — even on b2s tables where the right panel
  // shows "B2S lamp adjustment". Bloom animation only runs when the image
  // panel is active (otherwise: single base draw, no rAF loop).
  $effect(() => {
    void bloomCanvas; void imageDataUrl;
    void imgCurrent.baseBrightness;
    void imgCurrent.threshold;
    void imgCurrent.minAlpha; void imgCurrent.maxAlpha;
    void imgCurrent.cycleSeconds; void imgCurrent.motion;
    void activePanel;

    (async () => {
      stopBloomLoop();
      // Source load is cheap (cached after first decode) and always needed
      // to draw the base. Skip mask rebuild if we're not going to bloom.
      const url = imageDataUrl;
      if (!bloomCanvas || !url) return;
      const wantBloom = activePanel === "image";
      const ok = wantBloom
        ? await ensureBloomReady()
        : await ensureSourceReady();
      if (!ok) return;
      if (wantBloom) {
        startBloomLoop();
      } else {
        // Single base draw — no bloom layer, no animation.
        drawBaseOnly();
      }
    })();
  });

  /** Load (and cache) the source image without doing the bright-mask pass. */
  async function ensureSourceReady(): Promise<boolean> {
    const url = imageDataUrl;
    if (!url) return false;
    const token = ++bloomLoadToken;
    if (url !== bloomSourceUrl || !bloomSourceImg) {
      const img = new Image();
      img.src = url;
      try { await img.decode(); } catch { return false; }
      if (token !== bloomLoadToken) return false;
      bloomSourceImg = img;
      bloomSourceUrl = url;
      bloomBrightMask?.close();
      bloomBrightMask = null;
      bloomMaskThreshold = -1;
    }
    return true;
  }

  function drawBaseOnly() {
    const canvas = bloomCanvas;
    const img = bloomSourceImg;
    if (!canvas || !img) return;
    const w = img.naturalWidth, h = img.naturalHeight;
    if (!w || !h) return;
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
    const ctx = canvas.getContext("2d")!;
    ctx.globalCompositeOperation = "source-over";
    ctx.globalAlpha = 1;
    ctx.filter = `brightness(${imgCurrent.baseBrightness})`;
    ctx.clearRect(0, 0, w, h);
    ctx.drawImage(img, 0, 0);
    ctx.filter = "none";
  }
  // Which side panel to show.
  //  - If the table HAS a b2s, show "B2S lamp adjustment" (regardless of
  //    what's playing — including over video, so the same sliders tune the
  //    overlay/auto-glow once we wire b2s-over-video).
  //  - Pure video table (no b2s authored) → still show "image bloom
  //    adjustment" so the user can tune threshold/min/max/motion against
  //    the live video frames (same controls work).
  //  - Otherwise: real image → "image bloom adjustment"; nothing → none.
  let activePanel = $derived(
    selected?.has.has("b2s") ? "b2s" :
    showingVideo             ? "image" :
    imageDataUrl             ? "image" : "none"
  );

  // Mirror selected table into the shared `selection` state so StatusBar's
  // "Pull this table" button can act on it.
  $effect(() => {
    (async () => {
      const { selection } = await import("$lib/selection.svelte");
      selection.id = selected?.id ?? null;
      selection.name = selected?.name ?? "";
      selection.piFolder = selected?.folder ?? "";
    })();
  });

  $effect(() => {
    if (!selected || !selected.folder) {
      imageFiles = []; videoFiles = [];
      activeImageFile = null; activeVideoFile = null;
      imageDataUrl = ""; videoDataUrl = "";
      return;
    }
    loadDetail(selected.folder, selected.id);
  });

  async function loadDetail(folder: string, tableId: number) {
    const t0 = performance.now();
    // Cache hit — instant swap, no SSH.
    const cached = detailCache.get(tableId);
    if (cached) {
      imageFiles = cached.imageFiles;
      videoFiles = cached.videoFiles;
      activeImageFile = cached.activeImageFile;
      activeVideoFile = cached.activeVideoFile;
      activeFile = pickActiveFile(tableId, selected?.has ?? new Set(), cached.imageFiles, cached.videoFiles);
      imageDataUrl = cached.imageDataUrl;
      videoDataUrl = "";
      detailLoading = false;
      log("[detail]", `cache-hit table=${tableId} folder=${folder} in ${Math.round(performance.now() - t0)}ms`);
      return;
    }
    log("[detail]", `fetch START table=${tableId} folder=${folder}`);
    detailLoading = true;
    try {
      // File lists come from the DB (populated by the meta scan on app launch).
      // No SSH roundtrip — so table-click stays responsive even while sync is
      // saturating the SSH connection with SCP transfers in the background.
      const tList = performance.now();
      const { dbGetMedia } = await import("$lib/api");
      const dbRows = await dbGetMedia(tableId);
      const newImageFiles = dbRows.filter(r => r.slot === "default_image").map(r => r.filename);
      const newVideoFiles = dbRows.filter(r => r.slot === "default_video").map(r => r.filename);
      log("[detail]", `db list-files table=${tableId} (${newImageFiles.length}+${newVideoFiles.length}) took ${Math.round(performance.now() - tList)}ms`);

      // Bail if user clicked a different table during the fetch.
      if (selected?.id !== tableId) { log("[detail]", `bail (user moved on) table=${tableId}`); return; }

      const newActiveImage = pickImageForPreview(newImageFiles);
      const newActiveVideo = newVideoFiles.find(f => /\.(mp4|webm|mkv|mov|avi)$/i.test(f)) ?? null;

      // Commit the file lists now (cheap)
      imageFiles = newImageFiles;
      videoFiles = newVideoFiles;
      activeImageFile = newActiveImage;
      activeVideoFile = newActiveVideo;
      activeFile = pickActiveFile(tableId, selected?.has ?? new Set(), newImageFiles, newVideoFiles);

      // Fetch the image preview ONLY AFTER everything else is committed.
      // Don't clear the stale imageDataUrl — leave it until the new one is ready.
      //
      // CACHE FIRST: if the file is in the local mirror, read it from disk
      // (instant). Only fall back to SSH base64-over-the-wire if not cached.
      let newImageDataUrl = "";
      if (newActiveImage) {
        const tImg = performance.now();
        let b64 = "";
        try {
          b64 = await cacheGetBase64(ip, folder, "default_image", newActiveImage, cacheDir());
          if (b64) {
            log("[detail]", `cache hit image=${newActiveImage} bytes=${b64.length} took ${Math.round(performance.now() - tImg)}ms`);
          }
        } catch { /* cache miss — handled below */ }
        if (!b64) {
          if (isSyncing) {
            // Don't compete with the active sync for the SSH connection.
            // The file is being pulled and will appear in the cache shortly.
            log("[detail]", `cache miss + sync active — leaving preview blank for ${newActiveImage}`);
          } else {
            b64 = await sshGetBase64(ip, `${PI_MEDIA}/${folder}/default_image/${newActiveImage}`);
            log("[detail]", `ssh get-base64 image=${newActiveImage} bytes=${b64.length} took ${Math.round(performance.now() - tImg)}ms (cache miss)`);
          }
        }
        if (selected?.id !== tableId) { log("[detail]", `bail late (user moved on) table=${tableId}`); return; }
        newImageDataUrl = b64 ? dataUrlFor(newActiveImage, b64) : "";
        imageDataUrl = newImageDataUrl;
      } else {
        imageDataUrl = "";
      }
      videoDataUrl = "";

      detailCache.set(tableId, {
        imageFiles: newImageFiles,
        videoFiles: newVideoFiles,
        activeImageFile: newActiveImage,
        activeVideoFile: newActiveVideo,
        imageDataUrl: newImageDataUrl
      });
      log("[detail]", `fetch DONE table=${tableId} total=${Math.round(performance.now() - t0)}ms cached`);
    } catch (e) {
      console.error("loadDetail:", e);
    } finally {
      detailLoading = false;
    }
  }

  function pickImageForPreview(imgs: string[]): string | null {
    // Show what the Pi would actually display, in priority order
    const w = selected?.winner;
    if (w === "bgra") return imgs.find(f => f.toLowerCase().endsWith(".bgra")) ?? null;
    if (w === "png")  return imgs.find(f => f.toLowerCase().endsWith(".png")) ?? null;
    if (w === "jpg")  return imgs.find(f => /\.(jpg|jpeg)$/i.test(f)) ?? null;
    if (w === "webp") return imgs.find(f => f.toLowerCase().endsWith(".webp")) ?? null;
    if (w === "gif")  return imgs.find(f => f.toLowerCase().endsWith(".gif")) ?? null;
    if (w === "b2s") {
      // No image preview for raw b2s yet (would need to render). Use thumb as proxy.
      return imgs.find(f => f === "backglass.b2s_base.thumb.jpg") ?? null;
    }
    return imgs.find(f => /\.(jpg|jpeg|png|webp|bmp|gif)$/i.test(f)) ?? null;
  }

  // Track the in-flight load so we don't fire multiple SSH/cache fetches
  // on rapid table switches or repeated effect runs.
  let videoLoadInFlight = false;
  // Hold the previous Blob URL so we revoke it when assigning a new one
  // (prevents the Chrome internal blob registry from leaking).
  let lastVideoBlobUrl: string | null = null;

  function bytesToVideoBlobUrl(bytes: Uint8Array, filename: string): string {
    const ext = filename.toLowerCase().split(".").pop() ?? "";
    const mime = (
      ext === "mp4"  ? "video/mp4"  :
      ext === "webm" ? "video/webm" :
      ext === "mkv"  ? "video/x-matroska" :
      ext === "mov"  ? "video/quicktime" :
      ext === "m4v"  ? "video/mp4"  :
                       "application/octet-stream"
    );
    const blob = new Blob([bytes], { type: mime });
    const url = URL.createObjectURL(blob);
    if (lastVideoBlobUrl) { try { URL.revokeObjectURL(lastVideoBlobUrl); } catch {} }
    lastVideoBlobUrl = url;
    return url;
  }

  /** Refresh the paired .thumb.jpg URL whenever the active video changes.
   *  Reads via binary IPC (fast cache hit, sub-50ms typical) and wraps as
   *  a blob URL the <video poster> can consume. Failure is silent — the
   *  poster just doesn't render. */
  $effect(() => {
    const folder = selected?.folder;
    const f = activeVideoFile;
    if (!folder || !f) { videoThumbUrl = null; return; }
    const dot = f.lastIndexOf(".");
    const stem = dot > 0 ? f.slice(0, dot) : f;
    const thumbName = `${stem}.thumb.jpg`;
    (async () => {
      try {
        const { cacheGetBinary } = await import("$lib/api");
        const bytes = await cacheGetBinary(sshHost(), folder, "default_video", thumbName, cacheDir());
        if (bytes && bytes.length > 0) {
          const url = URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" }));
          videoThumbUrl = url;
        } else {
          videoThumbUrl = null;
        }
      } catch { videoThumbUrl = null; }
    })();
  });

  async function loadVideo() {
    if (!selected?.folder || !activeVideoFile || videoDataUrl || videoLoadInFlight) return;
    videoLoadInFlight = true;
    try {
      // Prefer local mirror via BINARY IPC. cacheGetBinary returns the file
      // as raw Uint8Array — no base64 / atob / data-URL round-trip. For a
      // 3.6 MB mp4 this is ~50ms vs. ~3-5s for the old base64+data-URL path.
      try {
        const { cacheGetBinary } = await import("$lib/api");
        const bytes = await cacheGetBinary(sshHost(), selected.folder, "default_video", activeVideoFile, cacheDir());
        if (bytes && bytes.length > 0) {
          videoDataUrl = bytesToVideoBlobUrl(bytes, activeVideoFile);
          return;
        }
      } catch (e) {
        log("[video]", `mirror miss/error, falling back to SSH: ${e}`);
      }
      // Fallback: SSH base64 (slow, but works when mirror is empty).
      const b64 = await sshGetBase64(ip, `${PI_MEDIA}/${selected.folder}/default_video/${activeVideoFile}`);
      if (b64) videoDataUrl = dataUrlFor(activeVideoFile, b64);
    } finally {
      videoLoadInFlight = false;
    }
  }

  // Auto-load the video preview when the user has the toggle on AND the
  // current table's default is a video. Fires reactively when activeVideoFile
  // or showingVideo or videoAutoPreview changes.
  $effect(() => {
    if (videoAutoPreview && showingVideo && activeVideoFile && !videoDataUrl) {
      void loadVideo();
    }
  });

  // ─── Generate-B2S-from-video state ─────────────────────────────────────
  // The interactive editor lifecycle:
  //   1. User clicks "Generate B2S from video" → extract max-brightness
  //      composite from active <video>, run scaffold script, parse result
  //   2. genB2sBulbs populates with detected lamp candidates → overlay
  //      renders striped boxes over the video
  //   3. User edits: click a lamp to delete, drag rectangle for "magic wand
  //      add", adjust threshold → re-run scaffold
  //   4. Save → write scaffold .directb2s into table's media folder,
  //      mark dirty, push to Pi (it'll overlay on the video)
  type GenBulb = {
    id: number;          // unique id — used as the auto-label too
    label: string;        // display label (defaults to "L<id>"; user-renamable later)
    x: number; y: number; // pixel coords in 1920×1080 composite space
    w: number; h: number;
    kept: boolean;        // user toggle (now driven via right-click menu only)
    /** "scaffold" = from scaffold_b2s_from_png.py; "wand" = added by user
     *  via magic-wand click. Wand bulbs carry a pixel mask so the save
     *  step can write a lit-sprite PNG matching the irregular blob. */
    source?: "scaffold" | "wand";
    /** Relative-to-bbox pixel mask describing the actual lamp shape.
     *  Wand bulbs always have one. */
    mask?: Uint8Array;
    /** Per-lamp feather radius in px — soft alpha falloff at the edge
     *  applied to the saved lit-sprite alpha channel. 0 = hard edge.
     *  Inherits from the toolbar slider at creation time. */
    feather: number;
    /** RGB color sampled from the composite under the mask (mean of all
     *  filled pixels). Used both for the on-screen preview tint and as
     *  the fill color of the saved .directb2s lit-sprite PNG. Falls
     *  back to amber if sampling yielded no pixels. */
    color: [number, number, number];
  };

  /** Sample the average RGB of composite pixels under a lamp's mask.
   *  Used to color the preview lamp shape and to color the saved
   *  lit-sprite PNG that goes into the .directb2s. */
  function sampleLampColor(
    composite: Uint8ClampedArray, cw: number, ch: number,
    bx: number, by: number, mask: Uint8Array, mw: number, mh: number
  ): [number, number, number] {
    let rs = 0, gs = 0, bs = 0, n = 0;
    for (let yy = 0; yy < mh; yy++) {
      const cy = by + yy;
      if (cy < 0 || cy >= ch) continue;
      for (let xx = 0; xx < mw; xx++) {
        if (!mask[yy * mw + xx]) continue;
        const cx = bx + xx;
        if (cx < 0 || cx >= cw) continue;
        const ci = (cy * cw + cx) * 4;
        rs += composite[ci];
        gs += composite[ci + 1];
        bs += composite[ci + 2];
        n++;
      }
    }
    if (n === 0) return [255, 191, 36];  // amber fallback
    return [Math.round(rs / n), Math.round(gs / n), Math.round(bs / n)];
  }
  let genB2sActive = $state(false);          // editor visible?
  let genB2sBusy = $state(false);
  let genB2sStatus = $state("");
  let genB2sBulbs = $state<GenBulb[]>([]);
  let genB2sCompositePath = $state<string | null>(null);  // temp PNG path for re-runs
  let genB2sOutputPath = $state<string | null>(null);
  let genB2sCompositeW = $state(1920);
  let genB2sCompositeH = $state(1080);
  /** Per-table identity for the b2s-from-video editor — when the user
   *  navigates to a different table, we wipe ALL the gen state so
   *  Black Rose's lamps don't bleed into Champion Pub. The id is the
   *  source of truth for "what table does the current editor belong
   *  to"; if it diverges from selected.id, reset. */
  let genB2sOwnerTableId = $state<number | null>(null);
  $effect(() => {
    const id = selected?.id ?? null;
    if (genB2sOwnerTableId === null) return;     // editor not active for anything
    if (id === genB2sOwnerTableId) return;        // same table — keep state
    // Different table → reset everything to a clean state. The previous
    // table's lamps would otherwise render over the new table's preview
    // with wrong coordinates / wrong sampled colors.
    genB2sActive = false;
    genB2sBulbs = [];
    genB2sStatus = "";
    genB2sCompositePath = null;
    genB2sOutputPath = null;
    genB2sCompositePixels = null;
    genB2sPreviewAnim = false;
    genB2sNextId = 1;
    bulbMenu = null;
    genB2sZoom = 1;
    genB2sOwnerTableId = null;
  });

  /** Auto-load previously-saved paired b2s when the user activates a
   *  video on a table that has one. So if you bounce back to Black Rose
   *  (where you already ran Save & Push), the lamps reappear in preview
   *  mode without having to re-run Generate B2S from video.
   *
   *  Trigger conditions:
   *   - active file is a video (.mp4/.webm/...)
   *   - the editor is NOT already loaded for this table
   *   - <stem>.directb2s exists in default_video
   *
   *  Loads into preview-animation mode by default — clean visualization,
   *  no authoring chrome. User can flip Preview off to enter edit mode. */
  $effect(() => {
    const tableId = selected?.id;
    const folder = selected?.folder;
    const file = activeVideoFile;
    if (tableId === undefined || tableId === null || !folder || !file) return;
    if (genB2sOwnerTableId === tableId && genB2sActive) return;  // already loaded
    if (genB2sBusy || genB2sSaveBusy) return;
    (async () => {
      const dot = file.lastIndexOf(".");
      const stem = dot > 0 ? file.slice(0, dot) : file;
      const directb2sName = `${stem}.directb2s`;
      try {
        const { cacheGetBinary, cacheFilePath } = await import("$lib/api");
        const bytes = await cacheGetBinary(sshHost(), folder, "default_video", directb2sName, cacheDir());
        if (!bytes || bytes.length === 0) return;
        const path = await cacheFilePath(sshHost(), folder, "default_video", directb2sName, cacheDir());
        const parsed = await parseScaffoldDoc(path);
        const bulbs = parsed.bulbs;
        if (bulbs.length === 0) return;
        // Guard against the user navigating away during the async load.
        if (selected?.id !== tableId) return;
        // Adopt source dims from the .directb2s so percent calculations
        // for bulb positions land on the actual art. PP Doctor's own video
        // outputs are 1920×1080, but VPU/scaffold imports may differ.
        if (parsed.sourceW && parsed.sourceH) {
          genB2sCompositeW = parsed.sourceW;
          genB2sCompositeH = parsed.sourceH;
        }
        genB2sBulbs = bulbs;
        genB2sActive = true;
        genB2sOwnerTableId = tableId;
        genB2sPreviewAnim = true;             // start in glowing-preview mode
        genB2sStatus = `Loaded ${bulbs.length} lamp${bulbs.length === 1 ? '' : 's'} from ${directb2sName}`;
        log("[b2s-from-video]", `auto-loaded table=${tableId} ${directb2sName} bulbs=${bulbs.length}`);
      } catch (e) {
        log("[b2s-from-video]", `auto-load skipped: ${e}`);
      }
    })();
  });
  /** Composite pixel data kept in memory so the magic wand can flood-fill
   *  without re-decoding the PNG on every click. RGBA Uint8ClampedArray
   *  in row-major order. Sized to genB2sCompositeW × genB2sCompositeH × 4. */
  let genB2sCompositePixels: Uint8ClampedArray | null = null;
  /** Animation preview toggle — when true, the lamp overlay renders as
   *  pulsing glow filled shapes (CSS keyframe + per-lamp phase delay)
   *  instead of the static marching-ants outlines. Lets the user see how
   *  the generated b2s would animate over the video before saving to Pi. */
  let genB2sPreviewAnim = $state(false);
  /** Preview zoom (1.0 = fit-to-container). Drives inner-wrapper width
   *  so the outer overflow-auto produces scrollbars naturally. Click
   *  coords work unchanged because getBoundingClientRect returns the
   *  zoomed rect. Clamped to [1, 4]; finer scrub via the magic-wand
   *  tolerance is for color, this is for pixel accuracy on small lamps. */
  let genB2sZoom = $state(1);
  function zoomIn()  { genB2sZoom = Math.min(4,   +(genB2sZoom + 0.25).toFixed(2)); }
  function zoomOut() { genB2sZoom = Math.max(1,   +(genB2sZoom - 0.25).toFixed(2)); }
  function zoomFit() { genB2sZoom = 1; }
  /** Per-lamp opacity computed each frame by the b2s attract engine
   *  (bulbAlpha from b2s.ts — same code Pi cabinet runs). Indexed by
   *  bulb.id. Updated in an rAF loop that reads b2sCurrent live, so
   *  dragging the B2S panel sliders updates the preview in real time. */
  let lampAlphas = $state<Map<number, number>>(new Map());
  let lampPreviewRaf: number | null = null;
  let lampPreviewStartTs = 0;
  /** Photoshop-style tolerance: RGB Euclidean distance from the seed pixel.
   *  0 = only exact color match; 255 = everything; 32 default matches
   *  Photoshop's out-of-the-box value. */
  let genB2sWandTolerance = $state(32);
  /** Feather radius in pixels (0-12). Dilates the binary mask and applies
   *  a linear alpha falloff at the edge, producing soft-edged lamps —
   *  same idea as Photoshop's "Feather Selection". Stored per-lamp so
   *  each detection can have its own setting. */
  let genB2sFeather = $state(0);
  let genB2sNextId = 1;

  /** Photoshop-style magic-wand flood-fill: seed pixel's color defines the
   *  selection palette, expand to 4-connected neighbors whose RGB distance
   *  to the seed is within `tolerance`. Returns bbox + relative-to-bbox
   *  shape mask + pixel count.
   *
   *  Different from the SCAFFOLD path's brightness threshold — that one
   *  picks out "anything bright enough", this one picks out "the lamp the
   *  user actually clicked, in the user's chosen color". */
  function magicWandFloodFill(
    pixels: Uint8ClampedArray, w: number, h: number,
    sx: number, sy: number, tolerance: number
  ): { x: number; y: number; w: number; h: number; mask: Uint8Array; pixelCount: number } | null {
    const idx = (x: number, y: number) => (y * w + x) * 4;
    if (sx < 0 || sy < 0 || sx >= w || sy >= h) return null;
    const si = idx(sx, sy);
    const sr = pixels[si], sg = pixels[si + 1], sb = pixels[si + 2];
    // Square the tolerance once so we can compare squared distances and
    // avoid the per-pixel sqrt. Tolerance is in RGB-distance units, max
    // possible distance is sqrt(255²·3) = 441.7 — pre-square covers it.
    const tolSq = tolerance * tolerance;
    const visited = new Uint8Array(w * h);
    const stack: number[] = [sx, sy];
    let minX = sx, minY = sy, maxX = sx, maxY = sy, count = 0;
    while (stack.length) {
      const y = stack.pop()!;
      const x = stack.pop()!;
      if (x < 0 || y < 0 || x >= w || y >= h) continue;
      const vi = y * w + x;
      if (visited[vi]) continue;
      const pi = vi * 4;
      const dr = pixels[pi] - sr;
      const dg = pixels[pi + 1] - sg;
      const db = pixels[pi + 2] - sb;
      // Euclidean RGB distance squared — matches Photoshop's tolerance
      // metric (within rounding). Compare against tolerance² so we skip
      // the sqrt per pixel.
      if (dr * dr + dg * dg + db * db > tolSq) continue;
      visited[vi] = 1;
      count++;
      if (x < minX) minX = x; if (x > maxX) maxX = x;
      if (y < minY) minY = y; if (y > maxY) maxY = y;
      stack.push(x + 1, y); stack.push(x - 1, y);
      stack.push(x, y + 1); stack.push(x, y - 1);
    }
    if (count < 4) return null;  // too small, probably a noisy single pixel
    const bw = maxX - minX + 1, bh = maxY - minY + 1;
    const mask = new Uint8Array(bw * bh);
    for (let yy = 0; yy < bh; yy++) {
      for (let xx = 0; xx < bw; xx++) {
        if (visited[(minY + yy) * w + (minX + xx)]) mask[yy * bw + xx] = 255;
      }
    }
    return { x: minX, y: minY, w: bw, h: bh, mask, pixelCount: count };
  }

  /** Left-click handler on the overlay. Hit-tests the lamp set first:
   *  - Click ON an existing lamp (scaffold OR wand) → open its edit menu
   *    at the click point. Same panel as right-click; lets the user
   *    select+edit detected lamps with a single click.
   *  - Click on empty space → run the magic wand from that pixel,
   *    flood-fill, add as a new wand lamp.
   *  Right-click still works on either path via onOverlayContextMenu. */
  /** Lazy composite extraction. Auto-load (re-loading a saved paired b2s)
   *  doesn't need a composite — it has the sprite masks already. But the
   *  wand needs the composite for flood-fill + color sampling. First
   *  click triggers this; subsequent clicks reuse the cached pixels. */
  async function ensureCompositeForWand(): Promise<boolean> {
    if (genB2sCompositePixels) return true;
    const videoEl = document.querySelector<HTMLVideoElement>("video[src]");
    if (!videoEl || !videoEl.videoWidth) return false;
    genB2sStatus = "Sampling video frames for wand…";
    try {
      const { blob, width, height } = await extractMaxBrightnessComposite(videoEl);
      genB2sCompositeW = width; genB2sCompositeH = height;
      const bmp = await createImageBitmap(blob);
      const c = document.createElement("canvas");
      c.width = width; c.height = height;
      const ctx = c.getContext("2d", { willReadFrequently: true })!;
      ctx.drawImage(bmp, 0, 0);
      genB2sCompositePixels = ctx.getImageData(0, 0, width, height).data;
      bmp.close();
      // Stash temp paths so "Detect lamps" works without re-extraction.
      const stem = sanitizeFilenameStem(selected?.name ?? "backglass");
      const compBytes = new Uint8Array(await blob.arrayBuffer());
      const { writeTempBytes } = await import("$lib/api");
      const compositePath = await writeTempBytes(`${stem}_composite.png`, compBytes);
      genB2sCompositePath = compositePath;
      genB2sOutputPath = compositePath.replace(/_composite\.png$/, "_scaffold.directb2s");
      genB2sStatus = "Wand ready — click to add a lamp";
      return true;
    } catch (e) {
      log("[b2s-from-video]", `lazy composite failed: ${e}`);
      return false;
    }
  }

  async function onMagicWandClick(e: MouseEvent) {
    const target = e.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const px = Math.round((e.clientX - rect.left) / rect.width  * genB2sCompositeW);
    const py = Math.round((e.clientY - rect.top)  / rect.height * genB2sCompositeH);
    // Hit-test first — left-click on a lamp is "select & edit", not
    // "wand-create a new overlapping lamp". Matches the user's mental
    // model: clicking a thing operates on it.
    const hit = hitTestBulbAt(px, py);
    if (hit) {
      openBulbMenu(e, hit.id);
      return;
    }
    // Empty space → wand flood-fill. Lazy-extract the composite on first
    // wand click (auto-loaded sessions don't have one until needed).
    if (!genB2sCompositePixels) {
      const ok = await ensureCompositeForWand();
      if (!ok) {
        genB2sStatus = "Wand needs the video frames — load video preview first";
        return;
      }
    }
    // Narrow + defend: ensureCompositeForWand() may report ok without
    // populating pixels (e.g. race/partial extract) — never flood-fill null.
    if (!genB2sCompositePixels) {
      genB2sStatus = "Wand couldn't build the composite — reload video preview";
      return;
    }
    const blob = magicWandFloodFill(
      genB2sCompositePixels,
      genB2sCompositeW, genB2sCompositeH,
      px, py,
      genB2sWandTolerance
    );
    if (!blob) {
      genB2sStatus = `No similar-color blob at click point — try raising the wand tolerance (currently ${genB2sWandTolerance})`;
      return;
    }
    const id = genB2sNextId++;
    const color = genB2sCompositePixels
      ? sampleLampColor(genB2sCompositePixels, genB2sCompositeW, genB2sCompositeH,
                        blob.x, blob.y, blob.mask, blob.w, blob.h)
      : ([255, 191, 36] as [number, number, number]);
    genB2sBulbs = [...genB2sBulbs, {
      id, label: `L${id}`,
      x: blob.x, y: blob.y, w: blob.w, h: blob.h,
      kept: true, source: "wand", mask: blob.mask,
      feather: genB2sFeather, color,
    }];
    genB2sStatus = `Wand: added ${`L${id}`} (${blob.pixelCount} px, feather ${genB2sFeather})`;
  }

  /** Hit-test: which bulb (if any) contains the given composite coord? */
  function hitTestBulbAt(px: number, py: number): GenBulb | null {
    // Walk in reverse so newer (top-rendered) lamps win on overlap.
    for (let i = genB2sBulbs.length - 1; i >= 0; i--) {
      const b = genB2sBulbs[i];
      if (px >= b.x && px < b.x + b.w && py >= b.y && py < b.y + b.h) {
        return b;
      }
    }
    return null;
  }

  /** Context-menu handler on the OVERLAY layer — picks the bulb under the
   *  cursor (if any). Right-clicking empty space is a no-op. */
  function onOverlayContextMenu(e: MouseEvent) {
    e.preventDefault();
    const target = e.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const px = (e.clientX - rect.left) / rect.width  * genB2sCompositeW;
    const py = (e.clientY - rect.top)  / rect.height * genB2sCompositeH;
    const hit = hitTestBulbAt(px, py);
    if (hit) openBulbMenu(e, hit.id);
  }

  /** Sample N frames from the active <video> element, build a max-brightness
   *  composite at a canonical 1920×1080 (16:9 — matches the preview
   *  container's aspect-video class).
   *
   *  IMPORTANT: we ALWAYS render into 1920×1080, even when the source video
   *  has a different native size or aspect ratio. Why: the <video> element
   *  defaults to `object-fit: fill` (stretches to fill the container), so
   *  any lamp position derived from the video's NATIVE coords would land
   *  in a different visual space than what the user sees. Stretching the
   *  composite to match the container's aspect means percent-positioned
   *  lamp overlays align pixel-perfectly with the on-screen video, no
   *  matter what the source resolution / aspect ratio is. */
  async function extractMaxBrightnessComposite(videoEl: HTMLVideoElement, frameCount = 30): Promise<{ blob: Blob; width: number; height: number }> {
    const w = 1920;
    const h = 1080;
    const canvas = document.createElement("canvas");
    canvas.width = w; canvas.height = h;
    const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
    // Composite accumulator — same size, all zeros.
    const composite = new Uint8ClampedArray(w * h * 4);
    const sampleEvery = videoEl.duration > 0 ? videoEl.duration / frameCount : 0.1;
    const wasPaused = videoEl.paused;
    videoEl.pause();
    for (let i = 0; i < frameCount; i++) {
      const t = sampleEvery * i;
      videoEl.currentTime = t;
      // Wait for seeked event
      await new Promise<void>((resolve) => {
        const handler = () => { videoEl.removeEventListener("seeked", handler); resolve(); };
        videoEl.addEventListener("seeked", handler, { once: true });
        // Fallback timeout in case seeked never fires
        setTimeout(() => { videoEl.removeEventListener("seeked", handler); resolve(); }, 500);
      });
      // 5-arg drawImage: stretches the source video to fill 1920×1080.
      // Matches the <video> element's default object-fit:fill behavior so
      // the composite frame is the same image the user sees on screen.
      ctx.drawImage(videoEl, 0, 0, w, h);
      const frame = ctx.getImageData(0, 0, w, h).data;
      for (let p = 0; p < composite.length; p += 4) {
        if (frame[p]     > composite[p])     composite[p]     = frame[p];
        if (frame[p + 1] > composite[p + 1]) composite[p + 1] = frame[p + 1];
        if (frame[p + 2] > composite[p + 2]) composite[p + 2] = frame[p + 2];
        composite[p + 3] = 255;
      }
    }
    ctx.putImageData(new ImageData(composite, w, h), 0, 0);
    if (!wasPaused) videoEl.play().catch(() => {});
    const blob: Blob | null = await new Promise(r => canvas.toBlob(r, "image/png"));
    if (!blob) throw new Error("composite toBlob returned null");
    return { blob, width: w, height: h };
  }

  /** Parse the scaffold's .directb2s and pull out bulb bboxes for the
   *  interactive editor. Doesn't need the sprites — only positions. */
  /** Decode the base64 lit-sprite PNG embedded on each <Bulb Image="..."/>
   *  back into a binary mask. The scaffold script writes pixels where the
   *  blob's alpha is 255 — outside the blob it's 0. We recover that exact
   *  binary mask so scaffold lamps render with the same Photoshop
   *  marching-ants outline and re-flood support as wand-added lamps. */
  async function maskFromBulbImageAttr(imgB64: string, expectedW: number, expectedH: number): Promise<{ mask: Uint8Array; color: [number, number, number] } | undefined> {
    try {
      const bin = atob(imgB64);
      const buf = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
      const bmp = await createImageBitmap(new Blob([buf], { type: "image/png" }));
      const w = bmp.width, h = bmp.height;
      const c = document.createElement("canvas");
      c.width = w; c.height = h;
      const ctx = c.getContext("2d", { willReadFrequently: true })!;
      ctx.drawImage(bmp, 0, 0);
      const data = ctx.getImageData(0, 0, w, h).data;
      bmp.close();
      const mask = new Uint8Array(w * h);
      // Also sample mean RGB from sprite pixels (alpha>0) — recovers the
      // lamp's actual tint so auto-loaded b2s shows correct colors instead
      // of falling back to amber/yellow when no composite is available.
      let rs = 0, gs = 0, bs = 0, n = 0;
      for (let i = 0; i < mask.length; i++) {
        const a = data[i * 4 + 3];
        if (a > 0) {
          mask[i] = 255;
          rs += data[i * 4];
          gs += data[i * 4 + 1];
          bs += data[i * 4 + 2];
          n++;
        }
      }
      const color: [number, number, number] = n > 0
        ? [Math.round(rs / n), Math.round(gs / n), Math.round(bs / n)]
        : [255, 191, 36];
      if (w !== expectedW || h !== expectedH) {
        log("[b2s-from-video]", `bulb sprite ${w}×${h} != bbox ${expectedW}×${expectedH}; using sprite dims`);
      }
      return { mask, color };
    } catch (e) {
      log("[b2s-from-video]", `mask decode failed: ${e}`);
      return undefined;
    }
  }

  /** Decode the PNG header of the .directb2s's BackglassImage to read
   *  the source bulb-coordinate space. Authored .directb2s files often
   *  use native PNG dims (e.g. 2000×1500) for bulb LocX/LocY/Width/
   *  Height; rendering bulbs at percent of a hardcoded 1920×1080 would
   *  shift them off the actual art. We use these dims as compositeW/H. */
  function readPngDims(bytes: Uint8Array): { w: number; h: number } | null {
    if (bytes.length < 24) return null;
    if (bytes[0] !== 0x89 || bytes[1] !== 0x50 || bytes[2] !== 0x4e || bytes[3] !== 0x47) return null;
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return { w: dv.getUint32(16, false), h: dv.getUint32(20, false) };
  }

  /** Parsed scaffold result. Source dims optional — when present they
   *  define the bulb coord space the editor should map percent positions
   *  against (instead of the default 1920×1080). */
  type ScaffoldParse = { bulbs: GenBulb[]; sourceW?: number; sourceH?: number };

  async function parseScaffoldDoc(directb2sPath: string): Promise<ScaffoldParse> {
    const { readLocalText } = await import("$lib/api");
    const xml = await readLocalText(directb2sPath);
    const doc = new DOMParser().parseFromString(xml, "application/xml");
    const out: GenBulb[] = [];
    genB2sNextId = 1;
    // Extract source dims from the BackglassImage PNG header.
    let sourceW: number | undefined;
    let sourceH: number | undefined;
    const bgEl = doc.querySelector("Images > BackglassImage[Value]");
    const bgB64 = bgEl?.getAttribute("Value");
    if (bgB64) {
      try {
        const bin = atob(bgB64.slice(0, 100));  // first 100 b64 chars decodes ~75 bytes — enough for PNG header
        const buf = new Uint8Array(bin.length);
        for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
        const dims = readPngDims(buf);
        if (dims) { sourceW = dims.w; sourceH = dims.h; }
      } catch {}
    }
    const bulbs = Array.from(doc.querySelectorAll("Illumination > Bulb"));
    // Sequentially decode lit-sprite masks. ~100 bulbs × ~5ms decode is
    // fast enough; parallelizing with Promise.all isn't worth the risk
    // of OOM on a giant scaffold.
    for (const b of bulbs) {
      const x = parseInt(b.getAttribute("LocX") ?? "0", 10);
      const y = parseInt(b.getAttribute("LocY") ?? "0", 10);
      const w = parseInt(b.getAttribute("Width") ?? "0", 10);
      const h = parseInt(b.getAttribute("Height") ?? "0", 10);
      if (w <= 0 || h <= 0) continue;
      const img = b.getAttribute("Image");
      const decoded = img ? await maskFromBulbImageAttr(img, w, h) : undefined;
      const mask = decoded?.mask;
      const id = genB2sNextId++;
      // Color priority: sprite-sampled (always present when sprite decoded
      // successfully — recovers the saved lamp's actual tint) → composite
      // re-sample if available (fresh from the current video) → amber.
      let color: [number, number, number] = [255, 191, 36];
      if (decoded?.color) {
        color = decoded.color;
      } else if (mask && genB2sCompositePixels) {
        color = sampleLampColor(genB2sCompositePixels, genB2sCompositeW, genB2sCompositeH, x, y, mask, w, h);
      }
      out.push({ id, label: `L${id}`, x, y, w, h, kept: true, source: "scaffold", mask, feather: 0, color });
    }
    return { bulbs: out, sourceW, sourceH };
  }

  /** Back-compat thin wrapper for callers that only want the bulb list
   *  (auto-load video sidecar, Detect-lamps button). Callers that also
   *  want to adopt the source-PNG coord space should call parseScaffoldDoc
   *  directly. */
  async function parseScaffoldBulbs(directb2sPath: string): Promise<GenBulb[]> {
    return (await parseScaffoldDoc(directb2sPath)).bulbs;
  }

  /** Open the b2s editor for the active video.
   *  - If the editor is ALREADY loaded for this table (e.g. auto-loaded
   *    a saved paired b2s), preserve existing lamps; just flip into
   *    authoring mode and ensure the composite is available for the
   *    wand color picker.
   *  - Otherwise start fresh: extract composite, open empty editor.
   *  Either way, "Detect lamps" in the toolbar can optionally append
   *  scaffold_b2s_from_png.py results to the current set. */
  async function openB2sEditorForVideo() {
    if (genB2sBusy) return;
    const folder = selected?.folder;
    const tableId = selected?.id;
    if (!folder || tableId === undefined || tableId === null) {
      dropError = "Select a table first."; return;
    }
    const videoEl = document.querySelector<HTMLVideoElement>("video[src]");
    if (!videoEl || !videoEl.videoWidth) {
      dropError = "Load the video preview first (enable auto preview).";
      return;
    }

    const sameTableEditor = (
      genB2sActive &&
      genB2sOwnerTableId === tableId &&
      genB2sBulbs.length > 0
    );

    genB2sBusy = true;
    genB2sStatus = sameTableEditor
      ? "Loading wand color picker…"
      : "Sampling video frames for wand color picker…";
    try {
      // The composite is required by the wand for flood-fill + color
      // sampling. Auto-loaded state doesn't have one (no need until the
      // user wants to add); extract it now whether we're keeping existing
      // lamps or starting fresh.
      if (!genB2sCompositePixels || genB2sCompositeW !== 1920 || genB2sCompositeH !== 1080) {
        const { blob, width, height } = await extractMaxBrightnessComposite(videoEl);
        genB2sCompositeW = width; genB2sCompositeH = height;
        const compBytes = new Uint8Array(await blob.arrayBuffer());
        try {
          const bmp = await createImageBitmap(blob);
          const ccanvas = document.createElement("canvas");
          ccanvas.width = width; ccanvas.height = height;
          const cctx = ccanvas.getContext("2d", { willReadFrequently: true })!;
          cctx.drawImage(bmp, 0, 0);
          genB2sCompositePixels = cctx.getImageData(0, 0, width, height).data;
          bmp.close();
        } catch (e) { log("[b2s-from-video]", `composite pixel cache failed: ${e}`); }
        // Stash temp paths so the optional Detect-lamps action can reuse them.
        const stem = sanitizeFilenameStem(selected?.name ?? "backglass");
        const { writeTempBytes } = await import("$lib/api");
        const compositePath = await writeTempBytes(`${stem}_composite.png`, compBytes);
        genB2sCompositePath = compositePath;
        genB2sOutputPath = compositePath.replace(/_composite\.png$/, "_scaffold.directb2s");
      }

      if (sameTableEditor) {
        // Editor already loaded for this table — preserve existing
        // bulbs and just enter authoring mode. User keeps their work.
        genB2sPreviewAnim = false;
        genB2sStatus = `Editing ${genB2sBulbs.length} lamp${genB2sBulbs.length === 1 ? '' : 's'} — click to wand-add, right-click an existing lamp for actions`;
        log("[b2s-from-video]", `editor enter-edit table=${tableId} bulbs=${genB2sBulbs.length} (preserved)`);
      } else {
        // Fresh start — empty editor.
        genB2sBulbs = [];
        genB2sNextId = 1;
        genB2sActive = true;
        genB2sOwnerTableId = tableId;
        genB2sPreviewAnim = false;
        genB2sStatus = "Click anywhere on the video to wand-add a lamp; right-click an existing lamp for actions";
        log("[b2s-from-video]", `editor opened table=${tableId} (fresh)`);
      }
    } catch (e) {
      dropError = `Edit B2S failed: ${e}`;
      log("[b2s-from-video]", `open error ${e}`);
      genB2sActive = false;
    } finally {
      genB2sBusy = false;
    }
  }

  /** Open the b2s editor for the DEFAULT authored b2s. Same toolset as
   *  the video flow but the composite source is the b2s base thumb
   *  (backglass.b2s_base.thumb.jpg in default_image), upscaled to
   *  1920×1080 so the wand can sample colors from the authored backglass
   *  art directly. Saves to a sidecar default_image/backglass_PP.directb2s
   *  — the authored backglass.directb2s is never clobbered. */
  async function openB2sEditorForDefault() {
    if (genB2sBusy) return;
    const folder = selected?.folder;
    const tableId = selected?.id;
    if (!folder || tableId === undefined || tableId === null) {
      dropError = "Select a table first."; return;
    }
    if (!selected?.has?.has("b2s")) {
      dropError = "This table has no authored B2S to edit."; return;
    }

    const sameTableEditor = (
      genB2sActive &&
      genB2sOwnerTableId === tableId &&
      genB2sBulbs.length > 0
    );

    genB2sBusy = true;
    genB2sStatus = sameTableEditor
      ? "Loading wand color picker…"
      : "Parsing authored B2S to align coord space…";
    try {
      // Three-source compatibility: scaffolded .directb2s (native PNG dims),
      // VPU downloads (whatever the author used), and PP Doctor video output
      // (1920×1080) must all render at the right positions. We parse the
      // .directb2s FIRST to learn its source dims, then size the composite
      // canvas to match — so bulb LocX/sourceW * 100 = correct percent.
      let imported: GenBulb[] = [];
      let sourceW = 1920, sourceH = 1080;
      const localPath = selected?.localDirectb2s;
      if (!sameTableEditor && localPath) {
        try {
          const { copyFileToCache } = await import("$lib/api");
          await copyFileToCache(sshHost(), folder, "default_image", "backglass.directb2s", localPath, cacheDir());
          log("[b2s-editor]", `mirrored authored .directb2s to local cache (versioned)`);
        } catch (e) {
          log("[b2s-editor]", `local backup of authored .directb2s failed: ${e}`);
        }
        try {
          genB2sStatus = `Reading authored .directb2s…`;
          const parsed = await parseScaffoldDoc(localPath);
          imported = parsed.bulbs;
          if (parsed.sourceW && parsed.sourceH) {
            sourceW = parsed.sourceW;
            sourceH = parsed.sourceH;
            log("[b2s-editor]", `source dims ${sourceW}×${sourceH} from BackglassImage`);
          }
          log("[b2s-editor]", `imported ${imported.length} authored bulbs from ${localPath}`);
        } catch (e) {
          log("[b2s-editor]", `import failed (${e}) — starting empty`);
        }
      }

      // Composite source for the default-b2s editor = the b2s base thumb,
      // upscaled to source dims so the wand reads pixels at the same coord
      // space the bulb positions live in.
      if (!genB2sCompositePixels || genB2sCompositeW !== sourceW || genB2sCompositeH !== sourceH) {
        const { cacheGetBinary } = await import("$lib/api");
        const thumbBytes = await cacheGetBinary(sshHost(), folder, "default_image", "backglass.b2s_base.thumb.jpg", cacheDir());
        if (!thumbBytes || thumbBytes.length === 0) {
          dropError = "No B2S base thumb found in default_image — can't sample colors. Pull the table first.";
          return;
        }
        const blob = new Blob([thumbBytes], { type: "image/jpeg" });
        const bmp = await createImageBitmap(blob);
        const c = document.createElement("canvas");
        c.width = sourceW; c.height = sourceH;
        const ctx = c.getContext("2d", { willReadFrequently: true })!;
        ctx.drawImage(bmp, 0, 0, sourceW, sourceH);
        bmp.close();
        genB2sCompositePixels = ctx.getImageData(0, 0, sourceW, sourceH).data;
        genB2sCompositeW = sourceW;
        genB2sCompositeH = sourceH;
        // Stash temp paths so optional Detect-lamps can scaffold from the thumb.
        const stem = sanitizeFilenameStem(selected?.name ?? "backglass") + "_b2s";
        const out: Blob | null = await new Promise(r => c.toBlob(r, "image/png"));
        if (out) {
          const compBytes = new Uint8Array(await out.arrayBuffer());
          const { writeTempBytes } = await import("$lib/api");
          const compositePath = await writeTempBytes(`${stem}_composite.png`, compBytes);
          genB2sCompositePath = compositePath;
          genB2sOutputPath = compositePath.replace(/_composite\.png$/, "_scaffold.directb2s");
        }
      }

      if (sameTableEditor) {
        genB2sPreviewAnim = false;
        genB2sStatus = `Editing ${genB2sBulbs.length} lamp${genB2sBulbs.length === 1 ? '' : 's'} — click to wand-add, right-click an existing lamp for actions`;
      } else {
        // Pre-existing _PP sidecar takes precedence over the authored
        // .directb2s when both exist — the user's prior edits supersede.
        // Re-parses to get the sidecar's source dims too (if it was saved
        // at different dims for some reason, we adopt those instead).
        try {
          const { cacheGetBinary, cacheFilePath } = await import("$lib/api");
          const sidecarBytes = await cacheGetBinary(sshHost(), folder, "default_image", "backglass_PP.directb2s", cacheDir());
          if (sidecarBytes && sidecarBytes.length > 0) {
            const sidecarPath = await cacheFilePath(sshHost(), folder, "default_image", "backglass_PP.directb2s", cacheDir());
            const sidecarParsed = await parseScaffoldDoc(sidecarPath);
            if (sidecarParsed.bulbs.length > 0) {
              imported = sidecarParsed.bulbs;
              log("[b2s-editor]", `loaded ${imported.length} bulbs from existing backglass_PP.directb2s sidecar`);
            }
          }
        } catch (e) { log("[b2s-editor]", `sidecar probe skipped: ${e}`); }

        genB2sBulbs = imported;
        // Continue numbering past the highest imported id so wand-adds
        // don't collide with authored ones.
        genB2sNextId = imported.length > 0
          ? imported.reduce((m, b) => Math.max(m, b.id), 0) + 1
          : 1;
        genB2sActive = true;
        genB2sOwnerTableId = tableId;
        genB2sPreviewAnim = false;
        genB2sStatus = imported.length > 0
          ? `Loaded ${imported.length} authored lamp${imported.length === 1 ? '' : 's'} — edit existing or wand-add new`
          : "B2S editor — click anywhere on the backglass to wand-add a lamp; right-click for actions";
      }
      log("[b2s-editor]", `default-b2s editor opened table=${tableId}`);
    } catch (e) {
      dropError = `Edit B2S failed: ${e}`;
      log("[b2s-editor]", `open error ${e}`);
      genB2sActive = false;
    } finally {
      genB2sBusy = false;
    }
  }

  /** OUTLINE path — boundary segments only, for stroke rendering
   *  (marching-ants). Pixel-perfect stairsteps along diagonals. Each
   *  filled cell whose neighbor is empty emits one M<x><y>h1 or
   *  M<x><y>v1 segment. NOT a valid fill path (disjoint subpaths
   *  enclose no area) — use maskToFilledSvgPath for fill. */
  function maskToSvgPath(mask: Uint8Array, mw: number, mh: number): string {
    const parts: string[] = [];
    for (let y = 0; y < mh; y++) {
      for (let x = 0; x < mw; x++) {
        if (!mask[y * mw + x]) continue;
        if (y === 0       || !mask[(y - 1) * mw + x]) parts.push(`M${x} ${y}h1`);
        if (y === mh - 1  || !mask[(y + 1) * mw + x]) parts.push(`M${x} ${y + 1}h1`);
        if (x === 0       || !mask[y * mw + (x - 1)]) parts.push(`M${x} ${y}v1`);
        if (x === mw - 1  || !mask[y * mw + (x + 1)]) parts.push(`M${x + 1} ${y}v1`);
      }
    }
    return parts.join("");
  }

  /** FILLED path — one closed rectangle per horizontal run of filled
   *  pixels. Each row scans left-to-right collapsing consecutive filled
   *  cells into a single `M x y h<run> v1 h-<run> z` subpath. Output is
   *  a valid fill path: each subpath encloses the run's area, so
   *  fill="<color>" actually fills it. Use this for the preview glow. */
  function maskToFilledSvgPath(mask: Uint8Array, mw: number, mh: number): string {
    const parts: string[] = [];
    for (let y = 0; y < mh; y++) {
      let runStart = -1;
      for (let x = 0; x <= mw; x++) {
        const filled = x < mw && mask[y * mw + x] > 0;
        if (filled && runStart < 0) {
          runStart = x;
        } else if (!filled && runStart >= 0) {
          const len = x - runStart;
          parts.push(`M${runStart} ${y}h${len}v1h${-len}z`);
          runStart = -1;
        }
      }
    }
    return parts.join("");
  }

  function toggleGenB2sBulb(id: number) {
    genB2sBulbs = genB2sBulbs.map(b => b.id === id ? { ...b, kept: !b.kept } : b);
  }

  /** Right-click context-menu state. Position derived from a 3×3 grid
   *  of the viewport:
   *
   *    ┌───────┬───────┬───────┐
   *    │ TL DR │ TC D  │ TR DL │   D=down, U=up, L=left, R=right, C=centered
   *    ├───────┼───────┼───────┤
   *    │ ML R  │ MC R  │ MR L  │
   *    ├───────┼───────┼───────┤
   *    │ BL UR │ BC U  │ BR UL │
   *    └───────┴───────┴───────┘
   *
   *  alignX/Y are the CSS transform offsets in %, applied via
   *  translate(alignX%, alignY%) — 0 anchors left/top to cursor,
   *  -50 centers, -100 anchors right/bottom. Pure CSS, measurement-free. */
  let bulbMenu = $state<null | { bulbId: number; x: number; y: number; alignX: number; alignY: number }>(null);

  // Bind for the live menu div — used by the global dismissal handler
  // to test whether a click landed INSIDE the menu (keep open) or
  // outside (dismiss). Positioning itself is transform-anchored and
  // doesn't need measurement.
  let bulbMenuEl = $state<HTMLDivElement | null>(null);

  // Global outside-click dismissal. Registered while a menu is open;
  // capture phase so it fires BEFORE the click reaches its target.
  // We close the menu but DON'T preventDefault — the original click /
  // right-click continues to its target (which can re-open the menu
  // for a different lamp). Without this you couldn't right-click one
  // lamp, then right-click another — the old backdrop button just
  // ate the second event.
  $effect(() => {
    if (!bulbMenu) return;
    function onWindowClick(e: MouseEvent) {
      if (bulbMenuEl && bulbMenuEl.contains(e.target as Node)) return;
      closeBulbMenu();
    }
    function onWindowContextMenu(e: MouseEvent) {
      if (bulbMenuEl && bulbMenuEl.contains(e.target as Node)) return;
      closeBulbMenu();
    }
    window.addEventListener("mousedown", onWindowClick, true);
    window.addEventListener("contextmenu", onWindowContextMenu, true);
    return () => {
      window.removeEventListener("mousedown", onWindowClick, true);
      window.removeEventListener("contextmenu", onWindowContextMenu, true);
    };
  });

  function openBulbMenu(e: MouseEvent, bulbId: number) {
    e.preventDefault();
    e.stopPropagation();
    // 3×3 grid placement:
    //   Click on ANY outer cell (8 surrounding cells) → menu centered
    //     in the middle-center cell (MC).
    //   Click in MC itself → menu shifts to an adjacent middle-row cell
    //     (ML or MR), picked OPPOSITE the cursor's side of MC so the
    //     menu doesn't cover the lamp the user just clicked.
    // translate(-50%, -50%) anchors the menu's center to the target
    // cell's center — bulletproof, can't overflow.
    const vw = typeof window !== "undefined" ? window.innerWidth  : 1920;
    const vh = typeof window !== "undefined" ? window.innerHeight : 1080;
    const colThird = e.clientX < vw / 3 ? 0
                   : e.clientX < (2 * vw) / 3 ? 1 : 2;
    const rowThird = e.clientY < vh / 3 ? 0
                   : e.clientY < (2 * vh) / 3 ? 1 : 2;
    let targetX: number, targetY: number;
    if (colThird === 1 && rowThird === 1) {
      // Click landed in middle-center → use ML or MR instead.
      // If cursor on LEFT half of MC → menu on RIGHT (MR center, x = 5*vw/6).
      // If cursor on RIGHT half of MC → menu on LEFT (ML center, x = vw/6).
      targetY = vh / 2;
      targetX = e.clientX < vw / 2 ? (5 * vw) / 6 : vw / 6;
    } else {
      // Outer cell click → center menu in middle of viewport (MC).
      targetX = vw / 2;
      targetY = vh / 2;
    }
    bulbMenu = { bulbId, x: targetX, y: targetY, alignX: -50, alignY: -50 };
  }
  function closeBulbMenu() { bulbMenu = null; }

  function deleteBulb(id: number) {
    genB2sBulbs = genB2sBulbs.filter(b => b.id !== id);
    closeBulbMenu();
  }
  function setBulbFeather(id: number, value: number) {
    genB2sBulbs = genB2sBulbs.map(b => b.id === id ? { ...b, feather: Math.max(0, Math.min(12, value)) } : b);
  }
  function renameBulb(id: number) {
    const b = genB2sBulbs.find(x => x.id === id);
    if (!b) return;
    const fresh = prompt(`Rename lamp ${b.label}:`, b.label)?.trim();
    if (fresh && fresh.length > 0) {
      genB2sBulbs = genB2sBulbs.map(x => x.id === id ? { ...x, label: fresh } : x);
    }
    closeBulbMenu();
  }
  function restoreBulb(id: number) {
    genB2sBulbs = genB2sBulbs.map(b => b.id === id ? { ...b, kept: true } : b);
    closeBulbMenu();
  }
  function discardBulb(id: number) {
    // Discard now means immediate delete — no "kept=false purgatory". User
    // feedback 2026-05-27: keeping discarded lamps around just clutters
    // the editor since they don't get saved anyway. If the user wants to
    // undo, that's a future undo-history feature; for now, click means gone.
    genB2sBulbs = genB2sBulbs.filter(b => b.id !== id);
    closeBulbMenu();
  }
  /** Re-flood-fill the wand bulb at its center pixel using the current
   *  tolerance — useful after adjusting the tolerance slider. The bulb's
   *  bbox + mask is recomputed in place. Only meaningful for source="wand"
   *  bulbs (scaffold bulbs don't have a stored seed pixel). */
  function refloodBulb(id: number) {
    if (!genB2sCompositePixels) return;
    const b = genB2sBulbs.find(x => x.id === id);
    if (!b) return;
    const cx = Math.round(b.x + b.w / 2);
    const cy = Math.round(b.y + b.h / 2);
    const fresh = magicWandFloodFill(
      genB2sCompositePixels, genB2sCompositeW, genB2sCompositeH,
      cx, cy, genB2sWandTolerance
    );
    if (!fresh) {
      genB2sStatus = `Reflood: no blob at lamp #${id} center with tolerance ${genB2sWandTolerance}`;
      closeBulbMenu();
      return;
    }
    genB2sBulbs = genB2sBulbs.map(x => x.id === id
      ? { ...x, x: fresh.x, y: fresh.y, w: fresh.w, h: fresh.h, mask: fresh.mask, source: "wand", kept: true }
      : x);
    genB2sStatus = `Lamp #${id} re-flooded (${fresh.pixelCount} px @ tol ${genB2sWandTolerance})`;
    closeBulbMenu();
  }

  /** Sanitize a table title into a filesystem-safe filename stem.
   *  Alphanumerics + dot + underscore + hyphen pass through; everything
   *  else collapses to a single underscore. Leading/trailing _ stripped.
   *  Used by ingestVideo, ingestImage, and saveGenB2s to keep the
   *  default_video/ filenames self-describing and consistent across
   *  the video, its thumb, and its paired generated .directb2s. */
  function sanitizeFilenameStem(s: string): string {
    return s.replace(/[^A-Za-z0-9._-]+/g, "_").replace(/^_+|_+$/g, "") || "backglass";
  }

  /** Encode a Uint8Array to base64 in chunks (avoids `btoa(String.from
   *  CharCode(...huge))` stack-overflow on large PNG sprite buffers). */
  function uint8ToBase64(u8: Uint8Array): string {
    let s = "";
    const CHUNK = 0x8000;
    for (let i = 0; i < u8.length; i += CHUNK) {
      s += String.fromCharCode.apply(null, Array.from(u8.subarray(i, i + CHUNK)));
    }
    return btoa(s);
  }

  /** Render a lamp's mask + sampled color into an RGBA PNG. Pixels where
   *  mask=255 get the bulb's color at full opacity; outside is transparent.
   *  Feather > 0 → soft alpha falloff via canvas blur filter (post-fill,
   *  before encoding). Returns PNG bytes ready to base64 + embed in
   *  the .directb2s. */
  async function renderLampSpritePng(bulb: GenBulb): Promise<Uint8Array> {
    const w = bulb.w, h = bulb.h;
    if (!bulb.mask) throw new Error("bulb has no mask");
    const c = document.createElement("canvas");
    c.width = w; c.height = h;
    const ctx = c.getContext("2d", { willReadFrequently: true })!;
    const id = ctx.createImageData(w, h);
    const [r, g, b] = bulb.color;
    for (let i = 0; i < w * h; i++) {
      if (bulb.mask[i]) {
        id.data[i * 4]     = r;
        id.data[i * 4 + 1] = g;
        id.data[i * 4 + 2] = b;
        id.data[i * 4 + 3] = 255;
      }
    }
    ctx.putImageData(id, 0, 0);
    let final: HTMLCanvasElement = c;
    if (bulb.feather > 0) {
      const c2 = document.createElement("canvas");
      c2.width = w; c2.height = h;
      const ctx2 = c2.getContext("2d")!;
      ctx2.filter = `blur(${bulb.feather}px)`;
      ctx2.drawImage(c, 0, 0);
      final = c2;
    }
    const blob: Blob | null = await new Promise(r => final.toBlob(r, "image/png"));
    if (!blob) throw new Error("sprite toBlob returned null");
    return new Uint8Array(await blob.arrayBuffer());
  }

  /** Build the .directb2s base image. Tries the video's paired thumb
   *  first — decodes the JPEG, upscales to 1920×1080, re-encodes as PNG.
   *  This makes the b2s self-contained: when loaded standalone (no
   *  video playing yet, or as a still preview), the thumb shows beneath
   *  the lamps. When the Pi plays the video on the Background layer,
   *  the thumb is overdrawn frame-by-frame so the lamps still composite
   *  correctly on top.
   *  Falls back to a transparent 1920×1080 PNG if the thumb is missing. */
  async function buildBasePng(folder: string, videoStem: string, slot: string = "default_video"): Promise<Uint8Array> {
    try {
      const { cacheGetBinary } = await import("$lib/api");
      const thumbBytes = await cacheGetBinary(sshHost(), folder, slot, `${videoStem}.thumb.jpg`, cacheDir());
      if (thumbBytes && thumbBytes.length > 0) {
        const blob = new Blob([thumbBytes], { type: "image/jpeg" });
        const bmp = await createImageBitmap(blob);
        const c = document.createElement("canvas");
        c.width = 1920; c.height = 1080;
        const ctx = c.getContext("2d")!;
        ctx.drawImage(bmp, 0, 0, 1920, 1080);
        bmp.close();
        const out: Blob | null = await new Promise(r => c.toBlob(r, "image/png"));
        if (out) return new Uint8Array(await out.arrayBuffer());
      }
    } catch (e) {
      log("[b2s-from-video]", `thumb-as-base load failed, using transparent: ${e}`);
    }
    // Fallback: transparent 1920×1080
    const c = document.createElement("canvas");
    c.width = 1920; c.height = 1080;
    const blob: Blob | null = await new Promise(r => c.toBlob(r, "image/png"));
    if (!blob) throw new Error("base toBlob returned null");
    return new Uint8Array(await blob.arrayBuffer());
  }

  /** Assemble the .directb2s XML from the current kept lamp set.
   *  Format matches what Pi's b2s_parser.cpp expects: <DirectB2SData>
   *  with <Illumination><Bulb LocX LocY Width Height RomID Image=
   *  "<base64 PNG>"/></Illumination> and an <Images><BackglassImage
   *  Value="..."/></Images> base. */
  async function buildDirectB2sXml(bulbs: GenBulb[], folder: string, baseStem: string, baseSlot: string = "default_video"): Promise<string> {
    const baseB64 = uint8ToBase64(await buildBasePng(folder, baseStem, baseSlot));
    let xml = '<?xml version="1.0" encoding="utf-8"?>\n<DirectB2SData>\n  <Illumination>\n';
    let romId = 1;
    for (const b of bulbs) {
      if (!b.mask) continue;
      const sprite = await renderLampSpritePng(b);
      const spriteB64 = uint8ToBase64(sprite);
      xml += `    <Bulb LocX="${b.x}" LocY="${b.y}" Width="${b.w}" Height="${b.h}" RomID="${romId}" Name="${b.label}" Image="${spriteB64}"/>\n`;
      romId++;
    }
    xml += '  </Illumination>\n  <Images>\n    <BackglassImage Value="' + baseB64 + '"/>\n  </Images>\n</DirectB2SData>\n';
    return xml;
  }

  /** Save the curated lamp set as <TableTitle>_PP.directb2s in the
   *  table's default_video/ slot — paired with the _PP.mp4 + _PP.thumb.jpg.
   *  Writes to local cache mirror, marks dirty, pushes via SCP. */
  let genB2sSaveBusy = $state(false);
  async function saveGenB2s() {
    if (genB2sSaveBusy) return;
    const folder = selected?.folder;
    const tableId = selected?.id;
    if (!folder || tableId === undefined || tableId === null) return;
    const kept = genB2sBulbs.filter(b => b.kept && b.mask);
    if (kept.length === 0) {
      genB2sStatus = "No lamps to save — add some first";
      return;
    }
    // Decide save target based on what's active. Video active → save as
    // <stem>_PP.directb2s in default_video (paired w/ the video). B2S
    // active → save as backglass_PP.directb2s in default_image (sidecar
    // beside the authored backglass.directb2s — never clobbers it).
    const isVideo = activeFile && /\.(mp4|webm|mkv|mov|m4v)$/i.test(activeFile);
    const stem = sanitizeFilenameStem(selected?.name ?? "backglass");
    const slot = isVideo ? "default_video" : "default_image";
    const filename = isVideo ? `${stem}_PP.directb2s` : "backglass_PP.directb2s";
    // For thumb-as-base lookup: video uses <stem>.thumb.jpg, default b2s
    // uses backglass.b2s_base.thumb.jpg. Reuses buildBasePng but we
    // pass the right key.
    const baseStem = isVideo ? stem : "backglass.b2s_base";
    genB2sSaveBusy = true;
    try {
      genB2sStatus = `Building .directb2s for ${kept.length} lamp${kept.length === 1 ? '' : 's'}…`;
      const xml = await buildDirectB2sXml(kept, folder, baseStem, isVideo ? "default_video" : "default_image");
      const { cacheWriteText, dbMarkDirty, syncPushDirty } = await import("$lib/api");
      genB2sStatus = `Writing ${(xml.length / 1024).toFixed(0)} KB to mirror…`;
      await cacheWriteText(sshHost(), folder, slot, filename, xml, cacheDir());
      await dbMarkDirty(tableId, slot, filename);
      await refreshDirtyTables();
      genB2sStatus = "Pushing to Pi…";
      await syncPushDirty(sshHost(), cacheDir());
      await refreshDirtyTables();
      genB2sStatus = `Saved ${filename} (${kept.length} lamps) → pushed to Pi`;
      log("[b2s-from-video]", `saved table=${tableId} ${filename} bulbs=${kept.length}`);
    } catch (e) {
      dropError = `Save failed: ${e}`;
      log("[b2s-from-video]", `save error: ${e}`);
    } finally {
      genB2sSaveBusy = false;
    }
  }

  /** Drive per-lamp opacities via b2s.ts::bulbAlpha — same attract math
   *  the Pi cabinet uses. Reads b2sCurrent (motion, alpha range, cycle,
   *  tail) every frame, so the B2S panel's sliders directly affect the
   *  live preview. Runs while preview is active; pauses otherwise. */
  $effect(() => {
    // Trigger reactivity on everything the loop reads.
    void genB2sPreviewAnim;
    void genB2sActive;
    void genB2sBulbs.length;
    void b2sCurrent.attractMotion;
    void b2sCurrent.attractMinAlpha;
    void b2sCurrent.attractMaxAlpha;
    void b2sCurrent.attractCycleSeconds;
    void b2sCurrent.attractTail;

    if (!genB2sPreviewAnim || !genB2sActive || genB2sBulbs.length === 0) {
      if (lampPreviewRaf !== null) { cancelAnimationFrame(lampPreviewRaf); lampPreviewRaf = null; }
      return;
    }
    // Snapshot bulb center coords + compute motion bounds (used by
    // bulbAlpha's WAVE phase math). For runner motion we also need a
    // sorted rank lookup — done once per loop arm.
    const bulbs = genB2sBulbs.map(b => ({
      id: b.id,
      cx: b.x + b.w / 2,
      cy: b.y + b.h / 2,
      w: b.w, h: b.h, x: b.x, y: b.y,
    }));
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const b of bulbs) {
      if (b.cx < minX) minX = b.cx; if (b.cx > maxX) maxX = b.cx;
      if (b.cy < minY) minY = b.cy; if (b.cy > maxY) maxY = b.cy;
    }
    const bounds = {
      minX: isFinite(minX) ? minX : 0,
      spanX: Math.max(1, maxX - minX),
      minY: isFinite(minY) ? minY : 0,
      spanY: Math.max(1, maxY - minY),
    };
    // Build a RUNNER rank table (sorted by x, defaults to x-axis wave dir).
    let runnerRank: Int32Array | undefined;
    if (b2sCurrent.attractMotion === "runner") {
      const order = bulbs.map((_, i) => i);
      order.sort((a, b) => bulbs[a].cx - bulbs[b].cx);
      runnerRank = new Int32Array(bulbs.length);
      for (let r = 0; r < order.length; r++) runnerRank[order[r]] = r;
    }
    const spec: B2SAttractSpec = {
      motion: b2sCurrent.attractMotion,
      lamps: bulbs.map(b => b.id),
      speedMs: Math.max(100, b2sCurrent.attractCycleSeconds * 1000),
      brightness: b2sCurrent.attractMaxAlpha,
      minBrightness: b2sCurrent.attractMinAlpha,
      waveDirection: "x",
      tail: b2sCurrent.attractTail,
      runnerRank,
    };

    if (lampPreviewRaf !== null) cancelAnimationFrame(lampPreviewRaf);
    lampPreviewStartTs = performance.now();
    const tick = (now: number) => {
      const next = new Map<number, number>();
      for (let i = 0; i < bulbs.length; i++) {
        const b = bulbs[i];
        const a = b2sBulbAlpha(
          { x: b.x, y: b.y, width: b.w, height: b.h } as B2SBulb,
          i, spec, bounds, now, lampPreviewStartTs
        );
        next.set(b.id, a);
      }
      lampAlphas = next;
      lampPreviewRaf = requestAnimationFrame(tick);
    };
    lampPreviewRaf = requestAnimationFrame(tick);
  });

  function closeGenB2sEditor() {
    genB2sActive = false;
    genB2sBulbs = [];
    genB2sStatus = "";
    genB2sCompositePath = null;
    genB2sOutputPath = null;
    genB2sCompositePixels = null;
    genB2sPreviewAnim = false;
    genB2sOwnerTableId = null;
    bulbMenu = null;
  }

  function onKey(e: KeyboardEvent) {
    if (!filtered.length) return;
    const tag = (e.target as HTMLElement)?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA") return;
    const idx = filtered.findIndex(t => t.id === selectedId);
    if (e.key === "ArrowDown" || e.key === "ArrowRight") {
      e.preventDefault();
      selectedId = filtered[(idx + 1) % filtered.length].id;
    } else if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
      e.preventDefault();
      selectedId = filtered[(idx - 1 + filtered.length) % filtered.length].id;
    }
  }

  /** Friendly label for a slot. */
  function slotLabel(s: Slot): string {
    return s.toUpperCase();
  }

  /** Color for a slot badge. Winner gets bright amber; others get muted. */
  function slotColor(s: Slot, isWinner: boolean): string {
    if (isWinner) return "bg-amber-400/15 text-amber-300 border-amber-400/30";
    switch (s) {
      case "bgra": return "bg-purple-500/10 text-purple-300 border-purple-500/20";
      case "png":  return "bg-emerald-500/10 text-emerald-300 border-emerald-500/20";
      case "jpg":  return "bg-emerald-500/10 text-emerald-300 border-emerald-500/20";
      case "webp": return "bg-emerald-500/10 text-emerald-300 border-emerald-500/20";
      case "b2s":  return "bg-sky-500/10 text-sky-300 border-sky-500/20";
      case "video":return "bg-rose-500/10 text-rose-300 border-rose-500/20";
      default:     return "bg-zinc-700/30 text-zinc-400 border-white/8";
    }
  }

  /** Order slots in a stable way for badge display. */
  const SLOT_ORDER: Slot[] = ["bgra", "png", "jpg", "webp", "gif", "b2s", "video"];
  function slotsOf(t: Table): Slot[] {
    return SLOT_ORDER.filter(s => t.has.has(s));
  }
</script>

<svelte:window onkeydown={onKey} />

<main class="h-screen flex flex-col bg-surface-100 text-zinc-100">
  <!-- Top bar -->
  <header class="flex items-center justify-between px-5 py-2 border-b border-white/8">
    <span class="font-mono text-xs text-zinc-400">cabinet · {ip}</span>
    <div class="flex items-center gap-3">
      {#if essentialsGaps.length > 0}
        <!-- Post-sync coverage gap banner. Shows when one or more tables
             lack a local backglass.b2scache / thumb, so the user knows
             which ones will hit the slow directb2s-fallback path. Click
             to expand the list; "Pull missing" re-syncs just those. -->
        <button type="button"
                onclick={() => essentialsExpanded = !essentialsExpanded}
                class="text-[11px] px-2 py-0.5 rounded
                       bg-amber-500/15 text-amber-200 border border-amber-500/40
                       hover:bg-amber-500/25 transition-colors font-mono"
                title="One or more tables are missing local cache + thumb — they'll fall back to the slow .directb2s parser on click.">
          ⚠ {essentialsGaps.length} table{essentialsGaps.length === 1 ? '' : 's'} not fully cached
        </button>
      {/if}
      <a href="/" class="text-xs text-zinc-400 hover:text-zinc-200 transition-colors">Disconnect</a>
    </div>
  </header>

  {#if essentialsGaps.length > 0 && essentialsExpanded}
    <!-- Expandable detail panel for the coverage gaps. Lists each gap
         table with its missing essentials + a one-click re-pull. Stays
         out of the way (collapsed by default) so it doesn't add chrome
         for the common case of a fully-synced cabinet. -->
    <div class="px-5 py-3 border-b border-amber-500/30 bg-amber-500/5 text-xs">
      <div class="flex items-center justify-between mb-2">
        <div class="text-amber-200 font-medium">
          Coverage audit: {essentialsGaps.length} table{essentialsGaps.length === 1 ? ' is' : 's are'} missing local cache files.
          Clicking these tables will fall back to the slow .directb2s parser.
        </div>
        <button type="button"
                onclick={pullMissingEssentials}
                disabled={essentialsPulling}
                class="text-[11px] px-2 py-1 rounded
                       bg-amber-400/15 text-amber-100 border border-amber-400/40
                       hover:bg-amber-400/25 disabled:opacity-40 transition-colors">
          {essentialsPulling ? "Pulling…" : "Pull missing from cabinet"}
        </button>
      </div>
      <div class="max-h-48 overflow-y-auto font-mono text-[10px] text-zinc-300 space-y-0.5">
        {#each essentialsGaps as g}
          <div class="flex items-baseline gap-2">
            <span class="text-zinc-500 w-12 text-right">#{g.table_id}</span>
            <span class="text-zinc-200 truncate flex-1">{g.name}</span>
            <span class="text-amber-300/70">{g.missing.join(", ")}</span>
          </div>
        {/each}
      </div>
    </div>
  {/if}

  {#if loading}
    <div class="flex-1 flex items-center justify-center text-zinc-400">
      <div class="text-center">
        <div class="animate-pulse text-amber-400 text-3xl mb-3">✦</div>
        <div class="text-sm">Scanning cabinet…</div>
      </div>
    </div>
  {:else if err}
    <div class="flex-1 flex items-center justify-center px-8">
      <div class="glass rounded-xl p-6 max-w-xl text-red-300">
        <div class="font-medium mb-2">Couldn't load tables</div>
        <pre class="text-xs font-mono text-red-400 whitespace-pre-wrap">{err}</pre>
      </div>
    </div>
  {:else}
    <div class="flex-1 flex overflow-hidden">
      <!-- Left: tables list -->
      <aside class="w-80 flex-shrink-0 border-r border-white/8 flex flex-col">
        <div class="p-3 border-b border-white/8">
          <input
            type="text"
            bind:value={q}
            placeholder="Search tables…"
            class="w-full px-3 py-2 rounded-lg
                   bg-black/30 border border-white/8
                   text-sm text-zinc-100 placeholder-zinc-500
                   focus:border-amber-400/60 focus:bg-black/40 transition-all"
          />
          <div class="mt-2 text-xs text-zinc-500">{filtered.length} of {tables.length}</div>
        </div>

        <div class="flex-1 overflow-y-auto py-1">
          {#each filtered as t (t.id)}
            <div
              class="relative w-full flex items-center gap-2
                     border-l-2 transition-colors
                     {selectedId === t.id
                       ? 'bg-amber-400/10 border-amber-400'
                       : 'border-transparent hover:bg-white/3'}
                     {!t.folder ? 'opacity-40' : ''}"
              style="content-visibility:auto;contain-intrinsic-size:auto 44px"
              data-tid={t.id}
            >
              <button
                onclick={() => (selectedId = t.id)}
                class="flex-1 text-left px-3 py-2.5 flex items-center gap-2 min-w-0"
              >
                <span class="font-mono text-xs text-zinc-500 w-8 flex-shrink-0">
                  {t.id.toString().padStart(3, " ")}
                </span>
                <span class="truncate text-sm flex-1 {selectedId === t.id ? 'text-amber-200' : 'text-zinc-300'}">
                  {t.name}
                </span>
                {#if badgeSlot(t) !== "none"}
                  <span
                    class="px-1.5 py-0.5 rounded text-[10px] font-mono font-medium border
                           {slotColor(badgeSlot(t), true)}"
                  >
                    {slotLabel(badgeSlot(t))}
                  </span>
                {/if}
              </button>
              <!-- Per-row sync icon — only shown when this table has dirty
                   files in the DB (local edits not yet pushed to the Pi).
                   Clicking pushes via the standard syncPushDirty flow. -->
              {#if dirtyTableIds.has(t.id)}
                <button
                  type="button"
                  onclick={(e) => { e.stopPropagation(); syncTable(t.id); }}
                  disabled={isSyncing}
                  aria-label="Push changes to cabinet"
                  title={isSyncing ? "Sync in progress…" : "Push this table's changes to the cabinet"}
                  class="mr-2 flex-shrink-0 w-6 h-6 rounded flex items-center justify-center
                         text-amber-300 hover:bg-amber-400/20 disabled:opacity-40 disabled:cursor-not-allowed
                         transition-colors"
                >
                  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                       stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="23 4 23 10 17 10"/>
                    <polyline points="1 20 1 14 7 14"/>
                    <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>
                  </svg>
                </button>
              {/if}
            </div>
          {/each}
        </div>
      </aside>

      <!-- Right: detail / preview -->
      <section class="flex-1 overflow-y-auto">
        {#if !selected}
          <div class="h-full flex items-center justify-center text-zinc-500">
            Select a table on the left.
          </div>
        {:else}
          <!-- Removed max-w-5xl mx-auto so the detail pane uses the full
               width to the right of the table list. The empty left margin
               was wasted space — now reclaimable for a B2S adjustments
               sidebar (user feedback 2026-05-25). -->
          <div class="px-6 pt-4 pb-8">
            <!-- Header: title + id + folder + B2S toggle all on ONE row.
                 User feedback 2026-05-25: consolidate the layout so title
                 and id stay close (instant Svelte update on click), folder
                 follows immediately after the id (was floated right), and
                 the B2S attract checkbox shifts up to the header instead
                 of taking its own row below. userImages/userVideos/
                 showingVideo are $derived in <script> since {@const} can't
                 sit directly under a <div>. -->
            <div class="mb-4 flex items-center gap-3 flex-wrap">
              <span class="text-lg text-zinc-400 font-mono">#{selected.id}</span>
              {#if selected.folder}
                <span class="text-base text-zinc-500 font-mono">{selected.folder}</span>
              {/if}
              <!-- Top-right unified "auto preview" toggle. Same state drives:
                   • B2S table → renders B2SCanvas with attract animation
                   • Video active → auto-loads the <video> element on table switch
                   • Image active → bloom canvas animation runs
                   • B2S+video together (future) → video plays with b2s lamp overlay
                   "Generate B2S from video" button shows when a video is active
                   (regardless of whether the table also has b2s) so the user can
                   author b2s overlay from video frames. -->
              {#if selected.folder}
                <div class="ml-auto flex items-center gap-3">
                  {#if showingVideo}
                    <button
                      type="button"
                      onclick={openB2sEditorForVideo}
                      class="text-[11px] px-2 py-1 rounded
                             bg-sky-500/10 text-sky-200 border border-sky-500/30
                             hover:bg-sky-500/20 transition-colors"
                      title="Open the b2s editor for this video — click to wand-add lamps. Result overlays on the video at runtime."
                    >Edit B2S</button>
                  {/if}
                  <!-- Zoom controls — visible only while the b2s editor is
                       active. Placed on the title row (same level as the
                       Edit B2S button + auto-preview toggle) so it doesn't
                       overlap the preview content. -->
                  {#if genB2sActive}
                    <div class="flex items-center gap-0.5 p-0.5 rounded-md
                                bg-zinc-900/60 border border-white/10 text-xs">
                      <button type="button" onclick={zoomOut} disabled={genB2sZoom <= 1}
                              class="w-6 h-6 flex items-center justify-center rounded
                                     hover:bg-white/10 disabled:opacity-30 disabled:cursor-not-allowed text-zinc-200"
                              aria-label="Zoom out">−</button>
                      <button type="button" onclick={zoomFit}
                              class="px-1.5 h-6 rounded font-mono text-[10px] text-zinc-300 hover:bg-white/10 min-w-[38px]"
                              aria-label="Reset zoom to fit">{Math.round(genB2sZoom * 100)}%</button>
                      <button type="button" onclick={zoomIn} disabled={genB2sZoom >= 4}
                              class="w-6 h-6 flex items-center justify-center rounded
                                     hover:bg-white/10 disabled:opacity-30 disabled:cursor-not-allowed text-zinc-200"
                              aria-label="Zoom in">+</button>
                    </div>
                  {/if}
                  <label class="flex items-center gap-2 text-sm text-zinc-400 cursor-pointer hover:text-zinc-200 transition-colors select-none">
                    <input
                      type="checkbox"
                      bind:checked={autoPreview}
                      class="rounded border-white/20 bg-black/30 text-amber-400 focus:ring-amber-400/40"
                    />
                    auto preview
                  </label>
                </div>
              {/if}
            </div>

            {#if !selected.folder}
              <div class="text-zinc-500 text-sm">
                No media folder for this table on the Pi yet.
              </div>
            {:else}

              <!-- Two-column layout: preview canvas (left) + B2S adjustments
                   sidebar (right). Adjustments sidebar uses ~280px; preview
                   absorbs the rest. Stacks on narrow screens via flex-wrap. -->
              <div class="flex gap-4 mb-6 flex-wrap">
                <!-- LEFT: Preview canvas. flex-1 absorbs remaining space.
                     Three nested layers:
                       OUTER (relative): hosts the floating zoom toolbar +
                         drop overlay so neither scrolls/scales with content.
                       SCROLL (overflow-auto aspect-video): clip + scrollbars
                         when ZOOM INNER is wider than this viewport.
                       ZOOM INNER (aspect-video, width=zoom*100%): scales the
                         preview content. width-based zoom (not transform:
                         scale) keeps layout natural — getBoundingClientRect
                         on the wand-click div returns the actual rendered
                         rect so percent → pixel math is unchanged. -->
                <div class="relative flex-1 min-w-0">
                <!-- B2S editor overlay snippet. Rendered in BOTH the video
                     preview branch (over the <video> element) and the B2S
                     preview branch (over the <B2SCanvas> component) so VPU
                     downloads / scaffolded .directb2s / PP Doctor video
                     sidecars all edit the same way. The snippet uses
                     absolute inset-0 to fill its rendering parent (the
                     ZOOM INNER), which gives correct coords regardless of
                     which preview branch is active. -->
                {#snippet b2sEditorOverlay()}
                  {#if genB2sActive}
                    <!-- Bulb overlay layer. Always-on Photoshop-style magic
                         wand: left-click anywhere creates a new lamp from the
                         flood-fill at that pixel. Right-click anywhere hit-
                         tests the lamp set; if you clicked on a lamp, its
                         action menu opens. Bulb visuals (boxes + marching
                         ants + labels) are pointer-events:none — purely
                         visual. -->
                    <!-- svelte-ignore a11y_click_events_have_key_events -->
                    <!-- Spatial pixel-picker: the flood-fill needs the exact
                         cursor (x,y); there is no keyboard equivalent to
                         "click this pixel", so no onkeydown is meaningful. -->
                    <div
                      class="absolute inset-0 z-10 cursor-crosshair"
                      onclick={onMagicWandClick}
                      oncontextmenu={onOverlayContextMenu}
                      role="button"
                      tabindex={0}
                      aria-label="Magic wand — left-click adds a lamp at the click point, right-click opens menu for a lamp under the cursor"
                    >
                      {#each genB2sBulbs as bulb, bulbIdx (bulb.id)}
                        {#if genB2sPreviewAnim && showingVideo && bulb.mask}
                          <!-- VIDEO + preview-anim: render the SVG glow
                               overlay. The <video> element underneath has
                               no lamp engine, so the overlay IS the lamp
                               animation in this branch. Two layers — halo
                               (blurred bloom) + body (sharp). -->
                          <svg
                            class="absolute pointer-events-none"
                            style="left:{((bulb.x - bulb.w * 0.5) / genB2sCompositeW * 100).toFixed(3)}%;
                                   top:{((bulb.y - bulb.h * 0.5) / genB2sCompositeH * 100).toFixed(3)}%;
                                   width:{(bulb.w * 2 / genB2sCompositeW * 100).toFixed(3)}%;
                                   height:{(bulb.h * 2 / genB2sCompositeH * 100).toFixed(3)}%;
                                   overflow:visible;
                                   opacity:{(lampAlphas.get(bulb.id) ?? 0).toFixed(3)};
                                   mix-blend-mode:screen;
                                   filter:blur({Math.max(6, 8 + bulb.feather * 2)}px);"
                            viewBox="0 0 {bulb.w * 2} {bulb.h * 2}"
                            preserveAspectRatio="none"
                          >
                            <g transform="translate({bulb.w * 0.5} {bulb.h * 0.5})">
                              <path d={maskToFilledSvgPath(bulb.mask, bulb.w, bulb.h)}
                                    fill="rgb({bulb.color[0]},{bulb.color[1]},{bulb.color[2]})"
                                    stroke="none" />
                            </g>
                          </svg>
                          <svg
                            class="absolute pointer-events-none"
                            style="left:{(bulb.x / genB2sCompositeW * 100).toFixed(3)}%;
                                   top:{(bulb.y / genB2sCompositeH * 100).toFixed(3)}%;
                                   width:{(bulb.w / genB2sCompositeW * 100).toFixed(3)}%;
                                   height:{(bulb.h / genB2sCompositeH * 100).toFixed(3)}%;
                                   overflow:visible;
                                   opacity:{(lampAlphas.get(bulb.id) ?? 0).toFixed(3)};
                                   mix-blend-mode:screen;
                                   filter:blur({Math.max(0, bulb.feather * 0.6)}px);"
                            viewBox="0 0 {bulb.w} {bulb.h}"
                            preserveAspectRatio="none"
                          >
                            <path d={maskToFilledSvgPath(bulb.mask, bulb.w, bulb.h)}
                                  fill="rgb({bulb.color[0]},{bulb.color[1]},{bulb.color[2]})"
                                  stroke="none" />
                          </svg>
                        {:else if genB2sPreviewAnim}
                          <!-- B2S + preview-anim: no per-bulb overlay.
                               B2SCanvas underneath runs the REAL attract
                               engine (same code path the Pi cabinet uses)
                               so editor preview matches auto-preview. The
                               wand-click hit-test layer stays alive so
                               right-click still works. -->
                        {:else if bulb.mask}
                          <!-- Authoring mode: Photoshop marching-ants trace of
                               the mask boundary. -->
                          <svg
                            class="absolute pointer-events-none ppe-marching-ants"
                            style="left:{(bulb.x / genB2sCompositeW * 100).toFixed(3)}%;
                                   top:{(bulb.y / genB2sCompositeH * 100).toFixed(3)}%;
                                   width:{(bulb.w / genB2sCompositeW * 100).toFixed(3)}%;
                                   height:{(bulb.h / genB2sCompositeH * 100).toFixed(3)}%;
                                   overflow:visible;"
                            viewBox="0 0 {bulb.w} {bulb.h}"
                            preserveAspectRatio="none"
                          >
                            <path d={maskToSvgPath(bulb.mask, bulb.w, bulb.h)}
                                  fill="none" stroke="#67e8f9" stroke-width="1"
                                  stroke-dasharray="3 2"
                                  vector-effect="non-scaling-stroke" />
                          </svg>
                        {/if}
                        <!-- Authoring chrome (bbox + label) — hidden in preview. -->
                        {#if !genB2sPreviewAnim}
                          <div
                            class="absolute pointer-events-none transition-all
                                   {bulb.source === 'wand'
                                      ? 'border border-sky-400/40'
                                      : 'border-2 border-amber-400/70 bg-amber-400/5'}"
                            style="left:{(bulb.x / genB2sCompositeW * 100).toFixed(3)}%;
                                   top:{(bulb.y / genB2sCompositeH * 100).toFixed(3)}%;
                                   width:{(bulb.w / genB2sCompositeW * 100).toFixed(3)}%;
                                   height:{(bulb.h / genB2sCompositeH * 100).toFixed(3)}%;"
                          >
                            <span class="absolute -top-1 -left-1 px-1 py-0
                                         text-[9px] font-mono font-medium leading-tight rounded
                                         {bulb.source === 'wand'
                                            ? 'bg-sky-500/90 text-zinc-950'
                                            : 'bg-amber-400/90 text-zinc-950'}">
                              {bulb.label}{#if bulb.feather > 0}<span class="opacity-60"> · f{bulb.feather}</span>{/if}
                            </span>
                          </div>
                        {/if}
                      {/each}
                    </div>

                    <!-- Right-click context menu — 3×3 grid-anchored. -->
                    {#if bulbMenu}
                      {@const menuBulb = genB2sBulbs.find(b => b.id === bulbMenu!.bulbId)}
                      {#if menuBulb}
                        <div
                          bind:this={bulbMenuEl}
                          class="fixed z-40 min-w-[180px] py-1 rounded-md
                                 bg-zinc-900/95 backdrop-blur-sm border border-white/10
                                 shadow-2xl text-xs"
                          style="left:{bulbMenu.x}px; top:{bulbMenu.y}px;
                                 transform: translate({bulbMenu.alignX}%, {bulbMenu.alignY}%);
                                 max-height: calc(100vh - 16px);
                                 overflow-y: auto;"
                        >
                          <div class="px-3 py-1.5 text-zinc-300 border-b border-white/5 font-mono flex items-center justify-between gap-3">
                            <span><b class="text-zinc-100">{menuBulb.label}</b><span class="text-zinc-600 ml-1">({menuBulb.source === 'wand' ? 'wand' : 'scaffold'})</span></span>
                            <span class="text-zinc-600 text-[10px]">{menuBulb.w}×{menuBulb.h}px</span>
                          </div>
                          <button type="button" onclick={() => renameBulb(menuBulb.id)}
                                  class="block w-full text-left px-3 py-1.5 text-zinc-200 hover:bg-white/8 transition-colors">
                            Rename…
                          </button>
                          <div class="px-3 py-1.5 hover:bg-white/5 transition-colors">
                            <div class="flex items-center justify-between text-[10px] text-zinc-500 mb-0.5">
                              <span>Feather edges</span>
                              <span class="font-mono text-zinc-300">{menuBulb.feather}px</span>
                            </div>
                            <input
                              type="range"
                              min="0" max="12" step="1"
                              value={menuBulb.feather}
                              oninput={(e) => setBulbFeather(menuBulb.id, parseInt((e.currentTarget as HTMLInputElement).value, 10))}
                              class="w-full accent-amber-400"
                            />
                          </div>
                          <button type="button" onclick={() => refloodBulb(menuBulb.id)}
                                  class="block w-full text-left px-3 py-1.5 text-zinc-200 hover:bg-sky-500/15 hover:text-sky-200 transition-colors">
                            Re-flood at current tolerance ({genB2sWandTolerance})
                          </button>
                          <div class="border-t border-white/5 my-1"></div>
                          <button type="button" onclick={() => discardBulb(menuBulb.id)}
                                  class="block w-full text-left px-3 py-1.5 text-rose-300 hover:bg-rose-500/25 hover:text-rose-100 transition-colors">
                            Discard (remove)
                          </button>
                        </div>
                      {/if}
                    {/if}
                  {/if}
                {/snippet}
                <div class="glass rounded-xl {genB2sActive && genB2sZoom > 1 ? 'overflow-auto' : 'overflow-hidden'} aspect-video bg-black/40 relative">
                <div class="aspect-video relative flex items-center justify-center" style="width:{(genB2sActive ? genB2sZoom : 1) * 100}%; min-width: 100%;">
                  {#if detailLoading}
                    <div class="text-zinc-500 text-sm">Loading preview…</div>
                  {:else if showingVideo}
                    <!-- Video preview branch — wins over B2S preview when the
                         user explicitly picked a video file as active (radio
                         click or saved choice). Generate-B2S-from-video
                         operates on the live <video> element and needs the
                         actual video frames, NOT the b2s thumb. (b2s-on-video
                         overlay is a future Phase 3 feature.) -->
                    {#if videoDataUrl}
                      <!-- svelte-ignore a11y_media_has_caption -->
                      <video
                        src={videoDataUrl}
                        poster={videoThumbUrl ?? undefined}
                        controls loop autoplay muted
                        class="w-full h-full"
                      ></video>

                      <!-- B2S editor overlay (snippet rendered identically in
                           the B2S preview branch below — so VPU downloads,
                           scaffolded .directb2s, and PP Doctor video-derived
                           sidecars all edit the same way regardless of which
                           preview branch their table falls into). -->
                      {@render b2sEditorOverlay()}
                    {:else if activeVideoFile}
                      <button onclick={loadVideo} class="text-amber-300 text-sm hover:text-amber-200">
                        ▶ Load video preview ({activeVideoFile})
                      </button>
                    {:else}
                      <div class="text-zinc-600 text-sm">No video in default_video/</div>
                    {/if}
                  {:else if (glowOn || genB2sActive) && selected.has.has("b2s")}
                    <!-- B2S preview branch — runs when the user is NOT viewing
                         a video (showingVideo=false above wins). Honors
                         autoPreview as the "attract preview" toggle OR forces
                         B2SCanvas when the editor is active (so authoring
                         always has the backglass as a positioned overlay
                         target, regardless of the preview toggle). -->
                    {#if b2sLoading}
                      <div class="text-zinc-500 text-sm">Fetching backglass.directb2s…</div>
                    {:else if b2sError}
                      <div class="text-zinc-500 text-sm text-center px-8">{b2sError}</div>
                    {:else if b2sXml || b2sCacheBuf}
                      <B2SCanvas
                        xml={b2sXml}
                        cacheBuf={b2sCacheBuf}
                        eventMapJson={b2sEventMapJson}
                        {baseBrightness}
                        overrideMinAlpha={b2sCurrent.attractMinAlpha}
                        overrideMaxAlpha={b2sCurrent.attractMaxAlpha}
                        overrideCycleSeconds={b2sCurrent.attractCycleSeconds}
                        overrideMotion={b2sCurrent.attractMotion}
                        overrideTail={b2sCurrent.attractTail}
                      />
                      <!-- Editor overlay on top of B2SCanvas — same lamp
                           markers + right-click menu the video branch uses. -->
                      {@render b2sEditorOverlay()}
                    {/if}
                  {:else if imageDataUrl}
                    <!-- Canvas-rendered preview with bloom pipeline. The
                         <img>-with-CSS-filter approach was swapped to a
                         canvas additive composite 2026-05-25 to match the
                         B2S glow shape (threshold + min/max alpha +
                         animated motion). -->
                    <canvas
                      bind:this={bloomCanvas}
                      aria-label={selected.name}
                      class="w-full h-full object-contain"
                      style="object-fit: contain;"
                    ></canvas>
                  {:else}
                    <div class="text-zinc-600 text-sm">No preview available</div>
                  {/if}

                  <!-- Drop status / error moved OUT of the preview container —
                       used to overlay the bottom band of the video and block
                       lamp clicks. Now renders as a normal flow element below
                       the preview / sidebar row (see "Status toast" further
                       down). -->
                </div><!-- /ZOOM INNER -->
                </div><!-- /SCROLL -->

                <!-- Drop overlay — visible while a file is being dragged
                     over the window. Whole preview area is the drop
                     target; Tauri delivers absolute paths via
                     onDragDropEvent (registered in onMount). Rendered at
                     OUTER level so it covers the visible scroll viewport
                     regardless of inner zoom/scroll position. -->
                {#if isDragOver || dropBusy}
                  <div class="absolute inset-0 z-20 flex items-center justify-center pointer-events-none
                              bg-zinc-950/70 backdrop-blur-sm
                              border-2 border-dashed
                              {dropBusy ? 'border-amber-400/60' : 'border-amber-300/80'}
                              rounded-xl">
                    <div class="text-center px-6">
                      {#if dropBusy}
                        <div class="text-amber-300 text-sm font-medium animate-pulse">{dropStatus || "Working…"}</div>
                      {:else}
                        <div class="text-amber-200 text-base font-medium mb-1">Drop image, video, or .directb2s</div>
                        <div class="text-zinc-400 text-xs leading-snug">
                          Images auto-resize to 1920×1080 + reencode as JPEG.<br/>
                          Videos transcode via ffmpeg to 1080p H.264 (24/30 fps).<br/>
                          .directb2s lands as default_image/backglass_PP.directb2s (sidecar — original kept).<br/>
                          Previous file backed up to <span class="font-mono">.versions/</span> (≤5).
                        </div>
                      {/if}
                    </div>
                  </div>
                {/if}

                </div><!-- /OUTER -->
                <!-- RIGHT: B2S adjustments panel (only shown when b2s preview
                     is active so we don't waste space on image/video previews). -->
                {#if activePanel === "b2s"}
                  <aside class="glass rounded-xl p-4 w-64 flex-shrink-0 self-start flex flex-col gap-4">
                    <h3 class="text-xs uppercase tracking-wider text-zinc-400 font-medium">B2S lamp adjustment</h3>

                    <!-- Base brightness (preview-only — never pushed to Pi) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="brightness-slider" class="text-xs text-zinc-400">Base brightness</label>
                        <span class="text-[10px] font-mono text-zinc-500">{b2sCurrent.baseBrightness.toFixed(2)}×</span>
                      </div>
                      <input
                        id="brightness-slider"
                        type="range"
                        min="0.3" max="2.5" step="0.05"
                        bind:value={b2sCurrent.baseBrightness}
                        class="w-full accent-amber-400"
                      />
                      <div class="text-[9px] text-zinc-600 mt-0.5">Preview-only.</div>
                    </div>

                    <!-- attract_min_alpha (event_map override) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="min-alpha-slider" class="text-xs text-zinc-400">attract_min_alpha</label>
                        <span class="text-[10px] font-mono text-zinc-500">{b2sCurrent.attractMinAlpha}</span>
                      </div>
                      <input
                        id="min-alpha-slider"
                        type="range"
                        min="0" max="255" step="1"
                        bind:value={b2sCurrent.attractMinAlpha}
                        class="w-full accent-amber-400"
                      />
                    </div>

                    <!-- attract_max_alpha (event_map override) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="max-alpha-slider" class="text-xs text-zinc-400">attract_max_alpha</label>
                        <span class="text-[10px] font-mono text-zinc-500">{b2sCurrent.attractMaxAlpha}</span>
                      </div>
                      <input
                        id="max-alpha-slider"
                        type="range"
                        min="0" max="255" step="1"
                        bind:value={b2sCurrent.attractMaxAlpha}
                        class="w-full accent-amber-400"
                      />
                    </div>

                    <!-- Attract motion type — port of b2s_motion.cpp.
                         Sweep/ripple match Pi's ALL_ON fallback today;
                         runner has full Cylon ping-pong with tail fade. -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="b2s-motion" class="text-xs text-zinc-400">Animation</label>
                      </div>
                      <select
                        id="b2s-motion"
                        bind:value={b2sCurrent.attractMotion}
                        class="w-full text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200"
                      >
                        <option value="wave">Wave</option>
                        <option value="runner">Runner (Cylon)</option>
                        <option value="random">Random</option>
                        <option value="strobe">Strobe</option>
                        <option value="flash">Flash (static)</option>
                        <option value="all_on">All-on (static)</option>
                        <option value="sweep">Sweep (= all-on on Pi)</option>
                        <option value="ripple">Ripple (= all-on on Pi)</option>
                      </select>
                    </div>

                    {#if b2sCurrent.attractMotion === "runner"}
                      <!-- Runner tail length (only meaningful for that motion) -->
                      <div>
                        <div class="flex items-baseline justify-between mb-1">
                          <label for="b2s-tail" class="text-xs text-zinc-400">Tail length</label>
                          <span class="text-[10px] font-mono text-zinc-500">{b2sCurrent.attractTail}</span>
                        </div>
                        <input
                          id="b2s-tail"
                          type="range"
                          min="1" max="12" step="1"
                          bind:value={b2sCurrent.attractTail}
                          class="w-full accent-amber-400"
                        />
                      </div>
                    {/if}

                    <!-- pulse speed = attract_cycle_seconds (event_map override) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="cycle-slider" class="text-xs text-zinc-400">Pulse speed</label>
                        <span class="text-[10px] font-mono text-zinc-500">{b2sCurrent.attractCycleSeconds.toFixed(1)}s/cycle</span>
                      </div>
                      <input
                        id="cycle-slider"
                        type="range"
                        min="0.5" max="10" step="0.1"
                        bind:value={b2sCurrent.attractCycleSeconds}
                        class="w-full accent-amber-400"
                      />
                    </div>

                    <!-- Save / Revert at the bottom — covers all sliders -->
                    <div class="pt-3 mt-auto border-t border-white/5">
                      <div class="flex gap-2">
                        <button
                          type="button"
                          onclick={saveB2SSettings}
                          disabled={!b2sSettingsDirty}
                          class="flex-1 text-[11px] px-2 py-1.5 rounded
                                 {b2sSettingsDirty
                                   ? 'bg-amber-400/15 text-amber-200 border border-amber-400/40 hover:bg-amber-400/25'
                                   : 'bg-white/5 text-zinc-600 border border-white/5 cursor-not-allowed'}
                                 transition-colors font-medium"
                          title={b2sSettingsDirty ? 'Persist all adjustments for this table' : 'No unsaved changes'}
                        >Save</button>
                        <button
                          type="button"
                          onclick={revertB2SSettings}
                          disabled={!b2sSettingsDirty}
                          class="flex-1 text-[11px] px-2 py-1.5 rounded
                                 {b2sSettingsDirty
                                   ? 'bg-white/5 text-zinc-300 border border-white/10 hover:bg-white/10'
                                   : 'bg-white/5 text-zinc-600 border border-white/5 cursor-not-allowed'}
                                 transition-colors font-medium"
                          title={b2sSettingsDirty ? 'Discard all changes and restore last saved values' : 'No unsaved changes'}
                        >Revert</button>
                      </div>
                      <div class="text-[9px] text-zinc-500 mt-1.5 leading-snug">
                        {#if b2sSettingsDirty}
                          <span class="text-amber-400">●</span> unsaved changes
                        {:else}
                          Saved <span class="font-mono">{formatSavedAt(b2sSavedAt)}</span>
                        {/if}
                      </div>
                      <div class="text-[9px] text-zinc-600 mt-1 leading-snug">
                        Save also pushes to the Pi Zero 2 (writes b2s_event_map.json
                        to the local mirror and SCPs to the cabinet).
                      </div>
                    </div>
                  </aside>
                {:else if activePanel === "image"}
                  <aside class="glass rounded-xl p-4 w-64 flex-shrink-0 self-start flex flex-col gap-4">
                    <h3 class="text-xs uppercase tracking-wider text-zinc-400 font-medium">Image bloom adjustment</h3>

                    <!-- Base brightness (CSS brightness on the base draw) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="img-base-bright" class="text-xs text-zinc-400">Base brightness</label>
                        <span class="text-[10px] font-mono text-zinc-500">{imgCurrent.baseBrightness.toFixed(2)}×</span>
                      </div>
                      <input
                        id="img-base-bright"
                        type="range"
                        min="0.3" max="2.5" step="0.05"
                        bind:value={imgCurrent.baseBrightness}
                        class="w-full accent-amber-400"
                      />
                      <div class="flex justify-between text-[9px] text-zinc-600 mt-0.5 font-mono">
                        <span>0.3×</span><span>2.5×</span>
                      </div>
                      <div class="text-[9px] text-zinc-600 mt-0.5">Preview-only.</div>
                    </div>

                    <!-- Threshold (luminance cutoff for glow source) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="img-thr" class="text-xs text-zinc-400">Bright threshold</label>
                        <span class="text-[10px] font-mono text-zinc-500">{Math.round(imgCurrent.threshold * 100)}%</span>
                      </div>
                      <input
                        id="img-thr"
                        type="range"
                        min="0" max="1" step="0.01"
                        bind:value={imgCurrent.threshold}
                        class="w-full accent-amber-400"
                      />
                      <div class="flex justify-between text-[9px] text-zinc-600 mt-0.5 font-mono">
                        <span>0%</span><span>100%</span>
                      </div>
                      <div class="text-[9px] text-zinc-600 mt-0.5">Pixels brighter than this glow.</div>
                    </div>

                    <!-- Min alpha (bloom layer floor) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="img-min-alpha" class="text-xs text-zinc-400">Min alpha</label>
                        <span class="text-[10px] font-mono text-zinc-500">{imgCurrent.minAlpha}</span>
                      </div>
                      <input
                        id="img-min-alpha"
                        type="range"
                        min="0" max="255" step="1"
                        bind:value={imgCurrent.minAlpha}
                        class="w-full accent-amber-400"
                      />
                      <div class="flex justify-between text-[9px] text-zinc-600 mt-0.5 font-mono">
                        <span>0</span><span>255</span>
                      </div>
                    </div>

                    <!-- Max alpha (bloom layer peak) -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="img-max-alpha" class="text-xs text-zinc-400">Max alpha</label>
                        <span class="text-[10px] font-mono text-zinc-500">{imgCurrent.maxAlpha}</span>
                      </div>
                      <input
                        id="img-max-alpha"
                        type="range"
                        min="0" max="255" step="1"
                        bind:value={imgCurrent.maxAlpha}
                        class="w-full accent-amber-400"
                      />
                      <div class="flex justify-between text-[9px] text-zinc-600 mt-0.5 font-mono">
                        <span>0</span><span>255</span>
                      </div>
                    </div>

                    <!-- Motion type -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="img-motion" class="text-xs text-zinc-400">Animation</label>
                      </div>
                      <select
                        id="img-motion"
                        bind:value={imgCurrent.motion}
                        class="w-full text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200"
                      >
                        <option value="wave">Wave</option>
                        <option value="runner">Runner (= wave for image)</option>
                        <option value="random">Random</option>
                        <option value="strobe">Strobe</option>
                        <option value="flash">Flash (static)</option>
                        <option value="all_on">All-on (static)</option>
                        <option value="sweep">Sweep (= wave for image)</option>
                        <option value="ripple">Ripple (= wave for image)</option>
                      </select>
                    </div>

                    <!-- Cycle speed -->
                    <div>
                      <div class="flex items-baseline justify-between mb-1">
                        <label for="img-cycle" class="text-xs text-zinc-400">Speed</label>
                        <span class="text-[10px] font-mono text-zinc-500">{imgCurrent.cycleSeconds.toFixed(1)}s/cycle</span>
                      </div>
                      <input
                        id="img-cycle"
                        type="range"
                        min="0.5" max="10" step="0.1"
                        bind:value={imgCurrent.cycleSeconds}
                        class="w-full accent-amber-400"
                      />
                      <div class="flex justify-between text-[9px] text-zinc-600 mt-0.5 font-mono">
                        <span>0.5s</span><span>10s</span>
                      </div>
                    </div>

                    <!-- Save / Revert at the bottom — covers all controls -->
                    <div class="pt-3 mt-auto border-t border-white/5">
                      <div class="flex gap-2">
                        <button
                          type="button"
                          onclick={saveImageBloom}
                          disabled={!imageBloomDirty}
                          class="flex-1 text-[11px] px-2 py-1.5 rounded
                                 {imageBloomDirty
                                   ? 'bg-amber-400/15 text-amber-200 border border-amber-400/40 hover:bg-amber-400/25'
                                   : 'bg-white/5 text-zinc-600 border border-white/5 cursor-not-allowed'}
                                 transition-colors font-medium"
                          title={imageBloomDirty ? 'Persist image bloom for this table' : 'No unsaved changes'}
                        >Save</button>
                        <button
                          type="button"
                          onclick={revertImageBloom}
                          disabled={!imageBloomDirty}
                          class="flex-1 text-[11px] px-2 py-1.5 rounded
                                 {imageBloomDirty
                                   ? 'bg-white/5 text-zinc-300 border border-white/10 hover:bg-white/10'
                                   : 'bg-white/5 text-zinc-600 border border-white/5 cursor-not-allowed'}
                                 transition-colors font-medium"
                          title={imageBloomDirty ? 'Discard changes and restore last saved values' : 'No unsaved changes'}
                        >Revert</button>
                      </div>
                      <div class="text-[9px] text-zinc-500 mt-1.5 leading-snug">
                        {#if imageBloomDirty}
                          <span class="text-amber-400">●</span> unsaved changes
                        {:else}
                          Saved <span class="font-mono">{formatSavedAt(imgSavedAt)}</span>
                        {/if}
                      </div>
                      <div class="text-[9px] text-zinc-600 mt-1 leading-snug">
                        Preview-only — bloom is rendered locally on a canvas and never pushed to the cabinet
                        (the Pi displays the raw JPG/PNG byte-for-byte).
                      </div>
                    </div>
                  </aside>
                {/if}
              </div>

              <!-- Status toast — formerly absolute-positioned inside the
                   preview, covering the bottom band of the video and
                   blocking lamp right-clicks. Now in normal flow below
                   the preview so the media area stays uncluttered. -->
              {#if !isDragOver && !dropBusy && (dropStatus || dropError)}
                <div class="mb-3 px-3 py-1.5 rounded text-[11px] font-mono flex items-center
                            {dropError
                              ? 'bg-rose-500/15 text-rose-200 border border-rose-500/30'
                              : 'bg-emerald-500/15 text-emerald-200 border border-emerald-500/30'}">
                  <span class="flex-1 truncate">{dropError || dropStatus}</span>
                  <button
                    type="button"
                    class="ml-3 text-zinc-400 hover:text-zinc-100"
                    onclick={() => { dropStatus = ""; dropError = ""; }}
                    aria-label="Dismiss"
                  >×</button>
                </div>
              {/if}

              <!-- Lamp editor toolbar — sits BELOW the preview (per user
                   request 2026-05-27). Was previously absolute-positioned
                   over the bottom of the video; relocated so the lamp
                   overlay itself stays unobstructed. Only renders while
                   the b2s-from-video editor is active. -->
              {#if genB2sActive}
                <div class="glass rounded-xl px-4 py-2 mb-6 flex items-center gap-4 text-xs flex-wrap">
                  <span class="text-amber-300 font-medium">
                    {genB2sBulbs.filter(b => b.kept).length}/{genB2sBulbs.length} lamps
                  </span>
                  <button
                    type="button"
                    onclick={() => (genB2sPreviewAnim = !genB2sPreviewAnim)}
                    class="text-[11px] px-2 py-1 rounded transition-colors
                           {genB2sPreviewAnim
                             ? 'bg-emerald-400/25 text-emerald-100 border border-emerald-300/60'
                             : 'bg-emerald-500/10 text-emerald-200 border border-emerald-500/30 hover:bg-emerald-500/20'}"
                    title="Preview the b2s lamp animation over the video. Per-lamp phase delay produces a wave/ripple feel like the Pi's attract mode."
                  >{genB2sPreviewAnim ? '⏸ Stop preview' : '▶ Preview animation'}</button>
                  <label class="flex items-center gap-2 text-zinc-400"
                         title="RGB color distance from the clicked pixel — Photoshop's magic-wand tolerance. 0=exact, 32=default, 128=loose">
                    <span>✦ Tolerance</span>
                    <input
                      type="range"
                      min="0" max="128" step="1"
                      bind:value={genB2sWandTolerance}
                      class="w-24 accent-amber-400"
                    />
                    <span class="font-mono text-zinc-300 w-6">{genB2sWandTolerance}</span>
                  </label>
                  <label class="flex items-center gap-2 text-zinc-400"
                         title="Feather radius — softens the lamp edges (alpha falloff) when the sprite is generated. Applied to newly-created wand lamps; existing lamps keep their own per-lamp value">
                    <span>Feather</span>
                    <input
                      type="range"
                      min="0" max="12" step="1"
                      bind:value={genB2sFeather}
                      class="w-20 accent-amber-400"
                    />
                    <span class="font-mono text-zinc-300 w-6">{genB2sFeather}px</span>
                  </label>
                  <button
                    type="button"
                    onclick={saveGenB2s}
                    disabled={genB2sSaveBusy || genB2sBulbs.filter(b => b.kept && b.mask).length === 0}
                    class="text-[11px] px-3 py-1 rounded font-medium transition-colors
                           bg-emerald-500/20 text-emerald-100 border border-emerald-400/60
                           hover:bg-emerald-500/30 disabled:opacity-40 disabled:cursor-not-allowed"
                    title="Build .directb2s from kept lamps (with masked + colored + feathered lit sprites) and push it to the Pi as <TableTitle>_PP.directb2s in default_video/"
                  >{genB2sSaveBusy ? "Saving…" : "💾 Save & Push"}</button>
                  <div class="flex-1"></div>
                  <span class="text-zinc-500 truncate">{genB2sStatus}</span>
                  <button
                    type="button"
                    onclick={closeGenB2sEditor}
                    class="text-[11px] px-2 py-1 rounded
                           bg-zinc-800 text-zinc-300 border border-white/10
                           hover:bg-zinc-700 transition-colors"
                  >Close</button>
                </div>
              {/if}

              <!-- Version history — backups of the most recently dropped /
                   pushed file, with restore buttons. Up to 5 retained. -->
              {#if dropVersions.length > 0}
                <div class="glass rounded-xl p-5 mb-4">
                  <div class="flex items-baseline justify-between mb-3">
                    <h2 class="font-medium text-zinc-200">
                      Previous versions
                      <span class="ml-2 text-xs text-zinc-500 font-mono">
                        {dropVersionsFor.slot}/{dropVersionsFor.filename}
                      </span>
                    </h2>
                    <div class="flex items-center gap-3">
                      <span class="text-xs text-zinc-500">Up to 5 backups · click restore</span>
                      <button
                        type="button"
                        onclick={deleteAllVersions}
                        class="text-[10px] px-2 py-0.5 rounded
                               bg-rose-500/10 text-rose-200 border border-rose-500/30
                               hover:bg-rose-500/20 transition-colors"
                        title="Delete every backup version of the active file from local mirror"
                      >Delete all</button>
                    </div>
                  </div>
                  <ul class="space-y-1">
                    {#each dropVersions as v}
                      <li class="flex items-center gap-3 px-2 py-1.5 rounded hover:bg-white/3 transition-colors">
                        <span class="font-mono text-xs text-zinc-400 flex-1 truncate">{v.filename}</span>
                        <span class="font-mono text-[10px] text-zinc-600">
                          {new Date(v.mtime_ms).toLocaleString()}
                        </span>
                        <span class="font-mono text-[10px] text-zinc-600 w-16 text-right">
                          {fmtBytes(v.size)}
                        </span>
                        <button
                          type="button"
                          onclick={() => restoreDropVersion(v.filename)}
                          disabled={dropBusy}
                          class="text-[11px] px-2 py-1 rounded
                                 bg-amber-400/10 text-amber-200 border border-amber-400/30
                                 hover:bg-amber-400/20 disabled:opacity-40 disabled:cursor-not-allowed
                                 transition-colors"
                          title="Restore this version as the active file (current primary is backed up first)"
                        >Restore</button>
                      </li>
                    {/each}
                  </ul>
                </div>
              {/if}

              <!-- File list with radios — pick which file the cabinet displays -->
              <div class="glass rounded-xl p-5">
                <div class="flex items-baseline justify-between mb-3">
                  <div class="flex items-center gap-3">
                    <h2 class="font-medium text-zinc-200">Files</h2>
                    {#if selected.has.has("b2s") && (userImages.length > 0 || userVideos.length > 0)}
                      <button
                        type="button"
                        onclick={resetThisTableToB2s}
                        disabled={resetBusy}
                        class="text-[11px] px-2 py-1 rounded
                               bg-rose-500/10 text-rose-200 border border-rose-500/30
                               hover:bg-rose-500/20 disabled:opacity-40 disabled:cursor-not-allowed
                               transition-colors"
                        title="Delete dropped media (mp4/jpg/png/etc.) from PP Doctor mirror AND the Pi. Keeps b2scache, event_map, thumb, glow."
                      >{resetBusy ? "Resetting…" : "Reset to b2s default"}</button>
                    {/if}
                  </div>
                  <span class="text-xs text-zinc-500">Click a row to make it the cabinet default</span>
                </div>

                {#if userImages.length || userVideos.length || selected.has.has("b2s")}
                  <ul class="space-y-0.5">
                    {#if selected.has.has("b2s")}
                      <li>
                        <label class="flex items-center gap-3 px-2 py-2 rounded cursor-pointer hover:bg-white/3 transition-colors {activeFile === '__b2s__' ? 'bg-amber-400/8' : ''}">
                          <input
                            type="radio"
                            name="active-{selected.id}"
                            value="__b2s__"
                            checked={activeFile === "__b2s__"}
                            onchange={() => setActive("__b2s__")}
                            class="text-amber-400 focus:ring-amber-400/40 focus:ring-offset-0 bg-black/40 border-white/20"
                          />
                          <span class="text-[10px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-300 border border-sky-500/20 font-mono font-medium">B2S</span>
                          <span class="flex-1 truncate font-mono text-xs {activeFile === '__b2s__' ? 'text-amber-200' : 'text-zinc-400'}">
                            backglass.directb2s
                          </span>
                          {#if activeFile === "__b2s__"}
                            <span class="text-[10px] text-amber-400 font-medium uppercase tracking-wider">Active</span>
                          {/if}
                        </label>
                      </li>
                    {/if}
                    {#each userImages as f}
                      <li>
                        <label class="flex items-center gap-3 px-2 py-2 rounded cursor-pointer hover:bg-white/3 transition-colors {activeFile === f ? 'bg-amber-400/8' : ''}">
                          <input
                            type="radio"
                            name="active-{selected.id}"
                            value={f}
                            checked={activeFile === f}
                            onchange={() => setActive(f)}
                            class="text-amber-400 focus:ring-amber-400/40 focus:ring-offset-0 bg-black/40 border-white/20"
                          />
                          <span class="text-[10px] px-1.5 py-0.5 rounded bg-emerald-500/10 text-emerald-300 border border-emerald-500/20 font-mono font-medium">IMG</span>
                          <span class="flex-1 truncate font-mono text-xs {activeFile === f ? 'text-amber-200' : 'text-zinc-400'}">
                            {f}
                          </span>
                          {#if isFileSynced("default_image", f)}
                            <span class="text-emerald-400" title="Cached locally">
                              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="2">
                                <path d="M1.5 5 L4 7.5 L8.5 2.5"/>
                              </svg>
                            </span>
                          {:else}
                            <span class="text-zinc-700" title="Not yet cached">
                              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.2">
                                <circle cx="5" cy="5" r="3.5"/>
                              </svg>
                            </span>
                          {/if}
                          {#if activeFile === f}
                            <span class="text-[10px] text-amber-400 font-medium uppercase tracking-wider">Active</span>
                          {:else}
                            <button
                              type="button"
                              onclick={(e) => { e.preventDefault(); setActive(f); }}
                              class="text-[10px] px-1.5 py-0.5 rounded
                                     bg-amber-400/10 text-amber-300 border border-amber-400/30
                                     hover:bg-amber-400/20 transition-colors"
                              title="Make this file the cabinet default"
                            >Set active</button>
                          {/if}
                          <button
                            type="button"
                            onclick={(e) => { e.preventDefault(); deleteFile("default_image", f); }}
                            class="text-[10px] px-1.5 py-0.5 rounded
                                   bg-rose-500/10 text-rose-300 border border-rose-500/30
                                   hover:bg-rose-500/20 transition-colors"
                            title="Delete this file from PP Doctor and the Pi"
                            aria-label="Delete {f}"
                          >×</button>
                        </label>
                      </li>
                    {/each}
                    {#each userVideos as f}
                      <li>
                        <label class="flex items-center gap-3 px-2 py-2 rounded cursor-pointer hover:bg-white/3 transition-colors {activeFile === f ? 'bg-amber-400/8' : ''}">
                          <input
                            type="radio"
                            name="active-{selected.id}"
                            value={f}
                            checked={activeFile === f}
                            onchange={() => setActive(f)}
                            class="text-amber-400 focus:ring-amber-400/40 focus:ring-offset-0 bg-black/40 border-white/20"
                          />
                          <span class="text-[10px] px-1.5 py-0.5 rounded bg-rose-500/10 text-rose-300 border border-rose-500/20 font-mono font-medium">VID</span>
                          <span class="flex-1 truncate font-mono text-xs {activeFile === f ? 'text-amber-200' : 'text-zinc-400'}">
                            {f}
                          </span>
                          {#if isFileSynced("default_video", f)}
                            <span class="text-emerald-400" title="Cached locally">
                              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="2">
                                <path d="M1.5 5 L4 7.5 L8.5 2.5"/>
                              </svg>
                            </span>
                          {:else}
                            <span class="text-zinc-700" title="Not yet cached">
                              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.2">
                                <circle cx="5" cy="5" r="3.5"/>
                              </svg>
                            </span>
                          {/if}
                          {#if activeFile === f}
                            <span class="text-[10px] text-amber-400 font-medium uppercase tracking-wider">Active</span>
                          {:else}
                            <button
                              type="button"
                              onclick={(e) => { e.preventDefault(); setActive(f); }}
                              class="text-[10px] px-1.5 py-0.5 rounded
                                     bg-amber-400/10 text-amber-300 border border-amber-400/30
                                     hover:bg-amber-400/20 transition-colors"
                              title="Make this file the cabinet default"
                            >Set active</button>
                          {/if}
                          <button
                            type="button"
                            onclick={(e) => { e.preventDefault(); deleteFile("default_video", f); }}
                            class="text-[10px] px-1.5 py-0.5 rounded
                                   bg-rose-500/10 text-rose-300 border border-rose-500/30
                                   hover:bg-rose-500/20 transition-colors"
                            title="Delete this video + its thumb from PP Doctor and the Pi"
                            aria-label="Delete {f}"
                          >×</button>
                        </label>
                      </li>
                    {/each}
                  </ul>
                {:else}
                  <div class="text-xs text-zinc-600">No media.</div>
                {/if}

                <div class="mt-4 border-2 border-dashed border-white/8 rounded-lg p-6 text-center text-xs text-zinc-500 hover:border-amber-400/30 hover:text-zinc-300 transition-colors">
                  Drag images or videos anywhere in this window to add to this table
                  <span class="block text-zinc-600 mt-1 leading-snug">
                    Images auto-resize to 1920×1080 + reencode as JPEG ·
                    videos transcode via ffmpeg to 1080p H.264 (24/30 fps) ·
                    previous file backed up to <span class="font-mono">.versions/</span> (≤5)
                  </span>
                </div>
              </div>
            {/if}
          </div>
        {/if}
      </section>
    </div>
  {/if}
</main>

<style>
  /* Photoshop-style marching-ants animation for the wand-selection
     outline. The path inside .ppe-marching-ants has stroke-dasharray
     "3 2" so the dashes are ~3px on, 2px off; animating dashoffset by
     5px (= 3 + 2) over the loop produces the moving-ant effect. */
  :global(.ppe-marching-ants path) {
    animation: ppe-march 0.5s linear infinite;
  }
  @keyframes ppe-march {
    from { stroke-dashoffset: 0; }
    to   { stroke-dashoffset: -5; }
  }

  /* Lamp animation preview — pulsing alpha + blend that approximates the
     Pi's attract animation. The 3s period + ease-in-out feels close to
     the renderer's default attract cycle. Each lamp has a unique
     animation-delay set inline, so they ripple across the video rather
     than all blinking together. screen blend lifts the lamp against
     the underlying video frame without crushing colors. */
  :global(.ppe-lamp-pulse) {
    animation: ppe-pulse 3s ease-in-out infinite;
    mix-blend-mode: screen;
  }
  @keyframes ppe-pulse {
    0%, 100% { opacity: 0.25; }
    50%      { opacity: 1; }
  }

  button:focus-visible {
    outline: 1px solid rgb(251 191 36 / 60%);
    outline-offset: -1px;
  }
</style>
