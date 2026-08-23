<script lang="ts">
  // Layers panel — authors the event_map "particles" / "overlays" arrays.
  // Edits the JSON objects in place (they ARE the saved format); fields left
  // untouched stay absent so the preset default applies on the Pi. Every
  // control shows the effective value (preset default when unset).
  import { FX_PRESETS, PRESET_LABELS, SPRITE_NAMES, resolveEmitter, presetDefaults,
           type FxDoc, type FxEmitterJson, type FxOverlayJson } from "./fx";

  let {
    doc = $bindable<FxDoc>({ particles: [], overlays: [] }),
    selected = $bindable(-1),
    editArea = $bindable(false),
    onTest = null,
    liveStats = null,
  }: {
    doc?: FxDoc;
    selected?: number;
    editArea?: boolean;
    /** Fire a MSG_* into the preview (triggered emitters/overlays). */
    onTest?: ((key: string) => void) | null;
    liveStats?: (() => { live: number; emitters: number; overlays: number }) | null;
  } = $props();

  // Canonical event names = the Pi's commands.json (MSG_BALLSAVED / MSG_BALLDRAIN
  // have no underscore). The renderer triggers FX by this canonical name.
  const EVENTS = [
    "MSG_GAME_START", "MSG_GAMEOVER", "MSG_BALLSAVED", "MSG_BALLDRAIN", "MSG_JACKPOT", "MSG_MULTIBALL",
    "MSG_MULTIBALL_END", "MSG_EXTRABALL", "MSG_SKILLSHOT", "MSG_REPLAY", "MSG_COMBO", "MSG_BONUS", "MSG_LOCK",
    "MSG_LOCK_CLEARED", "MSG_SMACK", "MSG_TILT", "MSG_BALL_LOADED", "MSG_MATCH_RESULT", "MSG_NEW_HIGHSCORE",
    "MSG_CREDITS",
  ];

  let addPreset = $state<string>("starfield");
  let section = $state<"particles" | "overlays">("particles");
  let live = $state({ live: 0, emitters: 0, overlays: 0 });
  $effect(() => {
    const id = setInterval(() => { if (liveStats) live = liveStats(); }, 500);
    return () => clearInterval(id);
  });

  let cur = $derived(selected >= 0 && selected < doc.particles.length ? doc.particles[selected] : null);
  let def = $derived(cur ? presetDefaults(cur.preset ?? "") : presetDefaults(""));
  let eff = $derived(cur ? resolveEmitter(cur) : null);

  function addEmitter() {
    const n = doc.particles.filter(p => p.preset === addPreset).length + 1;
    const e: FxEmitterJson = { name: `${addPreset} ${n}`, preset: addPreset };
    doc.particles.push(e);
    selected = doc.particles.length - 1;
    editArea = false;
  }
  function removeEmitter(i: number) {
    doc.particles.splice(i, 1);
    if (selected >= doc.particles.length) selected = doc.particles.length - 1;
    editArea = false;
  }
  function moveEmitter(i: number, d: number) {
    const j = i + d; if (j < 0 || j >= doc.particles.length) return;
    const t = doc.particles[i]; doc.particles[i] = doc.particles[j]; doc.particles[j] = t;
    if (selected === i) selected = j; else if (selected === j) selected = i;
  }
  function duplicateEmitter(i: number) {
    const c = JSON.parse(JSON.stringify(doc.particles[i])) as FxEmitterJson;
    c.name = (c.name ?? "emitter") + " copy";
    doc.particles.splice(i + 1, 0, c);
    selected = i + 1;
  }
  function changePreset(p: string) {
    if (!cur) return;
    // keep identity + placement + trigger; everything else follows the new preset
    const keep: FxEmitterJson = { name: cur.name, preset: p };
    if (cur.area) keep.area = cur.area;
    if (cur.point) keep.point = cur.point;
    if (cur.trigger !== undefined) keep.trigger = cur.trigger;
    if (cur.burst !== undefined) keep.burst = cur.burst;
    if (cur.duration_ms !== undefined) keep.duration_ms = cur.duration_ms;
    if (cur.z !== undefined) keep.z = cur.z;
    doc.particles[selected] = keep;
  }

  // ── field helpers: write the key only when it differs from the preset default
  function setNum(key: keyof FxEmitterJson, v: number, defVal: number) {
    if (!cur) return;
    if (Number.isNaN(v)) return;
    if (Math.abs(v - defVal) < 1e-9) delete (cur as any)[key]; else (cur as any)[key] = v;
  }
  function setRange(key: keyof FxEmitterJson, lo: number, hi: number, dlo: number, dhi: number) {
    if (!cur) return;
    if (Number.isNaN(lo) || Number.isNaN(hi)) return;
    if (hi < lo) hi = lo;
    if (Math.abs(lo - dlo) < 1e-9 && Math.abs(hi - dhi) < 1e-9) delete (cur as any)[key];
    else (cur as any)[key] = [lo, hi];
  }
  function setStr(key: keyof FxEmitterJson, v: string, defVal: string) {
    if (!cur) return;
    if (v === defVal || v === "") delete (cur as any)[key]; else (cur as any)[key] = v;
  }
  function setBool(key: keyof FxEmitterJson, v: boolean, defVal: boolean) {
    if (!cur) return;
    if (v === defVal) delete (cur as any)[key]; else (cur as any)[key] = v;
  }
  const zName = (s: 0 | 1 | 2) => (["below", "above", "top"] as const)[s];
  const spawnName = (i: number) => ["area", "top", "bottom", "left", "right", "point"][i] ?? "area";
  const alphaName = (i: number) => ["flat", "fade_out", "fade_in_out", "fade_in"][i] ?? "fade_out";

  // trigger UI: "idle" or one event; burst/duration only for events
  let triggerSel = $derived(cur ? (typeof cur.trigger === "string" ? cur.trigger : Array.isArray(cur.trigger) && cur.trigger.length ? cur.trigger[0] : "idle") : "idle");
  function setTrigger(v: string) {
    if (!cur) return;
    if (v === "idle") { delete cur.trigger; delete cur.burst; delete cur.duration_ms; }
    else { cur.trigger = v; if (cur.burst === undefined && cur.duration_ms === undefined) cur.burst = 60; }
  }

  // area UI
  let areaFull = $derived(cur ? !cur.area : true);
  function setAreaFull(full: boolean) {
    if (!cur) return;
    if (full) { delete cur.area; editArea = false; } else { cur.area = [0, 0, 1000, 600]; }
  }
  function setAreaField(i: number, v: number) {
    if (!cur || !cur.area || Number.isNaN(v)) return;
    cur.area[i] = Math.round(v);
  }

  // colors
  const hex = (c: number[]) => "#" + c.slice(0, 3).map(v => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0")).join("");
  const fromHex = (h: string): [number, number, number] => [parseInt(h.slice(1, 3), 16), parseInt(h.slice(3, 5), 16), parseInt(h.slice(5, 7), 16)];
  let ramp = $derived(eff ? eff.ramp : []);
  function setRampColor(i: number, h: string) {
    if (!cur || !eff) return;
    const r = eff.ramp.map(c => [...c] as [number, number, number]);
    r[i] = fromHex(h);
    cur.color = r;
  }
  function addRampStop() { if (!cur || !eff) return; cur.color = [...eff.ramp.map(c => [...c] as [number, number, number]), [...eff.ramp[eff.ramp.length - 1]] as [number, number, number]]; }
  function removeRampStop(i: number) {
    if (!cur || !eff || eff.ramp.length <= 1) return;
    const r = eff.ramp.map(c => [...c] as [number, number, number]); r.splice(i, 1); cur.color = r;
  }
  function resetColors() { if (cur) delete cur.color; }

  // ── overlays
  let oSel = $state(-1);
  let ocur = $derived(oSel >= 0 && oSel < doc.overlays.length ? doc.overlays[oSel] : null);
  function addOverlay() {
    const o: FxOverlayJson = { name: `overlay ${doc.overlays.length + 1}`, image: "fx/overlay.png", rect: [0, 0, 0, 0], z: "above" };
    doc.overlays.push(o); oSel = doc.overlays.length - 1;
  }
  function removeOverlay(i: number) { doc.overlays.splice(i, 1); if (oSel >= doc.overlays.length) oSel = doc.overlays.length - 1; }
  function oRect(i: number, v: number) { if (!ocur || Number.isNaN(v)) return; if (!ocur.rect) ocur.rect = [0, 0, 0, 0]; ocur.rect[i] = Math.round(v); }
  function oToggle(key: "breathe" | "sway" | "bob" | "drift" | "warp", on: boolean) {
    if (!ocur) return;
    if (!on) { delete ocur[key]; return; }
    if (key === "breathe") ocur.breathe = { period_ms: 4000, scale_x: 0, scale_y: 0.02, alpha: 0 };
    if (key === "sway") ocur.sway = { period_ms: 3000, deg: 2 };
    if (key === "bob") ocur.bob = { period_ms: 3000, px: 6 };
    if (key === "drift") ocur.drift = { vx: 20, vy: 0, wrap: true };
    if (key === "warp") ocur.warp = { type: "wave", amp: 6, period_ms: 2500, freq: 1 };
  }
</script>

<aside class="glass rounded-xl p-4 w-72 flex-shrink-0 self-start flex flex-col gap-3 max-h-[calc(100vh-8rem)] overflow-y-auto">
  <div class="flex items-baseline justify-between">
    <h3 class="text-xs uppercase tracking-wider text-zinc-400 font-medium">Layers</h3>
    <span class="text-[10px] font-mono text-zinc-500" title="live particles in the preview">{live.live} live</span>
  </div>

  <!-- section tabs -->
  <div class="flex gap-1 text-[11px]">
    <button type="button" onclick={() => section = "particles"}
      class="flex-1 px-2 py-1 rounded border {section === 'particles' ? 'bg-amber-400/15 text-amber-200 border-amber-400/40' : 'bg-white/5 text-zinc-400 border-white/10 hover:bg-white/10'}">Particles ({doc.particles.length})</button>
    <button type="button" onclick={() => section = "overlays"}
      class="flex-1 px-2 py-1 rounded border {section === 'overlays' ? 'bg-amber-400/15 text-amber-200 border-amber-400/40' : 'bg-white/5 text-zinc-400 border-white/10 hover:bg-white/10'}">Overlays ({doc.overlays.length})</button>
  </div>

  {#if section === "particles"}
    <!-- add -->
    <div class="flex gap-1">
      <select bind:value={addPreset} class="flex-1 text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200">
        {#each FX_PRESETS as p}<option value={p}>{PRESET_LABELS[p] ?? p}</option>{/each}
      </select>
      <button type="button" onclick={addEmitter}
        class="text-[11px] px-2 py-1 rounded bg-amber-400/15 text-amber-200 border border-amber-400/40 hover:bg-amber-400/25">+ Add</button>
    </div>

    <!-- list -->
    {#if doc.particles.length === 0}
      <div class="text-[10px] text-zinc-500 leading-snug">No particle layers yet. Pick a preset and Add — it previews here exactly as the cabinet renders it (same engine, same presets).</div>
    {:else}
      <ul class="flex flex-col gap-0.5">
        {#each doc.particles as p, i}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
          <li class="flex items-center gap-1 text-[11px] rounded px-1.5 py-1 cursor-pointer
                     {i === selected ? 'bg-amber-400/15 text-amber-100' : 'hover:bg-white/5 text-zinc-300'}"
              onclick={() => { selected = i; editArea = false; }}>
            <input type="checkbox" checked={p.enabled !== false} class="accent-amber-400"
                   onclick={(e) => e.stopPropagation()}
                   onchange={(e) => { const on = (e.currentTarget as HTMLInputElement).checked; if (on) delete p.enabled; else p.enabled = false; }}
                   title="enabled" />
            <span class="flex-1 truncate">{p.name ?? `emitter ${i}`} <span class="text-zinc-500">· {p.preset ?? "custom"}</span></span>
            <button type="button" class="text-zinc-500 hover:text-zinc-200 px-0.5" title="move up" onclick={(e) => { e.stopPropagation(); moveEmitter(i, -1); }}>▲</button>
            <button type="button" class="text-zinc-500 hover:text-zinc-200 px-0.5" title="move down" onclick={(e) => { e.stopPropagation(); moveEmitter(i, 1); }}>▼</button>
            <button type="button" class="text-zinc-500 hover:text-zinc-200 px-0.5" title="duplicate" onclick={(e) => { e.stopPropagation(); duplicateEmitter(i); }}>⧉</button>
            <button type="button" class="text-zinc-500 hover:text-red-300 px-0.5" title="remove" onclick={(e) => { e.stopPropagation(); removeEmitter(i); }}>✕</button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if cur && eff}
      <div class="border-t border-white/5 pt-3 flex flex-col gap-3">
        <!-- identity -->
        <div class="flex gap-1">
          <input type="text" value={cur.name ?? ""} placeholder="name"
                 oninput={(e) => { cur!.name = (e.currentTarget as HTMLInputElement).value; }}
                 class="flex-1 text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200" />
          <select value={cur.preset ?? ""} onchange={(e) => changePreset((e.currentTarget as HTMLSelectElement).value)}
                  class="text-xs bg-black/30 border border-white/10 rounded px-1 py-1 text-zinc-200 w-24">
            {#each FX_PRESETS as p}<option value={p}>{p}</option>{/each}
          </select>
        </div>

        <!-- z + trigger -->
        <div class="grid grid-cols-2 gap-2">
          <div>
            <label class="text-[10px] text-zinc-400" for="fx-z">Layer</label>
            <select id="fx-z" value={cur.z ?? zName(def.slot)} onchange={(e) => setStr("z", (e.currentTarget as HTMLSelectElement).value, zName(def.slot))}
                    class="w-full text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200">
              <option value="below">Below lamps (behind)</option>
              <option value="above">Above lamps</option>
              <option value="top">Top</option>
            </select>
          </div>
          <div>
            <label class="text-[10px] text-zinc-400" for="fx-trigger">Runs</label>
            <select id="fx-trigger" value={triggerSel} onchange={(e) => setTrigger((e.currentTarget as HTMLSelectElement).value)}
                    class="w-full text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200">
              <option value="idle">Always (idle)</option>
              {#each EVENTS as ev}<option value={ev}>{ev.replace("MSG_", "on ")}</option>{/each}
            </select>
          </div>
        </div>
        {#if triggerSel !== "idle"}
          <div class="grid grid-cols-2 gap-2">
            <div>
              <div class="flex items-baseline justify-between"><label class="text-[10px] text-zinc-400" for="fx-burst">Burst</label><span class="text-[10px] font-mono text-zinc-500">{cur.burst ?? 0}</span></div>
              <input id="fx-burst" type="range" min="0" max="300" step="1" value={cur.burst ?? 0}
                     oninput={(e) => { cur!.burst = +(e.currentTarget as HTMLInputElement).value; }} class="w-full accent-amber-400" />
            </div>
            <div>
              <div class="flex items-baseline justify-between"><label class="text-[10px] text-zinc-400" for="fx-dur">Window</label><span class="text-[10px] font-mono text-zinc-500">{cur.duration_ms ?? 0} ms</span></div>
              <input id="fx-dur" type="range" min="0" max="10000" step="100" value={cur.duration_ms ?? 0}
                     oninput={(e) => { const v = +(e.currentTarget as HTMLInputElement).value; if (v > 0) cur!.duration_ms = v; else delete cur!.duration_ms; }} class="w-full accent-amber-400" />
            </div>
          </div>
          <button type="button" onclick={() => onTest?.(triggerSel)}
                  class="text-[11px] px-2 py-1 rounded bg-white/5 text-zinc-200 border border-white/10 hover:bg-white/10">▶ Test {triggerSel.replace("MSG_", "")}</button>
        {/if}

        <!-- area -->
        <div>
          <div class="flex items-center justify-between">
            <label class="text-[10px] text-zinc-400 flex items-center gap-1">
              <input type="checkbox" checked={areaFull} onchange={(e) => setAreaFull((e.currentTarget as HTMLInputElement).checked)} class="accent-amber-400" /> Full screen area
            </label>
            {#if !areaFull}
              <button type="button" onclick={() => editArea = !editArea}
                      class="text-[10px] px-1.5 py-0.5 rounded border {editArea ? 'bg-amber-400/20 text-amber-200 border-amber-400/50' : 'bg-white/5 text-zinc-300 border-white/10 hover:bg-white/10'}"
                      title="Drag a rectangle on the preview">{editArea ? "drawing… drag on preview" : "Draw on preview"}</button>
            {/if}
          </div>
          {#if !areaFull && cur.area}
            <div class="grid grid-cols-4 gap-1 mt-1">
              {#each ["x", "y", "w", "h"] as k, i}
                <label class="text-[9px] text-zinc-500">{k}
                  <input type="number" value={cur.area[i]} oninput={(e) => setAreaField(i, +(e.currentTarget as HTMLInputElement).value)}
                         class="w-full text-[11px] bg-black/30 border border-white/10 rounded px-1 py-0.5 text-zinc-200 font-mono" />
                </label>
              {/each}
            </div>
          {/if}
          <div class="grid grid-cols-2 gap-2 mt-1">
            <div>
              <label class="text-[10px] text-zinc-400" for="fx-spawn">Spawn from</label>
              <select id="fx-spawn" value={cur.spawn ?? spawnName(def.spawnMode)} onchange={(e) => setStr("spawn", (e.currentTarget as HTMLSelectElement).value, spawnName(def.spawnMode))}
                      class="w-full text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200">
                <option value="area">anywhere in area</option><option value="top">top edge</option><option value="bottom">bottom edge</option>
                <option value="left">left edge</option><option value="right">right edge</option><option value="point">center point</option>
              </select>
            </div>
            <div>
              <label class="text-[10px] text-zinc-400 flex items-center gap-1 mt-4">
                <input type="checkbox" checked={cur.wrap ?? def.wrap} onchange={(e) => setBool("wrap", (e.currentTarget as HTMLInputElement).checked, def.wrap)} class="accent-amber-400" /> Wrap around
              </label>
            </div>
          </div>
        </div>

        <!-- motion sliders -->
        {#snippet slider(label: string, id: string, value: number, min: number, max: number, step: number, fmt: string, set: (v: number) => void)}
          <div>
            <div class="flex items-baseline justify-between"><label class="text-[10px] text-zinc-400" for={id}>{label}</label><span class="text-[10px] font-mono text-zinc-500">{fmt}</span></div>
            <input {id} type="range" {min} {max} {step} {value} oninput={(e) => set(+(e.currentTarget as HTMLInputElement).value)} class="w-full accent-amber-400" />
          </div>
        {/snippet}

        {@render slider("Count", "fx-max", eff.maxN, 1, 300, 1, `${eff.maxN}`, (v) => setNum("max", v, def.maxN))}
        {@render slider("Rate", "fx-rate", eff.rate, 0, 300, 1, `${eff.rate}/s`, (v) => setNum("rate", v, def.rate))}
        <div class="grid grid-cols-2 gap-2">
          {@render slider("Speed min", "fx-spmin", eff.speedMin, 0, 2000, 1, `${Math.round(eff.speedMin)}`, (v) => setRange("speed", v, Math.max(v, eff!.speedMax), def.speedMin, def.speedMax))}
          {@render slider("Speed max", "fx-spmax", eff.speedMax, 0, 2000, 1, `${Math.round(eff.speedMax)} px/s`, (v) => setRange("speed", Math.min(v, eff!.speedMin), v, def.speedMin, def.speedMax))}
        </div>
        <div class="grid grid-cols-2 gap-2">
          {@render slider("Direction", "fx-angle", eff.angle, 0, 360, 1, `${eff.angle}° ${eff.angle === 0 ? "→" : eff.angle === 90 ? "↓" : eff.angle === 180 ? "←" : eff.angle === 270 ? "↑" : ""}`, (v) => setNum("angle", v, def.angle))}
          {@render slider("Spread", "fx-spread", eff.spread, 0, 180, 1, `±${eff.spread}°`, (v) => setNum("spread", v, def.spread))}
        </div>
        <div class="grid grid-cols-2 gap-2">
          {@render slider("Size min", "fx-szmin", eff.sizeMin, 1, 200, 1, `${Math.round(eff.sizeMin)}`, (v) => setRange("size", v, Math.max(v, eff!.sizeMax), def.sizeMin, def.sizeMax))}
          {@render slider("Size max", "fx-szmax", eff.sizeMax, 1, 200, 1, `${Math.round(eff.sizeMax)} px`, (v) => setRange("size", Math.min(v, eff!.sizeMin), v, def.sizeMin, def.sizeMax))}
        </div>
        <div class="grid grid-cols-2 gap-2">
          {@render slider("Life min", "fx-lfmin", eff.lifeMin, 0, 20, 0.1, `${eff.lifeMin.toFixed(1)}s`, (v) => setRange("life_s", v, Math.max(v, eff!.lifeMax), def.lifeMin, def.lifeMax))}
          {@render slider("Life max", "fx-lfmax", eff.lifeMax, 0, 20, 0.1, `${eff.lifeMax.toFixed(1)}s (0=∞)`, (v) => setRange("life_s", Math.min(v, eff!.lifeMin), v, def.lifeMin, def.lifeMax))}
        </div>
        {@render slider("Brightness", "fx-alpha", eff.alphaMax, 0, 1, 0.01, eff.alphaMax.toFixed(2), (v) => setNum("alpha_max", v, def.alphaMax))}
        <div class="grid grid-cols-2 gap-2">
          {@render slider("Twinkle", "fx-tw", eff.twinkleAmt, 0, 1, 0.01, eff.twinkleAmt.toFixed(2), (v) => setNum("twinkle", v, def.twinkleAmt))}
          {@render slider("Twinkle Hz", "fx-twhz", eff.twinkleHz, 0, 10, 0.1, `${eff.twinkleHz.toFixed(1)} Hz`, (v) => setNum("twinkle_hz", v, def.twinkleHz))}
        </div>
        <div class="grid grid-cols-2 gap-2">
          {@render slider("Gravity", "fx-grav", eff.gravity, -500, 2000, 5, `${eff.gravity}`, (v) => setNum("gravity", v, def.gravity))}
          {@render slider("Drag", "fx-drag", eff.drag, 0, 1, 0.01, eff.drag.toFixed(2), (v) => setNum("drag", v, def.drag))}
        </div>
        <div class="grid grid-cols-2 gap-2">
          {@render slider("Wander", "fx-wander", eff.wander, 0, 300, 1, `${eff.wander}`, (v) => setNum("wander", v, def.wander))}
          {@render slider("Grow", "fx-grow", eff.grow, -1, 3, 0.05, `${eff.grow > 0 ? "+" : ""}${Math.round(eff.grow * 100)}%`, (v) => setNum("grow", v, def.grow))}
        </div>

        <!-- look -->
        <div class="grid grid-cols-3 gap-2">
          <div>
            <label class="text-[10px] text-zinc-400" for="fx-sprite">Sprite</label>
            <select id="fx-sprite" value={cur.sprite ?? SPRITE_NAMES[def.sprite]} onchange={(e) => setStr("sprite", (e.currentTarget as HTMLSelectElement).value, SPRITE_NAMES[def.sprite])}
                    class="w-full text-xs bg-black/30 border border-white/10 rounded px-1 py-1 text-zinc-200">
              {#each SPRITE_NAMES as s}<option value={s}>{s}</option>{/each}
            </select>
          </div>
          <div>
            <label class="text-[10px] text-zinc-400" for="fx-blend">Blend</label>
            <select id="fx-blend" value={cur.blend ?? def.blend} onchange={(e) => setStr("blend", (e.currentTarget as HTMLSelectElement).value, def.blend)}
                    class="w-full text-xs bg-black/30 border border-white/10 rounded px-1 py-1 text-zinc-200">
              <option value="add">add (glow)</option><option value="blend">blend</option>
            </select>
          </div>
          <div>
            <label class="text-[10px] text-zinc-400" for="fx-fade">Fade</label>
            <select id="fx-fade" value={cur.alpha ?? alphaName(def.alphaCurve)} onchange={(e) => setStr("alpha", (e.currentTarget as HTMLSelectElement).value, alphaName(def.alphaCurve))}
                    class="w-full text-xs bg-black/30 border border-white/10 rounded px-1 py-1 text-zinc-200">
              <option value="flat">flat</option><option value="fade_out">fade out</option><option value="fade_in_out">in/out</option><option value="fade_in">fade in</option>
            </select>
          </div>
        </div>
        <div>
          <div class="flex items-baseline justify-between">
            <span class="text-[10px] text-zinc-400">Color over life</span>
            <span class="flex gap-1">
              <button type="button" onclick={addRampStop} class="text-[10px] text-zinc-400 hover:text-zinc-200" title="add stop">+</button>
              <button type="button" onclick={resetColors} class="text-[10px] text-zinc-400 hover:text-zinc-200" title="preset colors">reset</button>
            </span>
          </div>
          <div class="flex gap-1 mt-1">
            {#each ramp as c, i}
              <span class="flex flex-col items-center gap-0.5">
                <input type="color" value={hex(c)} oninput={(e) => setRampColor(i, (e.currentTarget as HTMLInputElement).value)} class="w-8 h-6 bg-transparent border border-white/10 rounded cursor-pointer" />
                {#if ramp.length > 1}<button type="button" onclick={() => removeRampStop(i)} class="text-[9px] text-zinc-500 hover:text-red-300">✕</button>{/if}
              </span>
            {/each}
          </div>
        </div>
        <div class="text-[9px] text-zinc-600 leading-snug">Source-pixel units, same as bulbs. Unchanged fields are not written — the cabinet applies the preset default.</div>
      </div>
    {/if}

  {:else}
    <!-- overlays -->
    <button type="button" onclick={addOverlay}
      class="text-[11px] px-2 py-1 rounded bg-amber-400/15 text-amber-200 border border-amber-400/40 hover:bg-amber-400/25">+ Add image overlay</button>
    {#if doc.overlays.length === 0}
      <div class="text-[10px] text-zinc-500 leading-snug">PNG cut-outs with breathing / sway / bob / drift / warp. Put the PNG under the table's <span class="font-mono">default_image/</span> folder and reference it by relative path (e.g. <span class="font-mono">fx/ship.png</span>). A cut-out at "above" with an emitter at "below" keeps particles behind the character.</div>
    {:else}
      <ul class="flex flex-col gap-0.5">
        {#each doc.overlays as o, i}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
          <li class="flex items-center gap-1 text-[11px] rounded px-1.5 py-1 cursor-pointer {i === oSel ? 'bg-amber-400/15 text-amber-100' : 'hover:bg-white/5 text-zinc-300'}"
              onclick={() => oSel = i}>
            <input type="checkbox" checked={o.enabled !== false} class="accent-amber-400" onclick={(e) => e.stopPropagation()}
                   onchange={(e) => { const on = (e.currentTarget as HTMLInputElement).checked; if (on) delete o.enabled; else o.enabled = false; }} />
            <span class="flex-1 truncate">{o.name ?? `overlay ${i}`} <span class="text-zinc-500">· {o.image}</span></span>
            <button type="button" class="text-zinc-500 hover:text-red-300 px-0.5" title="remove" onclick={(e) => { e.stopPropagation(); removeOverlay(i); }}>✕</button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if ocur}
      <div class="border-t border-white/5 pt-3 flex flex-col gap-2">
        <input type="text" value={ocur.name ?? ""} placeholder="name" oninput={(e) => { ocur!.name = (e.currentTarget as HTMLInputElement).value; }}
               class="text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200" />
        <label class="text-[10px] text-zinc-400">Image (relative to default_image/)
          <input type="text" value={ocur.image} oninput={(e) => { ocur!.image = (e.currentTarget as HTMLInputElement).value; }}
                 class="w-full text-xs bg-black/30 border border-white/10 rounded px-2 py-1 text-zinc-200 font-mono" />
        </label>
        <div class="grid grid-cols-4 gap-1">
          {#each ["x", "y", "w", "h"] as k, i}
            <label class="text-[9px] text-zinc-500">{k}
              <input type="number" value={ocur.rect?.[i] ?? 0} oninput={(e) => oRect(i, +(e.currentTarget as HTMLInputElement).value)}
                     class="w-full text-[11px] bg-black/30 border border-white/10 rounded px-1 py-0.5 text-zinc-200 font-mono" />
            </label>
          {/each}
        </div>
        <div class="text-[9px] text-zinc-600">w/h 0 = image's native size (source px).</div>
        <div class="grid grid-cols-3 gap-2">
          <div>
            <label class="text-[10px] text-zinc-400" for="ov-z">Layer</label>
            <select id="ov-z" value={ocur.z ?? "above"} onchange={(e) => { ocur!.z = (e.currentTarget as HTMLSelectElement).value as any; }}
                    class="w-full text-xs bg-black/30 border border-white/10 rounded px-1 py-1 text-zinc-200">
              <option value="below">below</option><option value="above">above</option><option value="top">top</option>
            </select>
          </div>
          <div>
            <label class="text-[10px] text-zinc-400" for="ov-blend">Blend</label>
            <select id="ov-blend" value={ocur.blend ?? "blend"} onchange={(e) => { ocur!.blend = (e.currentTarget as HTMLSelectElement).value as any; }}
                    class="w-full text-xs bg-black/30 border border-white/10 rounded px-1 py-1 text-zinc-200">
              <option value="blend">blend</option><option value="add">add</option>
            </select>
          </div>
          <div>
            <div class="flex items-baseline justify-between"><label class="text-[10px] text-zinc-400" for="ov-alpha">Alpha</label><span class="text-[10px] font-mono text-zinc-500">{(ocur.alpha ?? 1).toFixed(2)}</span></div>
            <input id="ov-alpha" type="range" min="0" max="1" step="0.01" value={ocur.alpha ?? 1} oninput={(e) => { ocur!.alpha = +(e.currentTarget as HTMLInputElement).value; }} class="w-full accent-amber-400" />
          </div>
        </div>
        <div class="grid grid-cols-2 gap-1">
          <label class="text-[9px] text-zinc-500">pivot x (0–1)
            <input type="number" step="0.05" min="0" max="1" value={ocur.pivot?.[0] ?? 0.5} oninput={(e) => { ocur!.pivot = [+(e.currentTarget as HTMLInputElement).value, ocur!.pivot?.[1] ?? 0.5]; }}
                   class="w-full text-[11px] bg-black/30 border border-white/10 rounded px-1 py-0.5 text-zinc-200 font-mono" /></label>
          <label class="text-[9px] text-zinc-500">pivot y (0–1)
            <input type="number" step="0.05" min="0" max="1" value={ocur.pivot?.[1] ?? 0.5} oninput={(e) => { ocur!.pivot = [ocur!.pivot?.[0] ?? 0.5, +(e.currentTarget as HTMLInputElement).value]; }}
                   class="w-full text-[11px] bg-black/30 border border-white/10 rounded px-1 py-0.5 text-zinc-200 font-mono" /></label>
        </div>

        <!-- sprite-sheet frames -->
        <label class="text-[10px] text-zinc-300 flex items-center gap-1">
          <input type="checkbox" checked={!!ocur.frames} onchange={(e) => { if ((e.currentTarget as HTMLInputElement).checked) ocur!.frames = { cols: 6, rows: 5, count: 30, fps: 15 }; else delete ocur!.frames; }} class="accent-amber-400" /> Animated sprite sheet
        </label>
        {#if ocur.frames}
          <div class="grid grid-cols-4 gap-1 pl-4">
            {@render num("cols", ocur.frames, "cols", 1, 32, 1)}
            {@render num("rows", ocur.frames, "rows", 1, 32, 1)}
            {@render num("count", ocur.frames, "count", 1, 1024, 1)}
            {@render num("fps", ocur.frames, "fps", 1, 60, 1)}
          </div>
        {/if}

        {#snippet motion(label: string, key: "breathe" | "sway" | "bob" | "drift" | "warp")}
          <label class="text-[10px] text-zinc-300 flex items-center gap-1">
            <input type="checkbox" checked={!!ocur?.[key]} onchange={(e) => oToggle(key, (e.currentTarget as HTMLInputElement).checked)} class="accent-amber-400" /> {label}
          </label>
        {/snippet}
        {#snippet num(label: string, obj: any, key: string, min: number, max: number, step: number)}
          <label class="text-[9px] text-zinc-500">{label}
            <input type="number" {min} {max} {step} value={obj[key]} oninput={(e) => { obj[key] = +(e.currentTarget as HTMLInputElement).value; }}
                   class="w-full text-[11px] bg-black/30 border border-white/10 rounded px-1 py-0.5 text-zinc-200 font-mono" /></label>
        {/snippet}

        {@render motion("Breathe (stretch)", "breathe")}
        {#if ocur.breathe}
          <div class="grid grid-cols-4 gap-1 pl-4">
            {@render num("period ms", ocur.breathe, "period_ms", 200, 20000, 100)}
            {@render num("scale x", ocur.breathe, "scale_x", -0.5, 0.5, 0.005)}
            {@render num("scale y", ocur.breathe, "scale_y", -0.5, 0.5, 0.005)}
            {@render num("alpha dip", ocur.breathe, "alpha", 0, 1, 0.05)}
          </div>
        {/if}
        {@render motion("Sway (rotate)", "sway")}
        {#if ocur.sway}
          <div class="grid grid-cols-2 gap-1 pl-4">
            {@render num("period ms", ocur.sway, "period_ms", 200, 20000, 100)}
            {@render num("degrees", ocur.sway, "deg", -45, 45, 0.5)}
          </div>
        {/if}
        {@render motion("Bob (bounce)", "bob")}
        {#if ocur.bob}
          <div class="grid grid-cols-2 gap-1 pl-4">
            {@render num("period ms", ocur.bob, "period_ms", 200, 20000, 100)}
            {@render num("px", ocur.bob, "px", -200, 200, 1)}
          </div>
        {/if}
        {@render motion("Drift (move)", "drift")}
        {#if ocur.drift}
          <div class="grid grid-cols-3 gap-1 pl-4">
            {@render num("vx px/s", ocur.drift, "vx", -2000, 2000, 1)}
            {@render num("vy px/s", ocur.drift, "vy", -2000, 2000, 1)}
            <label class="text-[9px] text-zinc-500 flex items-end gap-1 pb-1"><input type="checkbox" checked={!!ocur.drift.wrap} onchange={(e) => { ocur!.drift!.wrap = (e.currentTarget as HTMLInputElement).checked; }} class="accent-amber-400" /> wrap</label>
          </div>
        {/if}
        {@render motion("Warp (mesh)", "warp")}
        {#if ocur.warp}
          <div class="grid grid-cols-4 gap-1 pl-4">
            <label class="text-[9px] text-zinc-500">type
              <select value={ocur.warp.type ?? "wave"} onchange={(e) => { ocur!.warp!.type = (e.currentTarget as HTMLSelectElement).value as any; }}
                      class="w-full text-[11px] bg-black/30 border border-white/10 rounded px-1 py-0.5 text-zinc-200">
                <option value="wave">wave</option><option value="flag">flag</option><option value="bulge">bulge</option><option value="jelly">jelly</option>
              </select></label>
            {@render num("amp px", ocur.warp, "amp", 0, 200, 1)}
            {@render num("period ms", ocur.warp, "period_ms", 100, 20000, 100)}
            {@render num("freq", ocur.warp, "freq", 0.1, 8, 0.1)}
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</aside>
