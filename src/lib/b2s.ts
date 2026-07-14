// .directb2s + b2s_event_map.json parser. Ports just enough of the Pi
// renderer's b2s_motion.cpp WAVE logic so the attract preview matches what
// the cabinet actually plays.

export interface B2SBulb {
  id: number;
  romId: number;
  romIdType: number;
  /** Name attribute — name-based addressing for EM tables. */
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  color: [number, number, number];  // OLE BGR fallback if no sprite
  litB64: string;                    // base64 lit-sprite PNG (Image attr)
  unlitB64: string;                  // base64 unlit-sprite PNG (OffImage attr)
  /** Pi snippit_type from b.attribute("SnippitType"). Non-zero indicates the
   *  bulb is a snippet/rotator (full-backglass overlay or rotating element).
   *  PP Doctor uses isSnippit as a draw-time hint; same wire format as Pi. */
  snippitType: number;
  isSnippit: boolean;
  /** Snippet-rotator (SnippitType=2, e.g. Scared Stiff's spider) fields. */
  rotatingSteps: number;
  rotatingDirection: number;
  rotatingStopBehaviour: number;
  initialState: number;
}

/** Score-reel rect — needed to extend useH for tables that author score reels
 *  below the painted backglass (Cactus Jack / Sword of Fury pattern).
 *  Mirrors B2SScore in b2s_parser.h. */
export interface B2SScore {
  id: number;
  x: number;
  y: number;
  width: number;
  height: number;
  digits: number;
  spacing: number;
  playerNo: number;
  startDigit: number;
  /** Reel colors from ReelLitColor/ReelDarkColor ("R.G.B"); used by the .b2scache
   *  writer so PBA score-reel tables render the right digit colors. */
  litR: number;
  litG: number;
  litB: number;
  darkR: number;
  darkG: number;
  darkB: number;
  reelType: string;
}

/** Authored animation from .directb2s `<Animations>` block. Pi plays these
 *  via `b2sStartAnimation` + `b2sTickAnimations` in renderer.cpp:5518-5590.
 *  Per-step `on_names` / `off_names` toggle bulbs identified by their `name`
 *  attribute (not RomID) — this is how event-driven flashes are choreographed. */
export interface B2SAnimation {
  name: string;
  intervalMs: number;
  loops: number;           // <= 0 = infinite
  lightsAtStart: number;
  lightsAtEnd: number;
  stopBehaviour: number;
  startAtStartup: boolean;
  lockInvolved: boolean;
  steps: B2SAnimationStep[];
}
export interface B2SAnimationStep {
  onNames: string[];
  offNames: string[];
  waitAfterOn: number;     // in interval-ticks
  waitAfterOff: number;
}

/** Re-exported for callers that pass these around. */
export interface B2SDocBase {
  baseWidth: number;
  baseHeight: number;
  /** Pixels of grill/DMD strip at the bottom that should be cropped out
   *  during display (matches Pi renderer: useH = baseHeight - grillHeight). */
  grillHeight: number;
}

export interface B2SDoc extends B2SDocBase {
  baseDataUrl: string;
  bulbs: B2SBulb[];
  scores: B2SScore[];
  animations: B2SAnimation[];
  tableName: string;
}

/** Mirror of b2s_event_map.json's attract_animation block. Motion types
 *  match b2s_motion.cpp 1:1. RUNNER, SWEEP, RIPPLE were added 2026-05-25
 *  to expose them in PP Doctor — note that on the Pi today SWEEP and
 *  RIPPLE fall through to ALL_ON (b2s_motion.cpp:508), so PP Doctor
 *  mirrors that behavior. */
export type AttractMotion =
  | "wave" | "flash" | "strobe" | "random" | "all_on"
  | "runner" | "sweep" | "ripple";

export interface AttractSpec {
  motion: AttractMotion;
  lamps: number[];        // RomIDs (or B2SIDs for EM tables)
  speedMs: number;
  brightness: number;     // 0-255 peak
  minBrightness: number;  // 0-255 valley
  waveDirection: "x" | "y";
  /** RUNNER only — number of trailing bulbs that fade behind the primary.
   *  Defaults to 3 when not specified in event_map. */
  tail: number;
  /** RUNNER only — position rank lookup. motionIdx → 0..N-1 sorted by
   *  waveDirection center coordinate. Built by the caller (B2SCanvas)
   *  after motion bulbs are resolved; bulbAlpha consumes it for the
   *  primary+tail math. Null/empty disables runner. */
  runnerRank?: Int32Array;
}

export interface EventMap {
  attract?: AttractSpec;
  /** Whole-FxB-layer breathing alpha (applied on top of per-bulb motion).
   *  Pi: renderer.cpp:822-870, formula
   *      alpha = min + (0.5 + 0.5 * sin(2π·t/cycle)) * (max - min)
   *  Defaults from renderer.cpp:2991 when not in event_map. Set min=max=0
   *  to disable attract layer entirely (the entire FxB stays invisible). */
  layerPulse?: {
    minAlpha: number;       // 0-255
    maxAlpha: number;       // 0-255
    cycleSeconds: number;   // wave breathing period
  };
}

/** Renderer defaults — match global_effects.json + renderer.h fallbacks. */
const DEFAULT_SPEED_MS = 4200;
const DEFAULT_BRIGHTNESS = 240;
const DEFAULT_MIN_BRIGHTNESS = 90;

/** Parse a .directb2s XML string.
 *
 * **Direct port of the Pi renderer's `b2s_parser.cpp::parseDirectB2S`** so
 * PP Doctor sees exactly the same bulbs/scores/dims that the cabinet does.
 * Any divergence here breaks the preview-matches-cabinet rule (see memory
 * note feedback_ppdoctor_preview_matches_cabinet). Specifically mirrors:
 *
 *   1. Root must be `<DirectB2SData>` — return null/empty on missing
 *   2. Base image is `<Images><BackglassImage Value="..."/>` only
 *   3. Bulbs come from `<Illumination><Bulb>` direct children (NOT any
 *      `<Bulb>` anywhere — that would catch animation-step refs too)
 *   4. `Parent="Backglass"` (default when attr absent) or skip
 *   5. EM fallback: when RomID=0, adopt B2SID as the addressable id
 *   6. Skip bulb if Image= is empty (the cache writer + renderer compose
 *      paths refuse to render a no-PNG bulb, so PP Doctor matches)
 *   7. Read Snippit* + OffImage so motion + rotating logic can use them
 *   8. Parse `<Scores>` (used to extend useH for Cactus Jack / SoF style)
 */
export function parseDirectB2S(xml: string): B2SDoc {
  const doc = new DOMParser().parseFromString(xml, "text/xml");
  const perr = doc.querySelector("parsererror");
  if (perr) throw new Error("invalid .directb2s XML: " + perr.textContent);

  const root = doc.querySelector("DirectB2SData");
  if (!root) throw new Error("missing <DirectB2SData> root");

  // Table name (informational)
  const tableName = root.querySelector(":scope > Name")?.getAttribute("Value") ?? "";

  // <GrillHeight Value="N" /> — pixels at source resolution that belong to the
  // DMD strip and are cropped before stretching to 1920×1080.
  const grillHeight = parseInt(
    root.querySelector(":scope > GrillHeight")?.getAttribute("Value") ?? "0", 10) || 0;

  // <Images><BackglassImage Value="<base64 PNG>"/></Images> — Pi takes the
  // BackglassImage explicitly, no fallback to a bare <Backglass>.
  const bg = root.querySelector(":scope > Images > BackglassImage");
  let baseBase64 = bg?.getAttribute("Value") ?? "";
  // Strip any data:image/...;base64, prefix in case it was authored that way
  baseBase64 = baseBase64.replace(/^data:image\/[^;]+;base64,/, "");

  // Hardcoded fallback dims — overridden in B2SCanvas by decoded PNG naturalWidth/Height
  // (Pi backfills the same way in renderer.cpp:5166-67).
  const baseWidth = 1920;
  const baseHeight = 1080;

  // <Illumination><Bulb .../></Illumination> — direct children only.
  const bulbs: B2SBulb[] = [];
  const illum = root.querySelector(":scope > Illumination");
  if (illum) {
    for (const el of Array.from(illum.children)) {
      if (el.tagName !== "Bulb") continue;

      // Pi: const char* parent = b.attribute("Parent").as_string("Backglass");
      //     if (parent && std::strcmp(parent, "Backglass") != 0) continue;
      const parent = el.getAttribute("Parent") ?? "Backglass";
      if (parent !== "Backglass") continue;

      const id            = parseInt(el.getAttribute("ID") ?? "0", 10) || 0;
      let   romId         = parseInt(el.getAttribute("RomID") ?? "0", 10) || 0;
      const romIdType     = parseInt(el.getAttribute("RomIDType") ?? "1", 10) || 1;
      // EM-table B2SID fallback (b2s_parser.cpp:129-132): adopt B2SID as
      // addressable id when RomID=0 — Bally/Gottlieb EM tables route by
      // Name/B2SID, and without this fallback attract synthesis (which
      // filters romId>0) drops every bulb on the table.
      if (romId === 0) {
        const b2sid = parseInt(el.getAttribute("B2SID") ?? "0", 10) || 0;
        if (b2sid > 0) romId = b2sid;
      }
      const name = el.getAttribute("Name") ?? "";
      const x = parseInt(el.getAttribute("LocX") ?? "0", 10) || 0;
      const y = parseInt(el.getAttribute("LocY") ?? "0", 10) || 0;
      const width  = parseInt(el.getAttribute("Width") ?? "0", 10) || 0;
      const height = parseInt(el.getAttribute("Height") ?? "0", 10) || 0;
      const initialState         = parseInt(el.getAttribute("InitialState") ?? "0", 10) || 0;
      const snippitType          = parseInt(el.getAttribute("SnippitType") ?? "0", 10) || 0;
      const rotatingSteps        = parseInt(el.getAttribute("SnippitRotatingSteps") ?? "1", 10) || 1;
      const rotatingDirection    = parseInt(el.getAttribute("SnippitRotatingDirection") ?? "0", 10) || 0;
      const rotatingStopBehaviour = parseInt(el.getAttribute("SnippitRotatingStopBehaviour") ?? "0", 10) || 0;

      // LightColor (OLE BGR encoded int) — fallback when no lit PNG.
      let color: [number, number, number] = [255, 200, 80];
      const lc = el.getAttribute("LightColor");
      if (lc) {
        const n = parseInt(lc, 10);
        if (!isNaN(n)) color = [n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff];
      }

      let litB64 = el.getAttribute("Image") ?? "";
      litB64 = litB64.replace(/^data:image\/[^;]+;base64,/, "");
      let unlitB64 = el.getAttribute("OffImage") ?? "";
      unlitB64 = unlitB64.replace(/^data:image\/[^;]+;base64,/, "");

      // Pi: if (!bulb.lit_png.empty()) out.bulbs.push_back(...)
      // Skip bulbs with no lit PNG (they'd never render).
      if (!litB64) continue;

      // Pi only checks `snippit_type != 0` (b2s_parser.cpp:140 reads
      // SnippitType, doesn't touch IsImageSnippit). Mirror that exactly —
      // dual checks would diverge on tables that author one but not the other.
      const isSnippit = snippitType !== 0;

      bulbs.push({
        id, romId, romIdType, name,
        x, y, width, height,
        color, litB64, unlitB64,
        snippitType, isSnippit,
        rotatingSteps, rotatingDirection, rotatingStopBehaviour,
        initialState,
      });
    }
  }

  // <Scores><Score Parent="Backglass" .../></Scores> — direct children only.
  // Used by B2SCanvas to extend `useH` for score reels authored below the
  // painted backglass (renderer.cpp:5179-5182). Skip Parent="DMD" scores.
  const scores: B2SScore[] = [];
  const scoresNode = root.querySelector(":scope > Scores");
  if (scoresNode) {
    // "R.G.B" (dot-separated, clamped 0-255) — matches the Pi's parseColor
    // (b2s_parser.cpp:194-196). Absent/malformed → 0,0,0.
    const parseColor = (s: string | null): [number, number, number] => {
      const parts = (s ?? "").split(".");
      if (parts.length !== 3) return [0, 0, 0];
      const clamp = (n: number) => (Number.isFinite(n) ? Math.max(0, Math.min(255, n)) : 0);
      return [clamp(parseInt(parts[0], 10)), clamp(parseInt(parts[1], 10)), clamp(parseInt(parts[2], 10))];
    };
    for (const el of Array.from(scoresNode.children)) {
      if (el.tagName !== "Score") continue;
      const parent = el.getAttribute("Parent") ?? "Backglass";
      if (parent !== "Backglass") continue;
      const [litR, litG, litB] = parseColor(el.getAttribute("ReelLitColor"));
      const [darkR, darkG, darkB] = parseColor(el.getAttribute("ReelDarkColor"));
      scores.push({
        id:        parseInt(el.getAttribute("ID") ?? "0", 10) || 0,
        x:         parseInt(el.getAttribute("LocX") ?? "0", 10) || 0,
        y:         parseInt(el.getAttribute("LocY") ?? "0", 10) || 0,
        width:     parseInt(el.getAttribute("Width") ?? "0", 10) || 0,
        height:    parseInt(el.getAttribute("Height") ?? "0", 10) || 0,
        digits:    parseInt(el.getAttribute("Digits") ?? "6", 10) || 6,
        spacing:   parseInt(el.getAttribute("Spacing") ?? "5", 10) || 5,
        playerNo:    parseInt(el.getAttribute("B2SPlayerNo") ?? "0", 10) || 0,
        startDigit:  parseInt(el.getAttribute("B2SStartDigit") ?? "0", 10) || 0,
        litR, litG, litB, darkR, darkG, darkB,
        reelType: el.getAttribute("ReelType") ?? "",
      });
    }
  }

  // <Animations><Animation Parent="Backglass" ...>
  //   <AnimationStep Step="N" On="nameA,nameB" WaitLoopsAfterOn=N
  //                  Off="nameC" WaitLoopsAfterOff=N />
  // ...</Animation></Animations>
  // Pi b2s_parser.cpp:223-247 — split On/Off CSV, default Parent=Backglass.
  const splitCsv = (s: string): string[] => {
    if (!s) return [];
    return s.split(",").map(t => t.replace(/\s+/g, "")).filter(t => t.length > 0);
  };
  const animations: B2SAnimation[] = [];
  const animsNode = root.querySelector(":scope > Animations");
  if (animsNode) {
    for (const a of Array.from(animsNode.children)) {
      if (a.tagName !== "Animation") continue;
      const parent = a.getAttribute("Parent") ?? "Backglass";
      if (parent !== "Backglass") continue;
      const steps: B2SAnimationStep[] = [];
      for (const s of Array.from(a.children)) {
        if (s.tagName !== "AnimationStep") continue;
        steps.push({
          onNames:      splitCsv(s.getAttribute("On") ?? ""),
          offNames:     splitCsv(s.getAttribute("Off") ?? ""),
          waitAfterOn:  parseInt(s.getAttribute("WaitLoopsAfterOn") ?? "0", 10) || 0,
          waitAfterOff: parseInt(s.getAttribute("WaitLoopsAfterOff") ?? "0", 10) || 0,
        });
      }
      // Pi: if (!anim.steps.empty()) out.animations.push_back(...)
      if (steps.length === 0) continue;
      // Pi uses pugi `as_int(0) != 0` for booleans — returns 0 if attr is
      // missing OR unparseable ("true"/"True" → 0 = false). String compare
      // "!== '0'" would incorrectly map "True" → true; mirror Pi's behavior.
      const asIntBool = (attr: string): boolean => {
        const v = parseInt(a.getAttribute(attr) ?? "0", 10);
        return !isNaN(v) && v !== 0;
      };
      animations.push({
        name:             a.getAttribute("Name") ?? "",
        intervalMs:       parseInt(a.getAttribute("Interval") ?? "25", 10) || 25,
        loops:            parseInt(a.getAttribute("Loops") ?? "1", 10) || 1,
        startAtStartup:   asIntBool("StartAnimationAtBackglassStartup"),
        lightsAtStart:    parseInt(a.getAttribute("LightsStateAtAnimationStart") ?? "0", 10) || 0,
        lightsAtEnd:      parseInt(a.getAttribute("LightsStateAtAnimationEnd") ?? "0", 10) || 0,
        stopBehaviour:    parseInt(a.getAttribute("AnimationStopBehaviour") ?? "0", 10) || 0,
        lockInvolved:     asIntBool("LockInvolvedLamps"),
        steps,
      });
    }
  }

  return {
    baseDataUrl: baseBase64 ? "data:image/png;base64," + baseBase64 : "",
    baseWidth, baseHeight, grillHeight, bulbs, scores, animations, tableName,
  };
}

/** Parse b2s_event_map.json. Returns the attract spec — synthesized default if
 *  no event_map / no attract_animation block. */
export function parseEventMap(jsonText: string | null, allBulbs: B2SBulb[]): EventMap {
  let cfg: any = null;
  if (jsonText) {
    try { cfg = JSON.parse(jsonText); } catch { /* fall through to synth default */ }
  }

  // Whole-layer pulse (renderer.cpp:2991 defaults). Always present in returned
  // EventMap even when the event_map.json omits these keys — Pi falls back to
  // 230/255/3.0 so PP Doctor should too. If user sets both min=max=0 in the
  // event_map they're explicitly disabling attract layer-wide.
  const layerPulse = {
    minAlpha: cfg?.attract_min_alpha ?? 230,
    maxAlpha: cfg?.attract_max_alpha ?? 255,
    cycleSeconds: cfg?.attract_cycle_seconds ?? 3.0,
  };

  const a = cfg?.attract_animation;
  if (a && Array.isArray(a.lamps) && a.lamps.length > 0) {
    return {
      attract: {
        motion: (a.motion ?? "wave") as AttractMotion,
        lamps: a.lamps.map(Number),
        speedMs: a.speed ?? DEFAULT_SPEED_MS,
        brightness: a.brightness ?? DEFAULT_BRIGHTNESS,
        minBrightness: a.min_brightness ?? DEFAULT_MIN_BRIGHTNESS,
        waveDirection: (a.wave_direction === "y" ? "y" : "x"),
        tail: Math.max(1, a.tail ?? 3),
      },
      layerPulse,
    };
  }
  // Synthesized default — slow wave over every RomID>0 bulb, matches
  // Renderer::makeDefaultAttractMotion() in b2s_motion.cpp.
  return {
    attract: {
      motion: "wave",
      lamps: allBulbs.filter(b => b.romId > 0).map(b => b.romId),
      speedMs: DEFAULT_SPEED_MS,
      brightness: DEFAULT_BRIGHTNESS,
      minBrightness: DEFAULT_MIN_BRIGHTNESS,
      waveDirection: "x",
      tail: 3,
    },
    layerPulse,
  };
}

// ─── motion engine — direct port of b2s_motion.cpp:b2sApplyMotionToBulbs ─────

/** Returns indices of bulbs that participate in this motion, plus the
 *  position bounds used by WAVE for phase normalization. */
export function selectMotionBulbs(bulbs: B2SBulb[], lamps: number[]) {
  const idxs: number[] = [];
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  for (let i = 0; i < bulbs.length; i++) {
    const b = bulbs[i];
    if (lamps.includes(b.romId) || lamps.includes(b.id)) {
      idxs.push(i);
      const cx = b.x + b.width / 2, cy = b.y + b.height / 2;
      if (cx < minX) minX = cx; if (cx > maxX) maxX = cx;
      if (cy < minY) minY = cy; if (cy > maxY) maxY = cy;
    }
  }
  return {
    idxs,
    minX: minX === Infinity ? 0 : minX,
    spanX: Math.max(1, maxX - minX),
    minY: minY === Infinity ? 0 : minY,
    spanY: Math.max(1, maxY - minY)
  };
}

/** Per-bulb alpha (0..1) for a given motion + time. Mirrors the renderer's
 *  WAVE/FLASH/STROBE/RANDOM/ALL_ON branches. */
export function bulbAlpha(
  bulb: B2SBulb,
  bulbIdxInMotion: number,
  spec: AttractSpec,
  bounds: { minX: number; spanX: number; minY: number; spanY: number },
  nowMs: number,
  startMs: number
): number {
  const elapsed = nowMs - startMs;

  switch (spec.motion) {
    case "all_on":
    case "flash":
    // SWEEP / RIPPLE fall through to ALL_ON on Pi (b2s_motion.cpp:508).
    // Mirror that behavior so PP Doctor preview matches cabinet — when
    // these are implemented Pi-side, port the math here too.
    case "sweep":
    case "ripple": {
      return spec.brightness / 255;
    }
    case "runner": {
      // Cylon-style ping-pong chaser. Port of b2s_motion.cpp:462-505.
      // `runnerRank` must be pre-built by the caller (motion build-time
      // sort of bulbs by waveDirection center). Without it, RUNNER is a
      // no-op (returns 0) — render code is expected to detect this and
      // fall back gracefully.
      const rank = spec.runnerRank;
      if (!rank || rank.length === 0) return 0;
      const N = rank.length;
      const myRank = bulbIdxInMotion >= 0 && bulbIdxInMotion < N ? rank[bulbIdxInMotion] : -1;
      if (myRank < 0) return 0;
      if (N === 1) return spec.brightness / 255;
      const stepMs = Math.max(1, spec.speedMs);
      const period = 2 * (N - 1);
      const q = Math.floor(elapsed / stepMs) % period;
      const primaryRank = q < N ? q : (2 * (N - 1) - q);
      const goingRight = q < (N - 1);
      const dir = goingRight ? 1 : -1;
      const tail = Math.max(1, spec.tail);
      const t = (primaryRank - myRank) * dir;
      if (t < 0 || t >= tail) return 0;
      const fade = 1 - t / tail;
      return Math.max(0, Math.min(255, fade * spec.brightness)) / 255;
    }
    case "strobe": {
      const half = Math.max(1, spec.speedMs / 2);
      const on = Math.floor(elapsed / half) % 2 === 0;
      return on ? spec.brightness / 255 : 0;
    }
    case "random": {
      const step = Math.max(1, spec.speedMs);
      const phase = Math.floor(elapsed / step);
      // hash(phase, idx) -> [0, 1)
      let h = ((phase * 0x9e3779b1) ^ (bulbIdxInMotion * 0xb5297a4d)) >>> 0;
      h = (h ^ (h >>> 15)) >>> 0;
      const on = (h & 0xff) < 128;
      return on ? spec.brightness / 255 : 0;
    }
    case "wave":
    default: {
      // Port of b2s_motion.cpp:420-444. Phase from POSITION + per-bulb jitter.
      const t = elapsed / Math.max(1, spec.speedMs);
      const TWO_PI = 6.28318;
      const pos01 = spec.waveDirection === "y"
        ? (bulb.y + bulb.height / 2 - bounds.minY) / bounds.spanY
        : (bulb.x + bulb.width / 2 - bounds.minX) / bounds.spanX;
      // Per-bulb jitter — same hash the renderer uses (mul by 2654435761 ^ 0xB5297A4D)
      // JS bit-twiddling: keep to 32 bits via Math.imul / >>> 0.
      const h = (Math.imul(bulbIdxInMotion, 2654435761) ^ 0xB5297A4D) >>> 0;
      const jitter = ((h & 0xFFFF) / 0xFFFF) * 0.5;
      const phase = (pos01 + jitter) * TWO_PI;
      const s = 0.5 + 0.5 * Math.sin(t * TWO_PI + phase);
      const span = Math.max(0, spec.brightness - spec.minBrightness);
      const alpha = spec.minBrightness + s * span;
      return Math.max(0, Math.min(255, alpha)) / 255;
    }
  }
}
