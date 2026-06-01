// Parser for the on-disk b2scache binary format (v11).
//
// **Direct port of the Pi renderer's `b2s_cache.cpp::tryLoadB2SCacheBase +
// readB2SCacheBulbs`.** Mirror those functions byte-for-byte; if Pi changes
// the wire format, this file must change in lockstep. See memory note
// [[feedback-ppdoctor-preview-matches-cabinet]].
//
// Layout (all little-endian; exact order is load-bearing):
//   "B2SC" | u32 version=11 | u64 src_size | u64 src_mtime
//   i32 source_width | i32 source_height | i32 grill_height
//   u32 name_len | <name_len bytes> table_name
//   i32 base_w | i32 base_h | i32 base_pitch
//   <LZ4 block> base BGRA pixels (size = pitch*h)
//   u32 num_bulbs (cap 256 — Pi rejects more as corrupt)
//   for each bulb:
//     i32 id, rom_id, rom_id_type, x, y, w, h
//     i32 initial_state, snippit_type, rotating_steps, rotating_direction, rotating_stop_behaviour
//     u32 name_len | name
//     i32 sprite_off_x, sprite_off_y
//     i32 sprite_w, sprite_h, sprite_pitch    -- all 0 → no sprite block
//     <LZ4 block> sprite BGRA pixels (only if sprite_w/h/pitch > 0; pitch must be ≥ sw*4; w/h ≤ 4096)
//   u32 num_scores (cap 32)
//   for each score:
//     i32 id, x, y, w, h, digits, spacing, player_no, start_digit
//     u8 lit_r, lit_g, lit_b
//     u8 dark_r, dark_g, dark_b
//     str reel_type
//   u32 num_animations (cap 256)
//   for each animation:
//     str name; i32 interval_ms, loops, lights_at_start, lights_at_end, stop_behaviour
//     u8 start_at_startup, u8 lock_involved
//     u32 num_steps (cap 4096)
//     for each step:
//       i32 wait_after_on, wait_after_off
//       u32 on_count (cap 64); on_count × str
//       u32 off_count (cap 64); off_count × str
//
// LZ4 block format:
//   u32 orig_size | u32 comp_size | <comp_size bytes>
//   if orig_size == comp_size → stored uncompressed

// @ts-ignore — lz4js has no types
import lz4 from "lz4js";

export interface B2SCacheBulb {
  id: number;
  romId: number;
  romIdType: number;
  /** Name attribute — name-based addressing for EM tables / animation refs. */
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Initial lit state from .directb2s (0 or 1). */
  initialState: number;
  /** Bbox-crop offset within sprite_w/sprite_h. Sprites are usually larger
   *  than the bulb's logical w/h because the lit aura bleeds outside. */
  spriteOffX: number;
  spriteOffY: number;
  spriteWidth: number;
  spriteHeight: number;
  /** Snippit type (b2s_cache.cpp:380): 0=normal, 1=image-snippet (full-bg
   *  overlay), 2=rotator (SS spider). Non-zero → isSnippit. */
  snippitType: number;
  isSnippit: boolean;
  /** Rotator (snippitType=2) fields — driven by MSG_MECH_STATE on the Pi. */
  rotatingSteps: number;
  rotatingDirection: number;
  rotatingStopBehaviour: number;
  /** Decoded lit-state image (null if no sprite stored). Async-populated. */
  litSprite: ImageBitmap | null;
}

/** Score-reel rect from cache footer. Matches B2SScore in b2s_parser.h. */
export interface B2SCacheScore {
  id: number;
  x: number;
  y: number;
  width: number;
  height: number;
  digits: number;
  spacing: number;
  playerNo: number;
  startDigit: number;
  litR: number; litG: number; litB: number;
  darkR: number; darkG: number; darkB: number;
  reelType: string;
}

/** Authored animation from .directb2s `<Animations>` block. Plays on Pi
 *  per `start_at_startup` or via b2sStartAnimation event triggers.
 *  PP Doctor doesn't currently animate these — flagged in the memory note. */
export interface B2SCacheAnimation {
  name: string;
  intervalMs: number;
  loops: number;
  lightsAtStart: number;
  lightsAtEnd: number;
  stopBehaviour: number;
  startAtStartup: boolean;
  lockInvolved: boolean;
  steps: B2SCacheAnimationStep[];
}
export interface B2SCacheAnimationStep {
  waitAfterOn: number;
  waitAfterOff: number;
  onNames: string[];
  offNames: string[];
}

export interface B2SCacheDoc {
  /** "source" dims = the original .directb2s display area (typically 1920x1080
   *  for modern tables, 2000-3000 wide for VPU full-fidelity art). */
  sourceWidth: number;
  sourceHeight: number;
  /** Pixels at the bottom of source-space that hold the DMD/grill — hidden
   *  when rendering (useH = sourceHeight - grillHeight, extended by score
   *  bottoms). renderer.cpp:5168-5182. */
  grillHeight: number;
  /** Cache-validation fields. Pi compares (cachedSrcSize, cachedSrcMtime)
   *  against stat(.directb2s) — if mismatched, regenerate. PP Doctor doesn't
   *  use these for validation but they round-trip for diagnostic logs. */
  cachedSrcSize: number;
  cachedSrcMtime: number;
  tableName: string;
  /** Decoded base backglass — Pi cache writer always sizes baseBGRA to
   *  exactly 1920×1080 (renderer.cpp:5184). PP Doctor draws it as-is. */
  baseImage: ImageBitmap;
  bulbs: B2SCacheBulb[];
  scores: B2SCacheScore[];
  animations: B2SCacheAnimation[];
}

// ─── Binary reader ───────────────────────────────────────────────────────────

class Reader {
  view: DataView;
  bytes: Uint8Array;
  pos = 0;

  constructor(buf: ArrayBuffer) {
    this.view = new DataView(buf);
    this.bytes = new Uint8Array(buf);
  }

  /** Remaining bytes — used to bounds-check before each read so we fail
   *  with a clear error instead of overrunning the buffer. */
  remaining(): number { return this.bytes.length - this.pos; }

  magic(): string {
    if (this.remaining() < 4) throw new Error("b2scache: truncated at magic");
    const s = String.fromCharCode(
      this.bytes[this.pos], this.bytes[this.pos + 1],
      this.bytes[this.pos + 2], this.bytes[this.pos + 3]
    );
    this.pos += 4;
    return s;
  }
  u32(): number {
    if (this.remaining() < 4) throw new Error("b2scache: truncated at u32");
    const v = this.view.getUint32(this.pos, true); this.pos += 4; return v;
  }
  i32(): number {
    if (this.remaining() < 4) throw new Error("b2scache: truncated at i32");
    const v = this.view.getInt32(this.pos, true);  this.pos += 4; return v;
  }
  u8(): number {
    if (this.remaining() < 1) throw new Error("b2scache: truncated at u8");
    const v = this.view.getUint8(this.pos);        this.pos += 1; return v;
  }
  u64(): number {
    if (this.remaining() < 8) throw new Error("b2scache: truncated at u64");
    const lo = this.view.getUint32(this.pos, true);
    const hi = this.view.getUint32(this.pos + 4, true);
    this.pos += 8;
    return lo + hi * 0x100000000;   // safe for our small mtimes/sizes
  }
  str(): string {
    const len = this.u32();
    if (this.remaining() < len) throw new Error("b2scache: truncated at str");
    const s = new TextDecoder("utf-8").decode(this.bytes.subarray(this.pos, this.pos + len));
    this.pos += len;
    return s;
  }
  /** Read a raw LZ4-block (Pi's `w_compressed` writes block format, not frame).
   *  Pi format: `u32 orig_size | u32 comp_size | <comp_size bytes>` —
   *  raw LZ4 block, NO 0x184D2204 frame magic. We must use lz4js's
   *  `decompressBlock` not `decompress` (which validates frame magic and
   *  throws "invalid magic number"). Bug reproduced 2026-05-25. */
  block(): Uint8Array {
    const orig = this.u32();
    const comp = this.u32();
    if (this.remaining() < comp) throw new Error("b2scache: truncated at block");
    if (orig === comp) {
      // Uncompressed fallback (Pi writes this when compression overhead > savings).
      const out = this.bytes.slice(this.pos, this.pos + comp);
      this.pos += comp;
      return out;
    }
    const dst = new Uint8Array(orig);
    const written: number = lz4.decompressBlock(this.bytes, dst, this.pos, comp, 0);
    this.pos += comp;
    if (written !== orig) {
      throw new Error(`LZ4 block decompress: got ${written} bytes, want ${orig}`);
    }
    return dst;
  }
}

// ─── BGRA → ImageBitmap ──────────────────────────────────────────────────────

/** SDL writes BGRA32 (in-memory byte order: B, G, R, A). Canvas ImageData is
 *  RGBA byte order. Swap channels via Uint32 bit-manipulation (~5x faster
 *  than per-byte loop — measured 2026-05-25, 1920×1080 base went from
 *  ~250ms to ~50ms). Per-byte fallback for the rare non-aligned cache.
 *
 *  Little-endian Uint32 layout:
 *    BGRA in memory →  byte[0]=B, byte[1]=G, byte[2]=R, byte[3]=A
 *                  →  u32 = (A<<24)|(R<<16)|(G<<8)|B
 *    RGBA in memory →  byte[0]=R, byte[1]=G, byte[2]=B, byte[3]=A
 *                  →  u32 = (A<<24)|(B<<16)|(G<<8)|R
 *  Swap = move bits[0..7] (B) ↔ bits[16..23] (R), keep G + A. */
async function bgraToImageBitmap(bgra: Uint8Array, w: number, h: number, pitch: number): Promise<ImageBitmap> {
  const rowBytes = w * 4;
  const data = new Uint8ClampedArray(rowBytes * h);
  if (pitch === rowBytes && (bgra.byteOffset & 3) === 0) {
    // Fast path: tightly packed, 4-byte aligned. Process as Uint32.
    const u32In  = new Uint32Array(bgra.buffer, bgra.byteOffset, (w * h));
    const u32Out = new Uint32Array(data.buffer);
    for (let i = 0; i < u32In.length; i++) {
      const v = u32In[i];
      u32Out[i] = (v & 0xff00ff00) | ((v & 0xff) << 16) | ((v >>> 16) & 0xff);
    }
  } else {
    // Slow path: pitch padding OR misaligned buffer. Per-byte row walk.
    for (let y = 0; y < h; y++) {
      const srcOff = y * pitch;
      const dstOff = y * rowBytes;
      for (let x = 0; x < w; x++) {
        const s = srcOff + x * 4;
        const d = dstOff + x * 4;
        data[d]     = bgra[s + 2]; // R
        data[d + 1] = bgra[s + 1]; // G
        data[d + 2] = bgra[s];     // B
        data[d + 3] = bgra[s + 3]; // A
      }
    }
  }
  const img = new ImageData(data, w, h);
  return await createImageBitmap(img);
}

// ─── Public parser ───────────────────────────────────────────────────────────

/** Parsed-doc cache keyed by ArrayBuffer reference. Skips the ~500ms re-parse
 *  cost when the same buffer is passed in twice (LRU revisits in +page.svelte).
 *  WeakMap auto-evicts when the buffer is GC'd (matches the upstream LRU's
 *  ~12-entry capacity). Memory footprint per entry: ~1 ImageBitmap (base) +
 *  N×ImageBitmap (per bulb) ≈ a few MB — acceptable. */
const parsedDocCache = new WeakMap<ArrayBuffer, B2SCacheDoc>();

/** Parse a .b2scache buffer into a renderable B2SCacheDoc.
 *
 *  Direct port of `b2s_cache.cpp::tryLoadB2SCacheBase + readB2SCacheBulbs`.
 *  All counts and sizes are bounds-checked the same way Pi does (cap 256
 *  bulbs, 32 scores, 256 animations, 64 names per step, sprite w/h ≤ 4096).
 *  Throws on corrupt input — caller can fall back to .directb2s. */
export async function parseB2SCache(buffer: ArrayBuffer, onBase?: B2SCacheEarlyPaint): Promise<B2SCacheDoc> {
  // Reuse the parsed doc when caller hands us the same buffer (LRU revisits).
  // Saves ~300-500ms per arrow-key revisit for a 15MB cache file. When the
  // cached doc is reused, still fire the early-paint callback so B2SCanvas
  // can apply the same fast-first-paint path on re-render.
  const cached = parsedDocCache.get(buffer);
  if (cached) {
    if (onBase) {
      try { onBase({ baseImage: cached.baseImage, sourceWidth: cached.sourceWidth, sourceHeight: cached.sourceHeight, grillHeight: cached.grillHeight }); }
      catch {}
    }
    return cached;
  }
  const doc = await parseB2SCacheImpl(buffer, onBase);
  parsedDocCache.set(buffer, doc);
  return doc;
}

/** Optional callback fired the moment the base bitmap finishes decoding —
 *  long before the bulb sprite decompresses + ImageBitmap creates run. The
 *  caller (B2SCanvas) uses this to start painting the backglass immediately
 *  so the user sees something within ~100ms instead of waiting 1-10s for
 *  every bulb to finish decoding. */
export type B2SCacheEarlyPaint = (info: {
  baseImage: ImageBitmap;
  sourceWidth: number;
  sourceHeight: number;
  grillHeight: number;
}) => void;

async function parseB2SCacheImpl(buffer: ArrayBuffer, onBase?: B2SCacheEarlyPaint): Promise<B2SCacheDoc> {
  const perfMarks: { label: string; ms: number }[] = [];
  let perfPrev = performance.now();
  const perfT0 = perfPrev;
  const perfMark = (label: string) => {
    const now = performance.now();
    perfMarks.push({ label, ms: now - perfPrev });
    perfPrev = now;
  };
  const r = new Reader(buffer);

  perfMark("entry");
  // ── Header ──
  const m = r.magic();
  if (m !== "B2SC") throw new Error(`Bad magic: '${m}'`);
  const version = r.u32();
  if (version !== 11) throw new Error(`Unsupported .b2scache version ${version}`);
  const cachedSrcSize  = r.u64();   // v11: stat-based validation src.size
  const cachedSrcMtime = r.u64();   // v11: stat-based validation src.mtime
  const sourceWidth  = r.i32();
  const sourceHeight = r.i32();
  const grillHeight  = r.i32();
  const tableName    = r.str();

  // ── Base image ──
  const baseW = r.i32();
  const baseH = r.i32();
  const basePitch = r.i32();
  if (baseW <= 0 || baseH <= 0 || basePitch < baseW * 4) {
    throw new Error(`Bad base dims: ${baseW}x${baseH} pitch=${basePitch}`);
  }
  const baseBGRA = r.block();
  perfMark("LZ4 decompress base");
  const baseImage = await bgraToImageBitmap(baseBGRA, baseW, baseH, basePitch);
  perfMark("bgraToImageBitmap base");
  // EARLY PAINT: hand the base bitmap + source dims to the caller right
  // now so B2SCanvas can start rendering the backglass while we keep
  // chewing through the bulb sprite LZ4 decompresses below (which can
  // take 1-10 seconds total for tables with many large bulbs on a heap
  // under GC pressure). Reduces perceived "Loading preview" time from
  // multi-seconds to ~100ms.
  if (onBase) {
    try { onBase({ baseImage, sourceWidth, sourceHeight, grillHeight }); }
    catch (e) { /* don't let early-paint errors poison the parse */ }
  }

  // Sprite-dim conversion factors. Writer pre-scales sprite_w/h to baseBGRA
  // pixel space (b2s_cache.cpp:306-307 + 388-389) with sx=baseW/source_width,
  // sy=baseH/useH. We render in source coords, so invert back — otherwise
  // sprites scale to baseW/sourceWidth too small (Scared Stiff demo case).
  // CRITICAL: useH must include score-reel extension (b2s_cache.cpp:302-305).
  // PBA Dr Dude (and other Bally late-70s tables) has score reels authored
  // below the painted backglass — writer's useH extends past sourceHeight -
  // grillHeight, so sy_writer = baseH / useH_with_scores. Computing the
  // back-scale with useH_no_scores produced sprites stretched ~10% vertically
  // wrong vs the base bitmap (user-visible misalignment 2026-05-27). Defer
  // the y-back-scale to after the scores section is read so we know the
  // final useH the writer actually used.
  const spriteScaleBackX = sourceWidth / baseW;

  // ── Bulbs ──
  const numBulbs = r.u32();
  if (numBulbs > 256) throw new Error(`bulb count ${numBulbs} > 256 cap`);
  type RawBulb = Omit<B2SCacheBulb, "litSprite"> & {
    _spriteBgra?: Uint8Array; _spriteW?: number; _spriteH?: number; _spritePitch?: number;
  };
  const rawBulbs: RawBulb[] = [];
  for (let i = 0; i < numBulbs; i++) {
    const id        = r.i32();
    const romId     = r.i32();
    const romIdType = r.i32();
    const x = r.i32(), y = r.i32(), w = r.i32(), h = r.i32();
    const initialState         = r.i32();
    const snippitType          = r.i32();
    const rotatingSteps        = r.i32();
    const rotatingDirection    = r.i32();
    const rotatingStopBehaviour = r.i32();
    const name                 = r.str();
    const spriteOffX = r.i32();
    const spriteOffY = r.i32();
    const sw = r.i32();
    const sh = r.i32();
    const sp = r.i32();

    let _spriteBgra: Uint8Array | undefined;
    if (sw > 0 && sh > 0) {
      // Same defensive bounds as Pi (b2s_cache.cpp:570).
      if (sp < sw * 4 || sw > 4096 || sh > 4096) {
        throw new Error(`bulb ${id}: bad sprite dims w=${sw} h=${sh} pitch=${sp}`);
      }
      _spriteBgra = r.block();
      // Yield to UI every 4 bulbs so the early-painted backglass + any
      // user interactions (scrolling, button hover) stay responsive while
      // the rest of the sprite LZ4 decompresses chew through. lz4js is
      // synchronous so without this yield the entire bulb loop blocks
      // the main thread for the duration of all LZ4 calls combined.
      if (i % 4 === 3) await new Promise<void>(r => setTimeout(r, 0));
    }

    const spriteWidthSrc  = Math.round(sw * spriteScaleBackX);
    // spriteHeightSrc back-scale deferred — needs final useH (with scores)
    rawBulbs.push({
      id, romId, romIdType, name,
      x, y, width: w, height: h,
      initialState,
      spriteOffX, spriteOffY,
      spriteWidth: spriteWidthSrc, spriteHeight: 0,   // patched in below after scores read
      snippitType, isSnippit: snippitType !== 0,
      rotatingSteps, rotatingDirection, rotatingStopBehaviour,
      _spriteBgra, _spriteW: sw, _spriteH: sh, _spritePitch: sp,
    });
  }

  // ── Scores ──
  const numScores = r.u32();
  if (numScores > 32) throw new Error(`score count ${numScores} > 32 cap`);
  const scores: B2SCacheScore[] = [];
  for (let i = 0; i < numScores; i++) {
    const id = r.i32(), x = r.i32(), y = r.i32(), w = r.i32(), h = r.i32();
    const digits = r.i32(), spacing = r.i32();
    const playerNo = r.i32(), startDigit = r.i32();
    const litR = r.u8(), litG = r.u8(), litB = r.u8();
    const darkR = r.u8(), darkG = r.u8(), darkB = r.u8();
    const reelType = r.str();
    scores.push({
      id, x, y, width: w, height: h, digits, spacing, playerNo, startDigit,
      litR, litG, litB, darkR, darkG, darkB, reelType,
    });
  }

  // ── Final useH for Y back-scale (mirrors b2s_cache.cpp:302-305) ──
  // Score reels can extend useH past sourceHeight - grillHeight when
  // authored below the visible backglass. Match the writer exactly so
  // sprite vertical scaling round-trips cleanly.
  let useHWithScores = Math.max(1, sourceHeight - grillHeight);
  for (const sc of scores) {
    const bottom = sc.y + sc.height;
    if (bottom > useHWithScores) useHWithScores = bottom;
  }
  const spriteScaleBackY = useHWithScores / baseH;
  for (const rb of rawBulbs) {
    rb.spriteHeight = Math.round((rb._spriteH ?? 0) * spriteScaleBackY);
  }

  // ── Animations ──
  const numAnims = r.u32();
  if (numAnims > 256) throw new Error(`animation count ${numAnims} > 256 cap`);
  const animations: B2SCacheAnimation[] = [];
  for (let i = 0; i < numAnims; i++) {
    const name = r.str();
    const intervalMs    = r.i32();
    const loops         = r.i32();
    const lightsAtStart = r.i32();
    const lightsAtEnd   = r.i32();
    const stopBehaviour = r.i32();
    const startAtStartup = r.u8() !== 0;
    const lockInvolved   = r.u8() !== 0;
    const numSteps = r.u32();
    if (numSteps > 4096) throw new Error(`anim ${name}: step count ${numSteps} > 4096 cap`);
    const steps: B2SCacheAnimationStep[] = [];
    for (let j = 0; j < numSteps; j++) {
      const waitAfterOn  = r.i32();
      const waitAfterOff = r.i32();
      const onCount = r.u32();
      if (onCount > 64) throw new Error(`anim ${name} step ${j}: on count ${onCount} > 64`);
      const onNames: string[] = [];
      for (let k = 0; k < onCount; k++) onNames.push(r.str());
      const offCount = r.u32();
      if (offCount > 64) throw new Error(`anim ${name} step ${j}: off count ${offCount} > 64`);
      const offNames: string[] = [];
      for (let k = 0; k < offCount; k++) offNames.push(r.str());
      steps.push({ waitAfterOn, waitAfterOff, onNames, offNames });
    }
    animations.push({
      name, intervalMs, loops, lightsAtStart, lightsAtEnd, stopBehaviour,
      startAtStartup, lockInvolved, steps,
    });
  }

  perfMark(`read ${rawBulbs.length} bulb metadata + LZ4 sprites`);
  // Decode all bulb sprites in parallel (one createImageBitmap each).
  const bulbs: B2SCacheBulb[] = await Promise.all(rawBulbs.map(async rb => {
    let litSprite: ImageBitmap | null = null;
    if (rb._spriteBgra && rb._spriteW && rb._spriteH && rb._spritePitch) {
      try {
        litSprite = await bgraToImageBitmap(rb._spriteBgra, rb._spriteW, rb._spriteH, rb._spritePitch);
      } catch { /* skip if decode fails */ }
    }
    const { _spriteBgra: _a, _spriteW: _b, _spriteH: _c, _spritePitch: _d, ...clean } = rb;
    return { ...clean, litSprite };
  }));
  perfMark(`decode ${bulbs.length} sprite ImageBitmaps (parallel)`);

  // Emit perf summary so we can see exactly where parse time goes.
  const total = Math.round(performance.now() - perfT0);
  const breakdown = perfMarks.map(m => `${m.label}=${Math.round(m.ms)}ms`).join(", ");
  // Fire-and-forget log (don't await — the perf logger is async).
  import("$lib/api").then(({ log }) => log("[b2scache/perf]",
    `total=${total}ms  ${breakdown}`)).catch(() => {});

  return {
    sourceWidth, sourceHeight, grillHeight,
    cachedSrcSize, cachedSrcMtime, tableName,
    baseImage, bulbs, scores, animations,
  };
}
