<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { parseDirectB2S, parseEventMap, selectMotionBulbs, bulbAlpha,
           type EventMap } from "./b2s";
  import { parseB2SCache } from "./b2s_cache";
  import { bloomState } from "./bloom.svelte";

  /** Accept EITHER:
   *  - xml (.directb2s source — full fidelity, dev-only on most user setups)
   *  - cacheBuf (.b2scache binary — what every Pi has, what end users will use)
   *  At least one must be provided. xml wins if both present.
   *
   *  baseBrightness is a PP-Doctor-local preview tuning (NOT pushed to Pi).
   *  Some tables author the base art dim by design (e.g. Bally late-70s LEDs);
   *  the user can brighten the base in preview to see bulb effects more
   *  clearly while authoring event_map tweaks. 1.0 = unmodified.  Applied
   *  only to the base bitmap; bulbs render with their authored brightness. */
  let {
    xml = "",
    cacheBuf = null,
    eventMapJson = null,
    baseBrightness = 1.0,
    overrideMinAlpha = null,
    overrideMaxAlpha = null,
    overrideCycleSeconds = null,
    overrideMotion = null,
    overrideTail = null,
  }: {
    xml?: string;
    cacheBuf?: ArrayBuffer | null;
    eventMapJson?: string | null;
    baseBrightness?: number;
    /** Per-preview overrides for the FxB layer pulse. When non-null, these
     *  win over what's in event_map.json so the user can tune live without
     *  committing to the cabinet. */
    overrideMinAlpha?: number | null;
    overrideMaxAlpha?: number | null;
    overrideCycleSeconds?: number | null;
    /** Per-preview override for attract motion + runner tail. */
    overrideMotion?: import("$lib/b2s").AttractMotion | null;
    overrideTail?: number | null;
  } = $props();

  let canvas = $state<HTMLCanvasElement | undefined>();
  let parseError = $state("");

  // Unified internal render state — both xml and cache parsers populate this.
  type RenderBulb = {
    /** Bulb center coords (for wave-phase math) */
    cx: number; cy: number;
    /** Logical w/h (for wave-phase boundaries) */
    lw: number; lh: number;
    /** Where to actually drawImage the sprite */
    dx: number; dy: number; dw: number; dh: number;
    /** Decoded lit image — works whether HTMLImageElement or ImageBitmap */
    sprite: HTMLImageElement | ImageBitmap | null;
    romId: number;
    /** Name attribute — used by <Animations> on/off_names to address bulbs. */
    name: string;
    color: [number, number, number];
    /** Full-backglass GI/animation overlay — draw with single pass to avoid
     *  oversaturating the entire base when stack-blits multiply brightness. */
    isSnippit: boolean;
    /** Rotator fields (SnippitType=2 — SafeCracker wheel, Scared Stiff spider).
     *  Pi rotates sprite around its center by
     *    angle = (360/rotating_steps) * current_step;  if direction==0 negate;
     *    angle += 180 (PBA mStartRot.x rest pose offset)
     *  current_step is driven by MSG_MECH_STATE during gameplay; in attract
     *  preview we hold step 0 (rest pose only). renderer.cpp:4892-4900. */
    snippitType: number;
    rotatingSteps: number;
    rotatingDirection: number;
    currentStep: number;
    /** Lit state — set by animations via on/off names. False bulbs are
     *  drawn by attract motion only; true bulbs render at peak alpha
     *  (animation has authority over motion). */
    isLit: boolean;
  };
  /** Active animation runtime state — direct port of Pi's
   *  `Renderer::ActiveAnimation` (renderer.cpp:5522-5529 + 5538-5589). */
  type ActiveAnim = {
    animIdx: number;
    stepIdx: number;
    /** 0 = currently in post-On wait (next: do Off); 1 = post-Off wait (next: advance step) */
    phase: 0 | 1;
    waitRemaining: number;        // in interval-ticks
    loopsRemaining: number;       // -1 = infinite
    lastTickMs: number;
  };
  // $state so the bottom debug overlay re-renders as the parse progresses;
  // without this the {#if base && evt?.attract} block only re-evaluates when
  // evt changes (because base was a plain `let`), and we couldn't tell from
  // the UI alone whether the bulb step had executed.
  let base = $state<HTMLImageElement | ImageBitmap | null>(null);
  let baseW = $state(1920);
  let baseH = $state(1080);
  /** Grill/DMD strip pixels at the bottom of source coords that should be
   *  hidden — mirrors Pi renderer's `useH = sourceHeight - grillHeight` crop
   *  (renderer.cpp:5168-5192). Tables like BALLY Attack from Mars (grill=377)
   *  author the DMD/speakers inside the base image; without this crop the
   *  strip leaks through under the actual backglass. */
  let grillH = $state(0);
  /** `true` when `base` is the raw uncropped source PNG (decoded from
   *  .directb2s XML — needs a srcRect crop at draw time). `false` when `base`
   *  is the pre-cropped+stretched 1920×1080 bitmap from .b2scache (writer
   *  already applied the crop, srcRect should be the full bitmap). Tracking
   *  this avoids double-cropping the b2scache path. */
  let baseNeedsCrop = false;
  let bulbs = $state<RenderBulb[]>([]);
  let bulbsLoaded = $state(0);   // for the "sprites N/N" footer
  // Score reels — used to extend useH when they're authored below the
  // visible backglass (renderer.cpp:5179-5182, Cactus Jack / Sword of Fury).
  // $state so the diagnostic-strip count updates when the parse completes;
  // also silences Svelte 5 non-reactive-update warnings on reassignment.
  let scores = $state<{ y: number; h: number }[]>([]);
  /** Animations from .directb2s `<Animations>` block. Read by both XML and
   *  cache paths. Pi plays via b2sStartAnimation + b2sTickAnimations. */
  let animations = $state<import("$lib/b2s").B2SAnimation[]>([]);
  /** Active animation states (one per started animation). */
  let activeAnims = $state<ActiveAnim[]>([]);
  /** Index from bulb name → bulb indices in `bulbs`. Built when bulbs are
   *  set. Animations reference bulbs by name (CSV in On/Off attrs); multiple
   *  bulbs can share a name. Not template-rendered so plain `let` is fine,
   *  but keep $state to silence reassign warnings during dev. */
  let bulbsByName = $state(new Map<string, number[]>());

  let evt = $state<EventMap | null>(null);
  let motionIdxs = $state<number[]>([]);
  let motionBounds = { minX: 0, spanX: 1, minY: 0, spanY: 1 };
  /** Reverse lookup: bulb index → position within motionIdxs (-1 if not).
   *  Precomputed when motionIdxs changes so the draw loop avoids O(N²)
   *  indexOf calls (was rebuilding a Set + indexOf per-bulb per-frame). */
  let motionPos: Int32Array = new Int32Array(0);

  let raf = 0;
  let startTs = 0;
  // Frame-time telemetry — rolling samples logged once per second so the log
  // doesn't drown. Captures min/avg/max plus per-stage breakdowns. Lets us
  // see whether choppy = real fps drop vs. just brightness banding.
  let frameSamples: number[] = [];
  let frameStageBase = 0;
  let frameStageBulbs = 0;
  let frameStageCompose = 0;
  let lastFrameLog = 0;
  let lastFrameDrawn = 0;     // for "drew=N bulbs" telemetry
  let lastFrameSkipped = 0;
  // Offscreen "FxB layer" — mirrors the Pi renderer's bulb-compositing layer
  // (see renderer.cpp: layers_ FxB at LayerId::FxB). Bulbs are stack-blitted
  // here with SOURCE-OVER, alphas accumulate via standard porter-duff (1 -
  // (1-α)^N) without summing RGB — same as Pi's SDL_BLENDMODE_BLEND. The
  // assembled layer is then composited onto the main canvas additively
  // (LIGHTER), matching the Pi's final FxB→base composite.
  let fxCanvas: HTMLCanvasElement | null = null;

  // Pause the rAF loop when the document is hidden (PP Doctor backgrounded
  // behind another app). The 60fps render loop otherwise burns CPU even
  // when nothing is visible. Resume when document becomes visible again.
  let visibilityHandler: (() => void) | null = null;
  onMount(() => {
    parseAll();
    visibilityHandler = () => {
      if (document.hidden) {
        if (raf) { cancelAnimationFrame(raf); raf = 0; }
      } else if (!raf && base) {
        startLoop();
      }
    };
    document.addEventListener("visibilitychange", visibilityHandler);
  });
  onDestroy(() => {
    if (raf) cancelAnimationFrame(raf);
    if (visibilityHandler) document.removeEventListener("visibilitychange", visibilityHandler);
  });

  // Track what we've already parsed so we don't redo cache decode just
  // because eventMapJson updated (the $effect re-fires on EACH prop change,
  // and +page.svelte writes b2sXml + b2sCacheBuf + b2sEventMapJson in
  // sequence — that's 3 state writes → 3 effect runs → 3 parseAll calls
  // per table click. Cache parse for a 10MB file is 500-700ms, so 3x = 2s
  // of wasted work). Source change → full re-parse. event_map-only change
  // → just rebuild attract config + motion.
  let lastXml: string | null = null;
  let lastCacheBuf: ArrayBuffer | null | undefined = undefined;
  let lastEventMap: string | null | undefined = undefined;

  // Rebuild motion table when override changes — cheap (O(N log N) sort
  // over ≤300 motion bulbs). Doesn't re-parse the cache; just updates the
  // attract spec and re-ranks for RUNNER.
  let lastOverrideMotion: typeof overrideMotion = null;
  let lastOverrideTail: typeof overrideTail = null;
  $effect(() => {
    if (overrideMotion === lastOverrideMotion && overrideTail === lastOverrideTail) return;
    lastOverrideMotion = overrideMotion;
    lastOverrideTail   = overrideTail;
    if (evt && bulbs.length > 0) rebuildMotion();
  });

  $effect(() => {
    const xmlChanged = xml !== lastXml;
    const cacheChanged = cacheBuf !== lastCacheBuf;
    const eventMapChanged = eventMapJson !== lastEventMap;
    lastXml = xml; lastCacheBuf = cacheBuf; lastEventMap = eventMapJson;

    if (xmlChanged || cacheChanged) {
      parseAll();   // full re-parse: source decode + event_map + motion
    } else if (eventMapChanged && bulbs.length > 0) {
      // Source identical — just refresh event-map-derived state (cheap, sync).
      const allRomIds = bulbs.map(b => ({ romId: b.romId, id: b.romId, x: b.cx - b.lw / 2, y: b.cy - b.lh / 2, width: b.lw, height: b.lh, color: b.color, litB64: "" }));
      evt = parseEventMap(eventMapJson ?? null, allRomIds as any);
      rebuildMotion();
    }
  });

  async function parseAll() {
    parseError = "";
    base = null;
    bulbs = [];
    bulbsLoaded = 0;
    scores = [];
    animations = [];
    activeAnims = [];
    bulbsByName = new Map();
    if (raf) cancelAnimationFrame(raf);

    try {
      if (xml) {
        await parseFromXml();
      } else if (cacheBuf) {
        await parseFromCache();
      } else {
        return; // nothing to parse
      }
      // Build event-map driven attract motion config
      const allRomIds = bulbs.map(b => ({ romId: b.romId, id: b.romId, x: b.cx - b.lw / 2, y: b.cy - b.lh / 2, width: b.lw, height: b.lh, color: b.color, litB64: "" }));
      evt = parseEventMap(eventMapJson ?? null, allRomIds as any);
      rebuildMotion();
      // Build name→bulb-indices index for animation step lookups.
      rebuildNameIndex();
      // Kick off any animation marked StartAnimationAtBackglassStartup="1"
      // — matches Pi b2s_motion.cpp:226-234 (startB2SAttract).
      for (let i = 0; i < animations.length; i++) {
        if (animations[i].startAtStartup) startAnimation(i);
      }
      startLoop();
    } catch (e) {
      parseError = String(e);
    }
  }

  async function parseFromXml() {
    const d = parseDirectB2S(xml);
    grillH = d.grillHeight;
    baseNeedsCrop = grillH > 0;   // raw source PNG — draw() crops the DMD strip

    // Base image — async decode. CRITICAL: read source dims from the DECODED
    // bitmap's natural size, not from the hardcoded 1920×1080 in parseDirectB2S.
    // The Pi backfills source_width/source_height from `src->w / src->h` of the
    // decoded PNG (renderer.cpp:5166-67), and every bulb position is in that
    // coordinate space. AFM's BackglassImage decodes to 2000×1188; using
    // 1920×1080 throws every bulb's logical position off ~10% vertical.
    if (d.baseDataUrl) {
      base = await new Promise<HTMLImageElement>((res, rej) => {
        const img = new Image();
        img.onload = () => res(img);
        img.onerror = () => rej(new Error("base image failed to decode"));
        img.src = d.baseDataUrl;
      });
      baseW = (base as HTMLImageElement).naturalWidth || d.baseWidth;
      baseH = (base as HTMLImageElement).naturalHeight || d.baseHeight;
    } else {
      baseW = d.baseWidth;
      baseH = d.baseHeight;
    }

    // Scores — capture for useH extension (renderer.cpp:5179-5182)
    scores = d.scores.map(s => ({ y: s.y, h: s.height }));

    // Animations — captured here, started below after bulbs+index exist.
    animations = d.animations;

    // Bulbs — sprites decoded in parallel
    bulbs = d.bulbs.map(b => ({
      cx: b.x + b.width / 2, cy: b.y + b.height / 2,
      lw: b.width, lh: b.height,
      dx: b.x, dy: b.y, dw: b.width, dh: b.height,
      sprite: null,
      romId: b.romId, name: b.name, color: b.color,
      isSnippit: b.isSnippit,
      snippitType: b.snippitType,
      rotatingSteps: b.rotatingSteps,
      rotatingDirection: b.rotatingDirection,
      currentStep: 0,   // no MSG_MECH_STATE in preview → rest pose
      isLit: b.initialState === 1,
    }));
    // Async sprite decode (don't await — let them appear progressively)
    d.bulbs.forEach((b, i) => {
      if (!b.litB64) return;
      const img = new Image();
      img.onload  = () => { bulbs[i].sprite = img; bulbsLoaded++; };
      img.onerror = () => { bulbsLoaded++; };
      img.src = "data:image/png;base64," + b.litB64;
    });
  }

  async function parseFromCache() {
    if (!cacheBuf) return;
    const { log } = await import("$lib/api");
    log("[b2scache]", `parse start, bytes=${cacheBuf.byteLength}`);
    // Early-paint callback: fires the moment the base bitmap decodes (~50ms
    // into the parse) instead of waiting for all N bulb sprite LZ4 blocks
    // to decompress (~1-10s). Sets baseW/H/grillH + the base ImageBitmap
    // so the rAF loop below can render the backglass immediately, even
    // while parseB2SCache is still chewing through bulb sprites in the
    // background. The bulbs arrive later via the awaited `d` and pop in.
    const d = await parseB2SCache(cacheBuf, ({ baseImage, sourceWidth, sourceHeight, grillHeight }) => {
      baseW = sourceWidth;
      baseH = sourceHeight;
      grillH = grillHeight;
      // Cache writer already pre-cropped + stretched the visible region
      // into 1920×1080 BGRA; skip the draw-time crop.
      baseNeedsCrop = false;
      base = baseImage;
      log("[b2scache]", `early-paint base ready ${sourceWidth}x${sourceHeight} grill=${grillHeight}`);
      // Kick the rAF loop NOW so the user sees the backglass while bulbs
      // are still decoding. The draw() function early-returns on bulbs
      // when motionIdxs is empty — so it just paints the base. Once bulbs
      // arrive via the awaited `d` below, motion is rebuilt and bulbs
      // start animating without any further restart.
      startLoop();
    });
    // Re-affirm post-await in case state changed (shouldn't but cheap).
    baseW = d.sourceWidth;
    baseH = d.sourceHeight;
    grillH = d.grillHeight;
    base = d.baseImage;
    // b2s_cache.cpp writes baseBGRA at a fixed 1920×1080 with the DMD already
    // cropped + the visible region stretched in (renderer.cpp:5183-5192).
    // Don't crop again at draw time — the bitmap is the final intended frame.
    baseNeedsCrop = false;
    // Scores → useH extension (matches Pi b2s_motion.cpp:5179-5182 / cache
    // path renderer.cpp:5407-5411). Cactus Jack / Sword of Fury style.
    scores = d.scores.map(s => ({ y: s.y, h: s.height }));
    // Animations — same shape as B2SAnimation; cast through unknown to bridge
    // the parallel type defs in b2s.ts vs b2s_cache.ts (identical fields).
    animations = d.animations as unknown as import("$lib/b2s").B2SAnimation[];
    bulbs = d.bulbs.map(b => ({
      cx: b.x + b.width / 2, cy: b.y + b.height / 2,
      lw: b.width, lh: b.height,
      dx: b.x + b.spriteOffX, dy: b.y + b.spriteOffY,
      dw: b.spriteWidth || b.width, dh: b.spriteHeight || b.height,
      sprite: b.litSprite,
      romId: b.romId, name: b.name, color: [255, 200, 80],
      isSnippit: b.isSnippit,
      snippitType: b.snippitType,
      rotatingSteps: b.rotatingSteps,
      rotatingDirection: b.rotatingDirection,
      currentStep: 0,   // rest pose; MSG_MECH_STATE would update this on Pi
      isLit: b.initialState === 1,
    }));
    bulbsLoaded = bulbs.filter(b => b.sprite).length;
    const withRomId = bulbs.filter(b => b.romId > 0).length;
    const withSprite = bulbs.filter(b => b.sprite).length;
    log("[b2scache]", `parsed bulbs=${bulbs.length} withRomId=${withRomId} withSprite=${withSprite} baseDims=${baseW}x${baseH}`);
  }

  /** Build name → bulb-indices map for animation `on_names` / `off_names`
   *  lookups. Animations can target multiple bulbs sharing a name (e.g. all
   *  "L1" lamps in a column flash together). */
  function rebuildNameIndex() {
    bulbsByName = new Map();
    for (let i = 0; i < bulbs.length; i++) {
      const nm = bulbs[i].name;
      if (!nm) continue;
      let arr = bulbsByName.get(nm);
      if (!arr) { arr = []; bulbsByName.set(nm, arr); }
      arr.push(i);
    }
  }

  /** Apply an On/Off name list to bulbs. Direct port of Pi's
   *  `b2sApplyStepNames` — sets is_lit on every bulb whose name matches. */
  function applyStepNames(names: string[], on: boolean) {
    for (const nm of names) {
      const idxs = bulbsByName.get(nm);
      if (!idxs) continue;
      for (const i of idxs) bulbs[i].isLit = on;
    }
  }

  /** Direct port of `Renderer::b2sStartAnimation` (renderer.cpp:5518-5532). */
  function startAnimation(animIdx: number) {
    const anim = animations[animIdx];
    if (!anim || anim.steps.length === 0) return;
    activeAnims.push({
      animIdx,
      stepIdx: 0,
      phase: 0,
      waitRemaining: 0,   // fires step 0's On immediately on first tick
      loopsRemaining: anim.loops <= 0 ? -1 : anim.loops,
      lastTickMs: performance.now(),
    });
  }

  /** Direct port of `Renderer::b2sTickAnimations` (renderer.cpp:5534-5590). */
  function tickAnimations(nowMs: number) {
    if (activeAnims.length === 0) return;
    for (let i = 0; i < activeAnims.length; ) {
      const a = activeAnims[i];
      const anim = animations[a.animIdx];
      if (!anim) { activeAnims.splice(i, 1); continue; }
      const interval = anim.intervalMs > 0 ? anim.intervalMs : 25;
      const elapsed = nowMs - a.lastTickMs;
      let ticks = Math.floor(elapsed / interval);
      if (ticks <= 0) { i++; continue; }
      a.lastTickMs += ticks * interval;

      let erased = false;
      while (ticks > 0) {
        if (a.waitRemaining > 0) {
          const consume = Math.min(ticks, a.waitRemaining);
          a.waitRemaining -= consume;
          ticks -= consume;
          if (a.waitRemaining > 0) break;
        }
        // Wait finished — advance state machine.
        const step = anim.steps[a.stepIdx];
        if (a.phase === 0) {
          // Just finished post-On wait → apply Off, enter post-Off wait.
          applyStepNames(step.offNames, false);
          a.phase = 1;
          a.waitRemaining = step.waitAfterOff;
        } else {
          // Just finished post-Off wait → advance step.
          a.stepIdx++;
          if (a.stepIdx >= anim.steps.length) {
            a.stepIdx = 0;
            if (a.loopsRemaining > 0) {
              a.loopsRemaining--;
              if (a.loopsRemaining === 0) {
                activeAnims.splice(i, 1);
                erased = true;
                break;
              }
            }
          }
          // Start new step: apply On, enter post-On wait.
          const ns = anim.steps[a.stepIdx];
          applyStepNames(ns.onNames, true);
          a.phase = 0;
          a.waitRemaining = ns.waitAfterOn;
        }
      }
      if (!erased) i++;
    }
  }

  function rebuildMotion() {
    if (!evt?.attract || bulbs.length === 0) {
      motionIdxs = [];
      motionPos = new Int32Array(0);
      return;
    }
    // Apply per-preview overrides BEFORE building the rank table, so the
    // wave direction / motion type used here matches what the draw loop
    // sees. overrideMotion/Tail are nullable; null means "use authored".
    if (overrideMotion != null) evt.attract.motion = overrideMotion;
    if (overrideTail   != null) evt.attract.tail   = Math.max(1, overrideTail);
    const lamps = new Set(evt.attract.lamps);
    motionIdxs = [];
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (let i = 0; i < bulbs.length; i++) {
      const b = bulbs[i];
      if (lamps.has(b.romId)) {
        motionIdxs.push(i);
        if (b.cx < minX) minX = b.cx; if (b.cx > maxX) maxX = b.cx;
        if (b.cy < minY) minY = b.cy; if (b.cy > maxY) maxY = b.cy;
      }
    }
    motionBounds = {
      minX: minX === Infinity ? 0 : minX,
      spanX: Math.max(1, maxX - minX),
      minY: minY === Infinity ? 0 : minY,
      spanY: Math.max(1, maxY - minY),
    };
    // Build reverse index so draw can look up "is bulb b in motion, at what k?"
    // in O(1) instead of O(N) indexOf per bulb per frame.
    motionPos = new Int32Array(bulbs.length).fill(-1);
    for (let k = 0; k < motionIdxs.length; k++) motionPos[motionIdxs[k]] = k;

    // RUNNER: build position rank lookup. motionIdx → rank along the wave
    // axis (0..N-1, smallest coordinate first). bulbAlpha uses this for
    // Cylon-style primary+tail math. Built whenever the motion list changes;
    // re-sorting on each draw would be wasteful since bulb positions don't
    // move within a table. Pi b2s_motion.cpp:466-473.
    const N = motionIdxs.length;
    const rank = new Int32Array(N);
    const order = new Array<number>(N);
    const byY = evt.attract.waveDirection === "y";
    for (let k = 0; k < N; k++) order[k] = k;
    order.sort((a, b) => {
      const ba = bulbs[motionIdxs[a]];
      const bb = bulbs[motionIdxs[b]];
      return byY ? (ba.cy - bb.cy) : (ba.cx - bb.cx);
    });
    for (let r = 0; r < N; r++) rank[order[r]] = r;
    // Mutate the spec in-place; consumers (bulbAlpha) read .runnerRank.
    (evt.attract as any).runnerRank = rank;
  }

  function startLoop() {
    if (raf) cancelAnimationFrame(raf);
    startTs = performance.now();
    const step = (now: number) => { draw(now); raf = requestAnimationFrame(step); };
    raf = requestAnimationFrame(step);
  }

  function draw(now: number) {
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const frameT0 = performance.now();
    // Tick animations first so any On/Off names this frame are reflected
    // in the bulb is_lit state we use below for compose.
    tickAnimations(now);

    const w = canvas.width, h = canvas.height;
    // Match the Pi cabinet exactly: render to a 1920×1080 logical buffer with
    // separate x/y scales (renderer.cpp:5183-5192 + ~3503 b2s_scale_x_/y_).
    //   .b2scache: bitmap is already 1920×1080 (Pi cropped DMD + stretched
    //              visible region into 1920×1080 when writing the cache).
    //              Draw it as-is to fill the logical buffer.
    //   .directb2s: bitmap is raw source. srcRect crops the bottom grillH
    //              source-pixels off, dest fills 1920×1080 (the Pi's
    //              SDL_BlitScaled equivalent).
    // Then letterbox the logical buffer into the canvas with uniform `fit`.
    const PI_W = 1920, PI_H = 1080;
    // useH starts at baseH-grillH, then extends to include any score-reel
    // bottom that lives below the painted backglass (renderer.cpp:5179-5182).
    let useH = Math.max(1, baseH - grillH);
    for (const s of scores) {
      const bottom = s.y + s.h;
      if (bottom > useH) useH = bottom;
    }
    const sx = PI_W / baseW;            // Pi's b2s_scale_x_
    const sy = PI_H / useH;             // Pi's b2s_scale_y_
    const fit = Math.min(w / PI_W, h / PI_H);
    const ox = (w - PI_W * fit) / 2;
    const oy = (h - PI_H * fit) / 2;

    ctx.fillStyle = "#0a0a0a";
    ctx.fillRect(0, 0, w, h);
    if (base) {
      const natW = (base as any).naturalWidth ?? (base as ImageBitmap).width;
      const natH = (base as any).naturalHeight ?? (base as ImageBitmap).height;
      const srcH = baseNeedsCrop
        ? Math.max(1, Math.round(natH * useH / baseH))
        : natH;
      // Apply preview-only brightness to the base bitmap. Canvas2D's
      // `filter` is applied during drawImage, not stamped on the source —
      // so this doesn't mutate the cached bitmap. Bulbs draw later with
      // filter='none', so they keep their authored brightness.
      const applyFilter = baseBrightness !== 1.0;
      if (applyFilter) ctx.filter = `brightness(${baseBrightness})`;
      ctx.drawImage(
        base,
        0, 0, natW, srcH,
        ox, oy, PI_W * fit, PI_H * fit,
      );
      if (applyFilter) ctx.filter = "none";
    }
    const frameBaseEnd = performance.now();

    if (!evt?.attract || motionIdxs.length === 0) return;

    // ── Match Pi attract pipeline exactly ──────────────────────────────────
    //
    // Pi (renderer.cpp:4569-4590 CPU path, 4868-4906 GPU path):
    //   1. Each bulb sprite blitted onto the FxB layer with BLENDMODE_BLEND
    //      and SetAlphaMod(runtime_alpha). N stack-blits saturate alpha via
    //      porter-duff over: α_acc = 1 - (1-α)^N. RGB does NOT sum.
    //   2. FxB layer composited onto the base canvas additively (per layer
    //      blend mode for LayerId::FxB).
    //
    // Doing all blits in `lighter` (additive) directly on the main canvas —
    // which I was — produces N×RGB summation and oversaturates. The correct
    // pipeline needs a temporary "FxB" buffer. Reuse a single offscreen
    // canvas across frames; only re-create on canvas-size change.
    if (!fxCanvas || fxCanvas.width !== w || fxCanvas.height !== h) {
      fxCanvas = document.createElement("canvas");
      fxCanvas.width = w;
      fxCanvas.height = h;
    }
    const fx = fxCanvas.getContext("2d");
    if (!fx) return;
    fx.clearRect(0, 0, w, h);
    fx.globalCompositeOperation = "source-over";

    // Bloom stack-blit count: tunable via the Bloom Tuning panel
    // (bloom.svelte.ts). Defaults match the Pi (lit_passes=5 at
    // renderer.cpp:4441, min=1). Raising lit_passes brightens the peak;
    // raising min_passes raises the floor (dim-bulb baseline). Once a
    // chosen value is settled here, push it to renderer.cpp and rebuild
    // the Pi so the cabinet matches. Read every frame so slider changes
    // reflect live in the preview without restart.
    const LIT_PASSES = bloomState.litPasses;
    const MIN_PASSES = bloomState.minPasses;

    // Pi renders ALL bulbs where bulb.is_lit (renderer.cpp:4473):
    //   if (!bulb.is_lit) { ++dim_count; continue; }
    // Motion bulbs additionally get their runtime_alpha modulated by the
    // wave engine; non-motion lit bulbs (animation-driven or static-on)
    // render at full alpha.
    // Cache pos array length once — motionPos may be shorter than bulbs if
    // the reactive update hasn't run yet (race during parse).
    const mpLen = motionPos.length;
    for (let bi = 0; bi < bulbs.length; bi++) {
      const b = bulbs[bi];
      // O(1) motion lookup via precomputed reverse index (was per-frame
      // Set build + indexOf per bulb = O(N²)).
      const k = bi < mpLen ? motionPos[bi] : -1;
      const inMotion = k >= 0;
      const drawable = b.sprite && (inMotion || b.isLit);
      if (!drawable) continue;

      let alpha: number;
      if (inMotion) {
        // Wave attract — runtime_alpha varies 0.35..0.94 per Pi default.
        alpha = bulbAlpha(
          { x: b.cx - b.lw / 2, y: b.cy - b.lh / 2, width: b.lw, height: b.lh } as any,
          k, evt.attract, motionBounds, now, startTs
        );
      } else {
        // Animation/initial-state lit bulb — Pi blits at runtime_alpha=255.
        alpha = 1.0;
      }
      if (alpha <= 0.005) continue;

      // Pi blits the sprite N times under BLENDMODE_BLEND. Integer pass count
      // (Pi: `1 + floor((LIT_PASSES-1) · α/255)`) causes visible brightness
      // banding when α crosses pass thresholds (0.2/0.4/0.6/0.8). PP Doctor
      // smooths this: do `floor` full passes PLUS one partial pass at the
      // fractional remainder. Mathematically equivalent to "fractional N",
      // visually continuous, total brightness matches Pi at integer α values
      // and interpolates smoothly between them.
      const passesFloat = MIN_PASSES + (LIT_PASSES - MIN_PASSES) * alpha;
      const fullPasses = Math.floor(passesFloat);
      const fractional = passesFloat - fullPasses;
      fx.globalAlpha = alpha;
      const dx = ox + b.dx * sx * fit;
      const dy = oy + b.dy * sy * fit;
      const dw = b.dw * sx * fit;
      const dh = b.dh * sy * fit;

      // Rotator (SnippitType=2 with rotating_steps > 1) — direct port of
      // renderer.cpp:4892-4900. currentStep=0 in preview = 180° rest pose.
      if (b.snippitType === 2 && b.rotatingSteps > 1) {
        let angle = (360 / b.rotatingSteps) * b.currentStep;
        if (b.rotatingDirection === 0) angle = -angle;
        angle += 180;
        const rad = angle * Math.PI / 180;
        const cx = dx + dw / 2;
        const cy = dy + dh / 2;
        // Full passes
        for (let p = 0; p < fullPasses; p++) {
          fx.save();
          fx.translate(cx, cy);
          fx.rotate(rad);
          fx.drawImage(b.sprite!, -dw / 2, -dh / 2, dw, dh);
          fx.restore();
        }
        // Fractional pass for smooth alpha gradient
        if (fractional > 0.01) {
          fx.globalAlpha = alpha * fractional;
          fx.save();
          fx.translate(cx, cy);
          fx.rotate(rad);
          fx.drawImage(b.sprite!, -dw / 2, -dh / 2, dw, dh);
          fx.restore();
        }
      } else {
        // Static bulb — full passes + fractional partial for smooth gradient.
        for (let p = 0; p < fullPasses; p++) fx.drawImage(b.sprite!, dx, dy, dw, dh);
        if (fractional > 0.01) {
          fx.globalAlpha = alpha * fractional;
          fx.drawImage(b.sprite!, dx, dy, dw, dh);
        }
      }
    }

    // Final FxB→base composite — additive, matches Pi's LayerId::FxB blend.
    //
    // Pi sequence (renderer.cpp):
    //   1. b2sCompositeBulbs sets FxB layer alpha = 255 (lines 4647, 4914)
    //   2. Per-frame loop OVERRIDES alpha with attract_min/max formula —
    //      BUT ONLY when `b2s_attract_mode_` is true (line 855).
    //
    // PP Doctor doesn't track an explicit "attract_mode" boolean (we're
    // always in preview), so the question is when to apply the pulse. The
    // user-observed truth (2026-05-25): cabinet renders bulbs visibly even
    // when event_map says min=max=0 — because Pi's attract_mode hasn't
    // engaged for those tables. So treat min=max=0 as "no modulation,
    // layer stays at full" (the Pi default before attract_mode kicks in).
    // Non-zero values → apply breathing pulse as authored.
    // Priority for the FxB layer pulse:
    //   1. Per-preview override (slider in the right sidebar)
    //   2. event_map.json authored values
    //   3. Global bloomState defaults (Pi-matching: 230/255/3.0)
    // Override wins so the user can tune live before committing.
    const lpEm = evt.layerPulse ?? {
      minAlpha: bloomState.layerMin,
      maxAlpha: bloomState.layerMax,
      cycleSeconds: bloomState.cycleSeconds,
    };
    const lp = {
      minAlpha:     overrideMinAlpha     ?? lpEm.minAlpha,
      maxAlpha:     overrideMaxAlpha     ?? lpEm.maxAlpha,
      cycleSeconds: overrideCycleSeconds ?? lpEm.cycleSeconds,
    };
    let layerAlpha01: number;
    if (lp.minAlpha === 0 && lp.maxAlpha === 0) {
      // No pulse configured — Pi keeps FxB at 255. Match.
      layerAlpha01 = 1.0;
    } else {
      const cycleMs = Math.max(50, lp.cycleSeconds * 1000);
      const phase = ((now - startTs) % cycleMs) / cycleMs;
      const layerS = 0.5 + 0.5 * Math.sin(phase * 2 * Math.PI);
      const layerAlpha255 = lp.minAlpha + layerS * (lp.maxAlpha - lp.minAlpha);
      layerAlpha01 = Math.max(0, Math.min(255, layerAlpha255)) / 255;
    }
    const frameBulbsEnd = performance.now();
    ctx.globalAlpha = layerAlpha01;
    ctx.globalCompositeOperation = "lighter";
    ctx.drawImage(fxCanvas, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = "source-over";

    // Frame telemetry — accumulate per-stage costs, log a summary once
    // per second. Stages: base = base bitmap + grill crop; bulbs = the
    // per-bulb stack-blit loop into fxCanvas; compose = final fxCanvas
    // → main canvas with layer-alpha pulse.
    const frameEnd = performance.now();
    const frameMs = frameEnd - frameT0;
    frameSamples.push(frameMs);
    frameStageBase    += (frameBaseEnd  - frameT0);
    frameStageBulbs   += (frameBulbsEnd - frameBaseEnd);
    frameStageCompose += (frameEnd      - frameBulbsEnd);

    if (frameEnd - lastFrameLog > 1000 && frameSamples.length > 0) {
      const n = frameSamples.length;
      const sum = frameSamples.reduce((a, b) => a + b, 0);
      const avg = sum / n;
      const max = frameSamples.reduce((a, b) => Math.max(a, b), 0);
      const min = frameSamples.reduce((a, b) => Math.min(a, b), Infinity);
      const fps = Math.round(1000 / avg);
      import("$lib/api").then(({ log }) => log("[B2S_FRAME]",
        `${n}f in ${Math.round(frameEnd - lastFrameLog)}ms — fps=${fps} ` +
        `frame avg=${avg.toFixed(1)}ms min=${min.toFixed(1)} max=${max.toFixed(1)} ` +
        `[base=${(frameStageBase/n).toFixed(1)} bulbs=${(frameStageBulbs/n).toFixed(1)} compose=${(frameStageCompose/n).toFixed(1)}]ms ` +
        `bulbs=${bulbs.length} motion=${motionIdxs.length}`)).catch(() => {});
      frameSamples = [];
      frameStageBase = frameStageBulbs = frameStageCompose = 0;
      lastFrameLog = frameEnd;
    }
  }

  function setupResize(el: HTMLCanvasElement) {
    const parent = el.parentElement;
    if (!parent) return;
    const ro = new ResizeObserver(() => {
      const r = parent.getBoundingClientRect();
      el.width  = Math.round(r.width);
      el.height = Math.round(r.height);
    });
    ro.observe(parent);
    const r = parent.getBoundingClientRect();
    el.width = Math.round(r.width); el.height = Math.round(r.height);
    return { destroy() { ro.disconnect(); } };
  }

  let source = $derived(xml ? ".directb2s" : (cacheBuf ? ".b2scache" : "none"));
</script>

<div class="w-full h-full relative">
  {#if parseError}
    <div class="absolute inset-0 flex items-center justify-center text-red-300 text-sm p-4 z-10">
      {parseError}
    </div>
  {/if}
  <canvas bind:this={canvas} use:setupResize class="w-full h-full block"></canvas>
  <!-- Always-on diagnostic strip so we can see exactly where the pipeline drops out:
       base · bulbs · motion · sprites · evt.attract presence · source.
       If any of these don't progress as expected, the failure is upstream. -->
  <div class="absolute bottom-2 right-2 text-[10px] font-mono text-zinc-300 bg-black/60 px-2 py-1 rounded leading-snug">
    base:{base ? `${baseW}×${baseH}` : "—"} ·
    grill:{grillH} ·
    bulbs:{bulbs.length} ·
    motion:{motionIdxs.length} ·
    anim:{animations.length}({activeAnims.length}) ·
    sprites:{bulbsLoaded}/{bulbs.length} ·
    attract:{evt?.attract ? evt.attract.motion : "—"} ·
    src:{source}
  </div>
</div>
