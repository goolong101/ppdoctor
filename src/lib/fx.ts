// fx.ts — PP Doctor port of the Pi renderer's backglass FX engine
// (PinnerPi_repo/b2s_fx.cpp). Particle emitters + warpable image overlays
// authored in b2s_event_map.json ("particles" / "overlays" arrays).
//
// KEEP IN LOCKSTEP WITH b2s_fx.cpp: presets, field names/defaults, unit
// scaling, spawn/step/draw math. The preview must show what the cabinet
// draws. Units: JSON fields are .directb2s SOURCE pixels; the sim runs in
// Pi screen pixels (1920×1080) after scaling with sx/sy exactly like the Pi.
// Randomness is the only non-identical part (rand() vs Math.random()).

export type FxSlot = 0 | 1 | 2;   // below lamps / above lamps / top

export type FxColor = [number, number, number] | [number, number, number, number];

/** The JSON shape of one "particles" entry — what gets written to the event_map. */
export interface FxEmitterJson {
  name?: string;
  enabled?: boolean;
  preset?: string;
  z?: "below" | "above" | "top" | "below_lamps" | "above_lamps" | "behind";
  sprite?: "soft" | "dot" | "streak" | "star" | "puff" | "ring";
  blend?: "add" | "blend";
  spawn?: "area" | "top" | "bottom" | "left" | "right" | "point";
  area?: [number, number, number, number];
  point?: [number, number];
  clip?: [number, number, number, number];
  max?: number;
  rate?: number;
  burst?: number;
  duration_ms?: number;
  trigger?: string | string[];
  wrap?: boolean;
  life_s?: number | [number, number];
  speed?: number | [number, number];
  angle?: number;
  spread?: number;
  gravity?: number;
  drag?: number;
  wind?: [number, number];
  wander?: number;
  size?: number | [number, number];
  grow?: number;
  aspect?: number;
  align?: boolean;
  rotate?: number | [number, number];
  color?: FxColor | FxColor[];
  alpha?: "flat" | "fade_out" | "fade_in_out" | "fade_in";
  alpha_max?: number;
  twinkle_hz?: number;
  twinkle?: number;
  prewarm_s?: number;
  /** Whole-emitter brightness breathing. */
  pulse?: { period_ms?: number; depth?: number };
}

export interface FxOverlayJson {
  name?: string;
  enabled?: boolean;
  image: string;
  rect?: [number, number, number, number];
  z?: "below" | "above" | "top";
  blend?: "add" | "blend";
  alpha?: number;
  pivot?: [number, number];
  breathe?: { period_ms?: number; scale_x?: number; scale_y?: number; alpha?: number };
  sway?: { period_ms?: number; deg?: number };
  bob?: { period_ms?: number; px?: number };
  drift?: { vx?: number; vy?: number; wrap?: boolean };
  pulse?: { period_ms?: number; depth?: number };
  warp?: { type?: "wave" | "flag" | "bulge" | "jelly"; amp?: number; period_ms?: number; freq?: number };
  mesh?: number;
  /** Sprite-sheet animation: image holds cols×rows frames, `count` used, looped at fps. */
  frames?: { cols?: number; rows?: number; count?: number; fps?: number };
  trigger?: string | string[];
  duration_ms?: number;
  fade_ms?: number;
}

export interface FxDoc { particles: FxEmitterJson[]; overlays: FxOverlayJson[]; }

export const FX_PRESETS = [
  "starfield", "rain", "snow", "smoke", "fire", "flames", "embers", "sparkles", "fireflies", "meteors", "laser",
] as const;
export type FxPreset = typeof FX_PRESETS[number];

export const SPRITE_NAMES = ["soft", "dot", "streak", "star", "puff", "ring"] as const;

// ── caps (b2s_fx.cpp) ─────────────────────────────────────────────────────────
const SPRITE_SIZE = 64;
const MAX_PARTICLES_TOTAL = 500;
const MAX_PER_EMITTER = 300;
const MAX_EMITTERS = 16;
const MAX_OVERLAYS = 8;
const MAX_MESH = 24;

// ── presets: SOURCE-pixel units, identical table to applyPreset() ────────────
// [sprite, blend, z, spawn, maxN, rate, lifeMin, lifeMax, speedMin, speedMax,
//  angle, spread, gravity, drag, wander, sizeMin, sizeMax, grow, rotMin, rotMax,
//  alpha, alphaMax, twHz, twAmt, wrap, prewarm, aspect, align, ramp]
type PresetRow = {
  sprite: string; blend: string; z: string; spawn: string; maxN: number; rate: number;
  lifeMin: number; lifeMax: number; speedMin: number; speedMax: number; angle: number; spread: number;
  gravity: number; drag: number; wander: number; sizeMin: number; sizeMax: number; grow: number;
  rotMin: number; rotMax: number; alpha: string; alphaMax: number; twHz: number; twAmt: number;
  wrap: boolean; prewarm: number; aspect: number; align: boolean; ramp: FxColor[];
};
function row(sprite: string, blend: string, z: string, spawn: string, maxN: number, rate: number,
  lifeMin: number, lifeMax: number, speedMin: number, speedMax: number, angle: number, spread: number,
  gravity: number, drag: number, wander: number, sizeMin: number, sizeMax: number, grow: number,
  rotMin: number, rotMax: number, alpha: string, alphaMax: number, twHz: number, twAmt: number,
  wrap: boolean, prewarm: number, aspect: number, align: boolean, ramp: FxColor[]): PresetRow {
  return { sprite, blend, z, spawn, maxN, rate, lifeMin, lifeMax, speedMin, speedMax, angle, spread,
    gravity, drag, wander, sizeMin, sizeMax, grow, rotMin, rotMax, alpha, alphaMax, twHz, twAmt,
    wrap, prewarm, aspect, align, ramp };
}
const PRESETS: Record<string, PresetRow> = {
  starfield: row("soft",   "add",   "below", "area",   150, 0,    0,    0,    8,    22,   180, 3,   0, 1,    0,  3,  9,   0,    0,    0,   "flat",        0.9,  0.5, 0.6, true,  0,  1,  false, [[255,255,255],[200,220,255]]),
  rain:      row("streak", "blend", "above", "top",    250, 180,  1.2,  2.2,  900,  1400, 95,  2,   0, 1,    0,  3,  6,   0,    0,    0,   "flat",        0.55, 0,   0,   false, 2,  7,  true,  [[200,220,255]]),
  snow:      row("soft",   "blend", "above", "top",    180, 25,   8,    14,   35,   70,   90,  20,  0, 1,    45, 4,  12,  0,    0,    0,   "flat",        0.85, 0,   0,   false, 12, 1,  false, [[255,255,255]]),
  smoke:     row("puff",   "blend", "above", "bottom", 50,  10,   3,    5,    40,   80,   270, 25,  0, 0.6,  30, 60, 120, 1.5,  -20,  20,  "fade_in_out", 0.35, 0,   0,   false, 4,  1,  false, [[150,150,150],[80,80,80]]),
  fire:      row("soft",   "add",   "above", "bottom", 140, 70,   0.6,  1.2,  120,  220,  270, 18,  0, 0.7,  40, 30, 60,  -0.6, 0,    0,   "fade_out",    0.9,  0,   0,   false, 1.5,1,  false, [[255,255,200],[255,170,40],[220,50,10],[90,10,0]]),
  flames:    row("soft",   "add",   "above", "bottom", 220, 170,  0.45, 0.9,  170,  320,  270, 9,   0, 0.8,  140,70, 140, -0.75,0,    0,   "fade_out",    0.8,  0,   0,   false, 1,  1,  false, [[255,255,225],[255,200,60],[255,110,20],[190,28,6]]),
  embers:    row("dot",    "add",   "above", "bottom", 60,  12,   2,    4,    40,   90,   270, 35,  0, 1,    60, 3,  6,   0,    0,    0,   "fade_out",    1.0,  3.0, 0.5, false, 3,  1,  false, [[255,180,60],[255,60,10]]),
  sparkles:  row("star",   "add",   "above", "area",   120, 40,   0.4,  0.9,  0,    20,   270, 180, 0, 1,    0,  8,  22,  0,    -180, 180, "fade_in_out", 1.0,  0,   0,   false, 1,  1,  false, [[255,255,255],[255,220,120]]),
  fireflies: row("soft",   "add",   "above", "area",   25,  0,    0,    0,    10,   30,   0,   180, 0, 1,    80, 6,  10,  0,    0,    0,   "flat",        0.9,  0.5, 0.8, true,  0,  1,  false, [[200,255,120]]),
  meteors:   row("streak", "add",   "above", "right",  12,  0.6,  1.2,  2.0,  500,  800,  160, 8,   0, 1,    0,  6,  10,  0,    0,    0,   "fade_out",    0.9,  0,   0,   false, 0,  8,  true,  [[255,240,200],[255,140,40]]),
  laser:     row("streak", "add",   "above", "point",  40,  0,    0.15, 0.25, 1400, 1800, 0,   1,   0, 1,    0,  8,  12,  0,    0,    0,   "fade_out",    1.0,  0,   0,   false, 0,  10, true,  [[255,60,60]]),
};

/** Effective SOURCE-unit values for a JSON entry (preset + overrides) — used by
 *  the panel for placeholders and by the sim (after scaling). Mirrors the
 *  C++ load order: preset first, then every explicit field overrides. */
export interface FxResolved {
  name: string; preset: string; slot: FxSlot; sprite: number; blend: "add" | "blend"; spawnMode: number;
  maxN: number; rate: number; burst: number; durationMs: number; idle: boolean; triggers: string[];
  wrap: boolean; lifeMin: number; lifeMax: number; speedMin: number; speedMax: number;
  angle: number; spread: number; gravity: number; drag: number; windX: number; windY: number; wander: number;
  sizeMin: number; sizeMax: number; grow: number; aspect: number; align: boolean; rotMin: number; rotMax: number;
  ramp: FxColor[]; alphaCurve: number; alphaMax: number; twinkleHz: number; twinkleAmt: number; prewarmS: number;
  pulsePeriod: number; pulseDepth: number;
  area: [number, number, number, number] | null; point: [number, number] | null; clip: [number, number, number, number] | null;
}

function slotFromName(s: string | undefined, def: FxSlot): FxSlot {
  if (s === "below" || s === "below_lamps" || s === "behind") return 0;
  if (s === "above" || s === "above_lamps") return 1;
  if (s === "top") return 2;
  return def;
}
function spawnFromName(s: string | undefined, def: number): number {
  const i = ["area", "top", "bottom", "left", "right", "point"].indexOf(s ?? "");
  return i < 0 ? def : i;
}
function alphaCurveFromName(s: string | undefined, def: number): number {
  const i = ["flat", "fade_out", "fade_in_out", "fade_in"].indexOf(s ?? "");
  return i < 0 ? def : i;
}
function spriteFromName(s: string | undefined): number {
  const i = SPRITE_NAMES.indexOf((s ?? "soft") as any);
  return i < 0 ? 0 : i;
}
function rangeOf(v: number | [number, number] | undefined, lo: number, hi: number): [number, number] {
  if (v === undefined || v === null) return [lo, hi];
  if (typeof v === "number") return [v, v];
  if (Array.isArray(v)) {
    const arr = v as number[];
    if (arr.length >= 2) { const a = +arr[0], b = +arr[1]; return b < a ? [b, a] : [a, b]; }
    if (arr.length === 1) return [+arr[0], +arr[0]];
  }
  return [lo, hi];
}
function rampOf(c: FxEmitterJson["color"], def: FxColor[]): FxColor[] {
  if (!Array.isArray(c) || c.length === 0) return def;
  if (typeof c[0] === "number") return [c as FxColor];
  const r = (c as FxColor[]).filter(e => Array.isArray(e) && e.length >= 3);
  return r.length ? r : def;
}
function triggersOf(t: FxEmitterJson["trigger"], idleDefault: boolean): { triggers: string[]; idle: boolean } {
  if (t === undefined) return { triggers: [], idle: idleDefault };
  const list = typeof t === "string" ? [t] : Array.isArray(t) ? t.filter(x => typeof x === "string") : [];
  const named = list.filter(s => s !== "idle" && s !== "always");
  const hasIdle = list.some(s => s === "idle" || s === "always");
  return { triggers: named, idle: hasIdle ? true : (named.length ? false : idleDefault) };
}

/** Resolve preset + overrides to SOURCE units (no scaling). */
export function resolveEmitter(j: FxEmitterJson): FxResolved {
  const p = PRESETS[j.preset ?? ""] ?? null;
  const r: FxResolved = {
    name: j.name ?? "emitter", preset: j.preset ?? "",
    slot: slotFromName(p?.z, 1), sprite: spriteFromName(p?.sprite), blend: (p?.blend === "add" ? "add" : p ? "blend" : "add"),
    spawnMode: spawnFromName(p?.spawn, 0),
    maxN: p?.maxN ?? 100, rate: p?.rate ?? 0, burst: 0, durationMs: 0, idle: true, triggers: [],
    wrap: p?.wrap ?? false, lifeMin: p?.lifeMin ?? 1, lifeMax: p?.lifeMax ?? 2,
    speedMin: p?.speedMin ?? 0, speedMax: p?.speedMax ?? 0, angle: p?.angle ?? 270, spread: p?.spread ?? 0,
    gravity: p?.gravity ?? 0, drag: p?.drag ?? 1, windX: 0, windY: 0, wander: p?.wander ?? 0,
    sizeMin: p?.sizeMin ?? 4, sizeMax: p?.sizeMax ?? 8, grow: p?.grow ?? 0, aspect: p?.aspect ?? 1, align: p?.align ?? false,
    rotMin: p?.rotMin ?? 0, rotMax: p?.rotMax ?? 0, ramp: p?.ramp ?? [[255, 255, 255]],
    alphaCurve: alphaCurveFromName(p?.alpha, 1), alphaMax: p?.alphaMax ?? 1, twinkleHz: p?.twHz ?? 0, twinkleAmt: p?.twAmt ?? 0,
    prewarmS: p?.prewarm ?? 0, pulsePeriod: 0, pulseDepth: 0, area: null, point: null, clip: null,
  };
  // explicit overrides (same precedence as C++)
  r.slot = slotFromName(j.z, r.slot);
  if (j.sprite !== undefined) r.sprite = spriteFromName(j.sprite);
  if (j.blend !== undefined) r.blend = j.blend === "add" ? "add" : "blend";
  if (j.spawn !== undefined) r.spawnMode = spawnFromName(j.spawn, r.spawnMode);
  if (typeof j.max === "number") r.maxN = Math.min(MAX_PER_EMITTER, Math.max(1, Math.trunc(j.max)));
  if (typeof j.rate === "number") r.rate = j.rate;
  if (typeof j.burst === "number") r.burst = Math.trunc(j.burst);
  if (typeof j.duration_ms === "number") r.durationMs = Math.max(0, Math.trunc(j.duration_ms));
  if (typeof j.wrap === "boolean") r.wrap = j.wrap;
  [r.lifeMin, r.lifeMax] = rangeOf(j.life_s, r.lifeMin, r.lifeMax);
  [r.speedMin, r.speedMax] = rangeOf(j.speed, r.speedMin, r.speedMax);
  if (typeof j.angle === "number") r.angle = j.angle;
  if (typeof j.spread === "number") r.spread = j.spread;
  if (typeof j.gravity === "number") r.gravity = j.gravity;
  if (typeof j.drag === "number") r.drag = Math.min(1, Math.max(0, j.drag));
  if (Array.isArray(j.wind) && j.wind.length >= 2) { r.windX = +j.wind[0]; r.windY = +j.wind[1]; }
  if (typeof j.wander === "number") r.wander = j.wander;
  { const [lo, hi] = rangeOf(j.size, r.sizeMin, r.sizeMax); r.sizeMin = lo; r.sizeMax = Math.max(lo, hi); }
  if (typeof j.grow === "number") r.grow = j.grow;
  [r.rotMin, r.rotMax] = rangeOf(j.rotate, r.rotMin, r.rotMax);
  r.ramp = rampOf(j.color, r.ramp);
  r.alphaCurve = alphaCurveFromName(j.alpha, r.alphaCurve);
  if (typeof j.alpha_max === "number") r.alphaMax = Math.min(1, Math.max(0, j.alpha_max));
  if (typeof j.twinkle_hz === "number") r.twinkleHz = j.twinkle_hz;
  if (typeof j.twinkle === "number") r.twinkleAmt = Math.min(1, Math.max(0, j.twinkle));
  if (typeof j.aspect === "number") r.aspect = Math.max(0.1, j.aspect);
  if (typeof j.align === "boolean") r.align = j.align;
  if (typeof j.prewarm_s === "number") r.prewarmS = j.prewarm_s;
  if (j.pulse && typeof j.pulse === "object") { r.pulsePeriod = j.pulse.period_ms ?? 3000; r.pulseDepth = Math.min(1, Math.max(0, j.pulse.depth ?? 0.5)); }
  if (Array.isArray(j.area) && j.area.length >= 4) r.area = [+j.area[0], +j.area[1], +j.area[2], +j.area[3]];
  if (Array.isArray(j.point) && j.point.length >= 2) { r.point = [+j.point[0], +j.point[1]]; if (j.spawn === undefined) r.spawnMode = 5; }
  if (Array.isArray(j.clip) && j.clip.length >= 4) r.clip = [+j.clip[0], +j.clip[1], +j.clip[2], +j.clip[3]];
  const t = triggersOf(j.trigger, true); r.triggers = t.triggers; r.idle = t.idle;
  if (r.lifeMin <= 0 && r.lifeMax <= 0) { r.lifeMin = r.lifeMax = 0; }
  return r;
}

/** Preset defaults in source units (for panel placeholders). */
export function presetDefaults(preset: string): FxResolved { return resolveEmitter({ preset }); }

// ── runtime ───────────────────────────────────────────────────────────────────
interface Particle {
  x: number; y: number; vx: number; vy: number; age: number; life: number; maxLife: number;
  size: number; rot: number; rotV: number; seed: number; alive: boolean;
}
interface Emitter {
  r: FxResolved;              // SCREEN units after scaling (speed/size/gravity/wander/wind)
  area: { x: number; y: number; w: number; h: number };
  px: number; py: number;
  clip: { x: number; y: number; w: number; h: number } | null;
  pool: Particle[]; spawnAcc: number; activeUntil: number; lastMs: number; clock: number;
}
interface Overlay {
  j: FxOverlayJson; img: HTMLImageElement | null; dst: { x: number; y: number; w: number; h: number };
  slot: FxSlot; blend: "add" | "blend"; alpha: number; pivotX: number; pivotY: number;
  breathePeriod: number; breatheSX: number; breatheSY: number; breatheAlpha: number;
  swayPeriod: number; swayDeg: number; bobPeriod: number; bobPx: number;
  driftVX: number; driftVY: number; driftWrap: boolean;
  pulsePeriod: number; pulseDepth: number;
  warp: number; warpAmp: number; warpPeriod: number; warpFreq: number; cols: number; rows: number;
  frameCols: number; frameRows: number; frameCount: number; frameFps: number;
  idle: boolean; triggers: string[]; durationMs: number; fadeMs: number;
  activeUntil: number; activeSince: number; offX: number; offY: number; lastMs: number;
}

const PI = Math.PI;
const frand = () => Math.random();
const frange = (a: number, b: number) => a + (b - a) * Math.random();
const clamp = (v: number, lo: number, hi: number) => v < lo ? lo : v > hi ? hi : v;

function hash2(x: number, y: number): number {
  let h = (Math.imul(x, 374761393) + Math.imul(y, 668265263)) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177) >>> 0;
  return (h & 0xffff) / 65535;
}
function vnoise(x: number, y: number): number {
  const xi = Math.floor(x), yi = Math.floor(y);
  let fx = x - xi, fy = y - yi;
  fx = fx * fx * (3 - 2 * fx); fy = fy * fy * (3 - 2 * fy);
  const a = hash2(xi, yi), b = hash2(xi + 1, yi), c = hash2(xi, yi + 1), d = hash2(xi + 1, yi + 1);
  const top = a + (b - a) * fx, bot = c + (d - c) * fx;
  return top + (bot - top) * fy;
}

/** Procedural white sprites — same formulas as makeSprite(). */
function makeSprite(kind: number): HTMLCanvasElement {
  const N = SPRITE_SIZE;
  const cv = document.createElement("canvas"); cv.width = N; cv.height = N;
  const ctx = cv.getContext("2d")!;
  const img = ctx.createImageData(N, N);
  const c = (N - 1) * 0.5;
  for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) {
    const dx = (x - c) / c, dy = (y - c) / c, r = Math.sqrt(dx * dx + dy * dy);
    let a = 0;
    switch (kind) {
      case 0: { let t = clamp(1 - r, 0, 1); a = t * t * (3 - 2 * t); a *= a; break; }
      case 1: a = clamp((0.92 - r) * c, 0, 1); break;
      case 2: { const lx = clamp(1 - Math.abs(dx) * 4, 0, 1), ly = clamp(1 - Math.abs(dy), 0, 1);
                const head = 0.55 + 0.45 * clamp(dy + 0.2, 0, 1); a = lx * lx * ly * head; break; }
      case 3: { let core = clamp(1 - r * 1.6, 0, 1); core *= core;
                const ax = clamp(1 - Math.abs(dy) * 10, 0, 1) * clamp(1 - Math.abs(dx), 0, 1);
                const ay = clamp(1 - Math.abs(dx) * 10, 0, 1) * clamp(1 - Math.abs(dy), 0, 1);
                a = clamp(core + ax * ax + ay * ay, 0, 1); break; }
      case 4: { let t = clamp(1 - r, 0, 1); t = t * t * (3 - 2 * t); a = t * (0.55 + 0.45 * vnoise(x * 0.18, y * 0.18)); break; }
      case 5: { const d = Math.abs(r - 0.7); a = clamp(1 - d * 5, 0, 1); a *= a; break; }
    }
    const i = (y * N + x) * 4;
    img.data[i] = 255; img.data[i + 1] = 255; img.data[i + 2] = 255; img.data[i + 3] = Math.round(clamp(a, 0, 1) * 255);
  }
  ctx.putImageData(img, 0, 0);
  return cv;
}

function rampAt(ramp: FxColor[], t: number): [number, number, number, number] {
  if (ramp.length === 0) return [255, 255, 255, 255];
  const A = (c: FxColor) => (c.length >= 4 ? c[3]! : 255);
  if (ramp.length === 1 || t <= 0) { const c = ramp[0]; return [c[0], c[1], c[2], A(c)]; }
  if (t >= 1) { const c = ramp[ramp.length - 1]; return [c[0], c[1], c[2], A(c)]; }
  const f = t * (ramp.length - 1), i = Math.floor(f), u = f - i;
  if (i + 1 >= ramp.length) { const c = ramp[ramp.length - 1]; return [c[0], c[1], c[2], A(c)]; }
  const a = ramp[i], b = ramp[i + 1];
  // C++ truncates to uint8 after lerp
  return [Math.trunc(a[0] + (b[0] - a[0]) * u), Math.trunc(a[1] + (b[1] - a[1]) * u),
          Math.trunc(a[2] + (b[2] - a[2]) * u), Math.trunc(A(a) + (A(b) - A(a)) * u)];
}
function alphaCurve(curve: number, t: number): number {
  switch (curve) {
    case 1: return 1 - t;
    case 2: return t < 0.25 ? t / 0.25 : (t > 0.7 ? (1 - t) / 0.3 : 1);
    case 3: return t < 0.3 ? t / 0.3 : 1;
    default: return 1;
  }
}

/** Live simulation for one table. Create once per B2SCanvas; call load() when
 *  the doc or scale changes; draw(slot) from the render loop in PI SCREEN
 *  coordinates (1920×1080 logical space — set the transform before calling). */
export class FxSim {
  private emitters: Emitter[] = [];
  private overlays: Overlay[] = [];
  private sprites: HTMLCanvasElement[] = [];
  private tinted = new Map<string, HTMLCanvasElement>();
  private W = 1920; private H = 1080;
  private sx = 1; private sy = 1;
  /** Resolve an overlay image path to something <img> can load (data URL,
   *  asset URL). Returns null to skip. Set by the host. */
  imageResolver: ((path: string) => Promise<string | null>) | null = null;
  stats = { live: 0, emitters: 0, overlays: 0, lastMs: 0 };

  get emitterCount() { return this.emitters.length; }
  get scaleX() { return this.sx; }
  get scaleY() { return this.sy; }

  /** Screen-space area rect of emitter i (for the editor outline). */
  emitterArea(i: number) { return this.emitters[i]?.area ?? null; }
  /** Screen-space rect of overlay i (with drift offset) for the editor. */
  overlayRect(i: number) { const o = this.overlays[i]; return o && o.img ? { x: o.dst.x + o.offX, y: o.dst.y + o.offY, w: o.dst.w, h: o.dst.h } : null; }
  /** Move overlay i (screen px) without reloading its texture. */
  setOverlayPos(i: number, x: number, y: number) { const o = this.overlays[i]; if (o) { o.dst.x = x; o.dst.y = y; } }
  emitterPoint(i: number) { const e = this.emitters[i]; return e && e.r.spawnMode === 5 ? { x: e.px, y: e.py } : null; }

  load(doc: FxDoc | null, sx: number, sy: number, W = 1920, H = 1080) {
    this.emitters = []; this.overlays = [];
    this.sx = sx; this.sy = sy; this.W = W; this.H = H;
    if (!doc) return;
    const s = (sx + sy) * 0.5;
    let budget = MAX_PARTICLES_TOTAL;
    for (const j of (doc.particles ?? []).slice(0, MAX_EMITTERS)) {
      if (!j || typeof j !== "object") continue;
      if (j.enabled === false) continue;
      const src = resolveEmitter(j);
      const r: FxResolved = { ...src,
        speedMin: src.speedMin * s, speedMax: src.speedMax * s, gravity: src.gravity * s, wander: src.wander * s,
        windX: src.windX * s, windY: src.windY * s,
        sizeMin: Math.max(1, src.sizeMin * s), sizeMax: Math.max(Math.max(1, src.sizeMin * s), src.sizeMax * s),
      };
      const area = src.area
        ? { x: src.area[0] * sx, y: src.area[1] * sy, w: src.area[2] * sx, h: src.area[3] * sy }
        : { x: 0, y: 0, w: W, h: H };
      const px = src.point ? src.point[0] * sx : area.x + area.w * 0.5;
      const py = src.point ? src.point[1] * sy : area.y + area.h * 0.5;
      const clip = src.clip ? { x: Math.trunc(src.clip[0] * sx), y: Math.trunc(src.clip[1] * sy),
                                w: Math.trunc(src.clip[2] * sx + 0.5), h: Math.trunc(src.clip[3] * sy + 0.5) } : null;
      if (r.maxN > budget) r.maxN = budget;
      if (r.maxN <= 0) continue;
      budget -= r.maxN;
      const e: Emitter = { r, area, px, py, clip, pool: [], spawnAcc: 0, activeUntil: 0, lastMs: 0, clock: 0 };
      for (let i = 0; i < r.maxN; i++) e.pool.push({ x: 0, y: 0, vx: 0, vy: 0, age: 0, life: 0, maxLife: 0, size: 1, rot: 0, rotV: 0, seed: 0, alive: false });
      if (r.idle) {
        if (r.wrap || (r.lifeMax <= 0 && r.rate <= 0)) {
          for (let i = 0; i < e.pool.length; i++) this.spawnOne(e);
          for (const p of e.pool) if (p.alive) { p.x = area.x + frand() * area.w; p.y = area.y + frand() * area.h; p.age = frand() * 10; }
        } else if (r.prewarmS > 0) {
          const step = 1 / 30;
          for (let t = 0; t < r.prewarmS; t += step) this.stepEmitter(e, step, 0);
        }
      }
      this.emitters.push(e);
    }
    for (const j of (doc.overlays ?? []).slice(0, MAX_OVERLAYS)) {
      if (!j || typeof j !== "object" || j.enabled === false || !j.image) continue;
      const o: Overlay = {
        j, img: null, dst: { x: 0, y: 0, w: 0, h: 0 },
        slot: slotFromName(j.z, 1), blend: j.blend === "add" ? "add" : "blend", alpha: clamp(j.alpha ?? 1, 0, 1),
        pivotX: clamp(j.pivot?.[0] ?? 0.5, 0, 1), pivotY: clamp(j.pivot?.[1] ?? 0.5, 0, 1),
        breathePeriod: j.breathe ? (j.breathe.period_ms ?? 4000) : 0, breatheSX: j.breathe?.scale_x ?? 0,
        breatheSY: j.breathe ? (j.breathe.scale_y ?? 0.02) : 0, breatheAlpha: clamp(j.breathe?.alpha ?? 0, 0, 1),
        swayPeriod: j.sway ? (j.sway.period_ms ?? 3000) : 0, swayDeg: j.sway ? (j.sway.deg ?? 2) : 0,
        bobPeriod: j.bob ? (j.bob.period_ms ?? 3000) : 0, bobPx: j.bob ? (j.bob.px ?? 6) * sy : 0,
        driftVX: (j.drift?.vx ?? 0) * sx, driftVY: (j.drift?.vy ?? 0) * sy, driftWrap: !!j.drift?.wrap,
        pulsePeriod: j.pulse ? (j.pulse.period_ms ?? 3000) : 0, pulseDepth: j.pulse ? clamp(j.pulse.depth ?? 0.5, 0, 1) : 0,
        warp: j.warp ? ({ wave: 1, flag: 2, bulge: 3, jelly: 4 } as any)[j.warp.type ?? "wave"] ?? 0 : 0,
        warpAmp: (j.warp?.amp ?? 6) * s, warpPeriod: Math.max(50, j.warp?.period_ms ?? 2000), warpFreq: j.warp?.freq ?? 1,
        cols: 1, rows: 1,
        frameCols: Math.max(1, Math.trunc(j.frames?.cols ?? 1)), frameRows: Math.max(1, Math.trunc(j.frames?.rows ?? 1)),
        frameCount: 1, frameFps: Math.max(0, j.frames?.fps ?? 12),
        idle: true, triggers: [], durationMs: Math.max(0, Math.trunc(j.duration_ms ?? 0)),
        fadeMs: Math.max(0, Math.trunc(j.fade_ms ?? 300)),
        activeUntil: 0, activeSince: 0, offX: 0, offY: 0, lastMs: 0,
      };
      o.frameCount = Math.min(o.frameCols * o.frameRows, Math.max(1, Math.trunc(j.frames?.count ?? o.frameCols * o.frameRows)));
      if (o.warp) o.cols = o.rows = 12;
      if (typeof j.mesh === "number") o.cols = o.rows = Math.min(MAX_MESH, Math.max(1, Math.trunc(j.mesh)));
      const t = triggersOf(j.trigger, true); o.triggers = t.triggers; o.idle = t.idle;
      this.overlays.push(o);
      this.loadOverlayImage(o);
    }
    if (this.emitters.length || this.overlays.length) this.ensureSprites();
    this.stats.emitters = this.emitters.length; this.stats.overlays = this.overlays.length;
  }

  private async loadOverlayImage(o: Overlay) {
    if (!this.imageResolver) return;
    const url = await this.imageResolver(o.j.image).catch(() => null);
    if (!url) return;
    const img = new Image();
    img.onload = () => {
      const r = o.j.rect;
      const rx = r?.[0] ?? 0, ry = r?.[1] ?? 0;
      const rw = r && r[2] > 0 ? r[2] : img.naturalWidth / o.frameCols;    // one frame
      const rh = r && r[3] > 0 ? r[3] : img.naturalHeight / o.frameRows;
      o.dst = { x: rx * this.sx, y: ry * this.sy, w: rw * this.sx, h: rh * this.sy };
      o.img = img;
    };
    img.src = url;
  }

  private ensureSprites() {
    if (this.sprites.length) return;
    for (let k = 0; k < SPRITE_NAMES.length; k++) this.sprites.push(makeSprite(k));
  }

  /** Tinted sprite cache — canvas has no per-draw color modulation, so tint
   *  once per (kind, quantized color) with source-in. */
  private tintedSprite(kind: number, r: number, g: number, b: number): HTMLCanvasElement {
    const qr = r >> 3, qg = g >> 3, qb = b >> 3;
    const key = `${kind}:${qr},${qg},${qb}`;
    let cv = this.tinted.get(key);
    if (cv) return cv;
    cv = document.createElement("canvas"); cv.width = SPRITE_SIZE; cv.height = SPRITE_SIZE;
    const ctx = cv.getContext("2d")!;
    ctx.drawImage(this.sprites[kind], 0, 0);
    ctx.globalCompositeOperation = "source-in";
    ctx.fillStyle = `rgb(${qr << 3 | 4},${qg << 3 | 4},${qb << 3 | 4})`;
    ctx.fillRect(0, 0, SPRITE_SIZE, SPRITE_SIZE);
    if (this.tinted.size > 2048) this.tinted.clear();
    this.tinted.set(key, cv);
    return cv;
  }

  trigger(key: string, now: number) {
    for (const e of this.emitters) {
      if (!e.r.triggers.includes(key)) continue;
      for (let i = 0; i < e.r.burst; i++) this.spawnOne(e);
      if (e.r.durationMs > 0) e.activeUntil = now + e.r.durationMs;
    }
    for (const o of this.overlays) {
      if (!o.triggers.includes(key)) continue;
      o.activeSince = now; o.activeUntil = now + (o.durationMs > 0 ? o.durationMs : 2000);
    }
  }

  private spawnOne(e: Emitter) {
    const p = e.pool.find(q => !q.alive);
    if (!p) return;
    const a = e.area, r = e.r;
    switch (r.spawnMode) {
      case 1: p.x = a.x + frand() * a.w; p.y = a.y - r.sizeMax; break;
      case 2: p.x = a.x + frand() * a.w; p.y = a.y + a.h + r.sizeMax * 0.5; break;
      case 3: p.x = a.x - r.sizeMax; p.y = a.y + frand() * a.h; break;
      case 4: p.x = a.x + a.w + r.sizeMax; p.y = a.y + frand() * a.h; break;
      case 5: p.x = e.px; p.y = e.py; break;
      default: p.x = a.x + frand() * a.w; p.y = a.y + frand() * a.h; break;
    }
    const ang = (r.angle + frange(-r.spread, r.spread)) * PI / 180;
    const spd = frange(r.speedMin, r.speedMax);
    p.vx = Math.cos(ang) * spd; p.vy = Math.sin(ang) * spd;
    p.maxLife = r.lifeMax > 0 ? frange(r.lifeMin, r.lifeMax) : 0;
    p.life = p.maxLife > 0 ? p.maxLife : -1;
    p.age = 0;
    p.size = frange(r.sizeMin, r.sizeMax);
    p.rot = frand() * 360; p.rotV = frange(r.rotMin, r.rotMax);
    p.seed = frand() * 6.2831;
    p.alive = true;
  }

  private stepEmitter(e: Emitter, dt: number, now: number) {
    const r = e.r;
    e.clock += dt;
    const emitting = r.idle || (e.activeUntil !== 0 && now !== 0 && now < e.activeUntil);
    if (emitting && r.rate > 0) {
      e.spawnAcc += r.rate * dt;
      const n = Math.trunc(e.spawnAcc);
      if (n > 0) { e.spawnAcc -= n; for (let i = 0; i < n; i++) this.spawnOne(e); }
    }
    const dragK = r.drag >= 1 ? 1 : Math.pow(r.drag, dt);
    const a = e.area, margin = r.sizeMax * 2 + 4;
    for (const p of e.pool) {
      if (!p.alive) continue;
      p.age += dt;
      if (p.life >= 0) { p.life -= dt; if (p.life <= 0) { p.alive = false; continue; } }
      p.vx += r.windX * dt; p.vy += (r.gravity + r.windY) * dt;
      if (r.wander > 0) {
        p.vx += r.wander * Math.sin(e.clock * 1.7 + p.seed) * dt;
        p.vy += r.wander * 0.5 * Math.cos(e.clock * 1.3 + p.seed * 1.9) * dt;
      }
      p.vx *= dragK; p.vy *= dragK;
      p.x += p.vx * dt; p.y += p.vy * dt;
      p.rot += p.rotV * dt;
      if (r.wrap) {
        if (p.x < a.x - margin) p.x += a.w + 2 * margin; else if (p.x > a.x + a.w + margin) p.x -= a.w + 2 * margin;
        if (p.y < a.y - margin) p.y += a.h + 2 * margin; else if (p.y > a.y + a.h + margin) p.y -= a.h + 2 * margin;
      } else if (p.x < a.x - margin || p.x > a.x + a.w + margin || p.y < a.y - margin || p.y > a.y + a.h + margin) {
        p.alive = false;
      }
    }
  }

  private drawEmitter(ctx: CanvasRenderingContext2D, e: Emitter) {
    const r = e.r;
    ctx.save();
    ctx.globalCompositeOperation = r.blend === "add" ? "lighter" : "source-over";
    if (e.clip) { ctx.beginPath(); ctx.rect(e.clip.x, e.clip.y, e.clip.w, e.clip.h); ctx.clip(); }
    const pulseMul = (r.pulsePeriod > 0 && r.pulseDepth > 0)
      ? 1 - r.pulseDepth * (0.5 - 0.5 * Math.sin(performance.now() * 2 * PI / r.pulsePeriod)) : 1;
    for (const p of e.pool) {
      if (!p.alive) continue;
      const t = p.maxLife > 0 ? clamp(1 - p.life / p.maxLife, 0, 1) : 0;
      let a = alphaCurve(r.alphaCurve, t) * r.alphaMax * pulseMul;
      if (r.twinkleAmt > 0 && r.twinkleHz > 0)
        a *= (1 - r.twinkleAmt) + r.twinkleAmt * (0.5 + 0.5 * Math.sin(p.age * r.twinkleHz * 6.2831 + p.seed));
      if (a <= 0.003) continue;
      const c = rampAt(r.ramp, t);
      const size = p.size * (1 + r.grow * t);
      if (size < 0.5) continue;
      const w = size, h = size * r.aspect;
      const angle = r.align ? (Math.atan2(p.vy, p.vx) * 180 / PI - 90) : p.rot;
      const spr = this.tintedSprite(r.sprite, c[0], c[1], c[2]);
      ctx.globalAlpha = clamp(a, 0, 1) * (c[3] / 255);
      if (angle !== 0) {
        ctx.setTransform(this.baseT.a, this.baseT.b, this.baseT.c, this.baseT.d, this.baseT.e, this.baseT.f);
        ctx.translate(p.x, p.y); ctx.rotate(angle * PI / 180);
        ctx.drawImage(spr, -w / 2, -h / 2, w, h);
        ctx.setTransform(this.baseT.a, this.baseT.b, this.baseT.c, this.baseT.d, this.baseT.e, this.baseT.f);
      } else {
        ctx.drawImage(spr, p.x - w / 2, p.y - h / 2, w, h);
      }
    }
    ctx.restore();
  }

  private baseT = { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 };

  private drawOverlay(ctx: CanvasRenderingContext2D, o: Overlay, now: number) {
    if (!o.img) return;
    if (!o.lastMs) o.lastMs = now;
    const dt = clamp((now - o.lastMs) / 1000, 0, 0.05);
    o.lastMs = now;
    let vis = 1;
    if (!o.idle) {
      if (o.activeUntil === 0 || now >= o.activeUntil) return;
      const tin = now - o.activeSince, left = o.activeUntil - now;
      if (o.fadeMs > 0) { if (tin < o.fadeMs) vis = tin / o.fadeMs; if (left < o.fadeMs) vis = Math.min(vis, left / o.fadeMs); }
    }
    const tsec = now / 1000;
    const phase = (periodMs: number) => periodMs > 0 ? Math.sin(tsec * 2 * PI * 1000 / periodMs) : 0;
    if (o.driftVX !== 0 || o.driftVY !== 0) {
      o.offX += o.driftVX * dt; o.offY += o.driftVY * dt;
      if (o.driftWrap) {
        const spanX = this.W + o.dst.w, spanY = this.H + o.dst.h;
        if (o.dst.x + o.offX > this.W) o.offX -= spanX; else if (o.dst.x + o.dst.w + o.offX < 0) o.offX += spanX;
        if (o.dst.y + o.offY > this.H) o.offY -= spanY; else if (o.dst.y + o.dst.h + o.offY < 0) o.offY += spanY;
      }
    }
    const bp = phase(o.breathePeriod);
    const scaleX = 1 + o.breatheSX * bp, scaleY = 1 + o.breatheSY * bp;
    let alpha = o.alpha * vis * (1 - o.breatheAlpha * (0.5 - 0.5 * bp));
    if (o.pulsePeriod > 0 && o.pulseDepth > 0) alpha *= 1 - o.pulseDepth * (0.5 - 0.5 * phase(o.pulsePeriod));
    const swayRad = o.swayDeg * phase(o.swayPeriod) * PI / 180;
    const bob = o.bobPx * phase(o.bobPeriod);
    const pvx = o.dst.x + o.dst.w * o.pivotX, pvy = o.dst.y + o.dst.h * o.pivotY;
    const cs = Math.cos(swayRad), sn = Math.sin(swayRad);
    const wphase = tsec * 2 * PI * 1000 / o.warpPeriod;
    ctx.save();
    ctx.globalCompositeOperation = o.blend === "add" ? "lighter" : "source-over";
    ctx.globalAlpha = clamp(alpha, 0, 1);
    const cols = o.cols, rows = o.rows;
    // current sprite-sheet frame → source sub-rect
    const fi = (o.frameCount > 1 && o.frameFps > 0) ? Math.trunc(tsec * o.frameFps) % o.frameCount : 0;
    const iw = o.img.naturalWidth / o.frameCols, ih = o.img.naturalHeight / o.frameRows;
    const fox = (fi % o.frameCols) * iw, foy = Math.trunc(fi / o.frameCols) * ih;
    // vertex grid (same displacement math as the Pi's mesh)
    const vx: number[] = [], vy: number[] = [];
    const maxd = 0.5 * Math.sqrt(o.dst.w * o.dst.w + o.dst.h * o.dst.h);
    for (let rr = 0; rr <= rows; rr++) for (let cc = 0; cc <= cols; cc++) {
      const u = cc / cols, v = rr / rows;
      const lx = o.dst.x + u * o.dst.w, ly = o.dst.y + v * o.dst.h;
      let dxw = 0, dyw = 0;
      switch (o.warp) {
        case 1: dxw = o.warpAmp * Math.sin(v * o.warpFreq * 2 * PI + wphase) * (1 - v * o.pivotY); break;
        case 2: dyw = o.warpAmp * Math.sin(u * o.warpFreq * 2 * PI + wphase) * (0.25 + 0.75 * u); break;
        case 3: { const ddx = lx - pvx, ddy = ly - pvy, d = Math.sqrt(ddx * ddx + ddy * ddy);
                  let k = 1 - clamp(d / Math.max(1, maxd), 0, 1); k *= k;
                  const sgn = o.warpAmp * Math.sin(wphase) * k / Math.max(1, maxd) * 4;
                  dxw = ddx * sgn; dyw = ddy * sgn; break; }
        case 4: dxw = o.warpAmp * 0.5 * Math.sin(v * 3 * PI + wphase) * Math.sin(u * PI);
                dyw = o.warpAmp * 0.5 * Math.cos(u * 3 * PI + wphase * 0.8) * Math.sin(v * PI); break;
      }
      let x = lx + dxw, y = ly + dyw;
      x = pvx + (x - pvx) * scaleX; y = pvy + (y - pvy) * scaleY;
      const rx = pvx + (x - pvx) * cs - (y - pvy) * sn;
      const ry = pvy + (x - pvx) * sn + (y - pvy) * cs;
      vx.push(rx + o.offX); vy.push(ry + o.offY + bob);
    }
    // draw each cell as an affine-mapped quad (two triangles) — canvas has no
    // textured triangles, so clip each cell and map its texture affinely.
    for (let rr = 0; rr < rows; rr++) for (let cc = 0; cc < cols; cc++) {
      const i0 = rr * (cols + 1) + cc, i1 = i0 + 1, i2 = i0 + cols + 1, i3 = i2 + 1;
      const su = iw / cols, sv = ih / rows;
      const tx0 = fox + cc * su, ty0 = foy + rr * sv;
      this.texTri(ctx, o.img, [vx[i0], vy[i0]], [vx[i1], vy[i1]], [vx[i2], vy[i2]], [tx0, ty0], [tx0 + su, ty0], [tx0, ty0 + sv]);
      this.texTri(ctx, o.img, [vx[i1], vy[i1]], [vx[i3], vy[i3]], [vx[i2], vy[i2]], [tx0 + su, ty0], [tx0 + su, ty0 + sv], [tx0, ty0 + sv]);
    }
    ctx.restore();
  }

  /** Affine-mapped textured triangle (standard canvas technique). */
  private texTri(ctx: CanvasRenderingContext2D, img: HTMLImageElement,
    p0: [number, number], p1: [number, number], p2: [number, number],
    t0: [number, number], t1: [number, number], t2: [number, number]) {
    const x0 = p0[0], y0 = p0[1], x1 = p1[0], y1 = p1[1], x2 = p2[0], y2 = p2[1];
    const u0 = t0[0], v0 = t0[1], u1 = t1[0], v1 = t1[1], u2 = t2[0], v2 = t2[1];
    const det = (u1 - u0) * (v2 - v0) - (u2 - u0) * (v1 - v0);
    if (Math.abs(det) < 1e-6) return;
    const a = ((x1 - x0) * (v2 - v0) - (x2 - x0) * (v1 - v0)) / det;
    const b = ((y1 - y0) * (v2 - v0) - (y2 - y0) * (v1 - v0)) / det;
    const c = ((x2 - x0) * (u1 - u0) - (x1 - x0) * (u2 - u0)) / det;
    const d = ((y2 - y0) * (u1 - u0) - (y1 - y0) * (u2 - u0)) / det;
    const e = x0 - a * u0 - c * v0, f = y0 - b * u0 - d * v0;
    ctx.save();
    ctx.beginPath(); ctx.moveTo(x0, y0); ctx.lineTo(x1, y1); ctx.lineTo(x2, y2); ctx.closePath(); ctx.clip();
    ctx.transform(a, b, c, d, e, f);
    ctx.drawImage(img, 0, 0);
    ctx.restore();
  }

  /** Update + draw everything in `slot`. ctx must already carry the
   *  logical→canvas transform (1920×1080 space). */
  draw(ctx: CanvasRenderingContext2D, slot: FxSlot, now: number) {
    if (!this.emitters.length && !this.overlays.length) return;
    const T = ctx.getTransform();
    this.baseT = { a: T.a, b: T.b, c: T.c, d: T.d, e: T.e, f: T.f };
    for (const o of this.overlays) if (o.slot === slot) this.drawOverlay(ctx, o, now);
    let live = 0;
    for (const e of this.emitters) {
      if (e.r.slot !== slot) continue;
      if (!e.lastMs) e.lastMs = now;
      const dt = clamp((now - e.lastMs) / 1000, 0, 0.05);
      e.lastMs = now;
      if (dt > 0) this.stepEmitter(e, dt, now);
      this.drawEmitter(ctx, e);
      for (const p of e.pool) if (p.alive) live++;
    }
    if (slot === 2) { this.stats.live = this.liveCount(); this.stats.lastMs = now; }
  }

  liveCount(): number {
    let n = 0; for (const e of this.emitters) for (const p of e.pool) if (p.alive) n++; return n;
  }
}

/** Pull the FX doc out of a parsed event_map (or null if none). */
export function fxDocFromEventMap(em: any): FxDoc {
  return {
    particles: Array.isArray(em?.particles) ? em.particles.filter((e: any) => e && typeof e === "object") : [],
    overlays:  Array.isArray(em?.overlays)  ? em.overlays.filter((e: any) => e && typeof e === "object")  : [],
  };
}

/** Write the FX doc back into an event_map object (drops empty arrays). */
export function fxDocIntoEventMap(em: any, doc: FxDoc): void {
  if (doc.particles.length) em.particles = doc.particles; else delete em.particles;
  if (doc.overlays.length)  em.overlays  = doc.overlays;  else delete em.overlays;
}

/** Particle budget check for the panel: which emitters survive the renderer's
 *  global cap (applied in list order, same as b2s_fx.cpp load()). */
export const FX_PARTICLE_CAP = MAX_PARTICLES_TOTAL;
export const FX_MAX_TEXTURE = 2048;
export function fxBudget(doc: FxDoc): { total: number; rows: { name: string; want: number; got: number }[] } {
  let budget = MAX_PARTICLES_TOTAL, total = 0;
  const rows: { name: string; want: number; got: number }[] = [];
  for (const j of (doc.particles ?? []).slice(0, MAX_EMITTERS)) {
    if (!j || j.enabled === false) continue;
    const want = resolveEmitter(j).maxN;
    const got = Math.max(0, Math.min(want, budget));
    budget -= got; total += want;
    rows.push({ name: j.name ?? "emitter", want, got });
  }
  return { total, rows };
}

/** Human labels for the panel. */
export const PRESET_LABELS: Record<string, string> = {
  starfield: "Starfield (moving stars)", rain: "Rain", snow: "Snow", smoke: "Smoke", fire: "Fire (sparks)", flames: "Flames (dense fire)",
  embers: "Embers", sparkles: "Sparkles", fireflies: "Fireflies", meteors: "Meteors", laser: "Laser fire",
};
