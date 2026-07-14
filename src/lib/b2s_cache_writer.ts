// Generate a v11 `.b2scache` from a parsed `.directb2s`, entirely on the PC.
//
// Renders the base backglass (stretched to 1920×1080) and each bulb's lit sprite
// to BGRA in the webview, then hands the raw buffers to the Rust serializer
// (`generate_b2scache`) which writes the exact on-disk format the Pi reads. This
// offloads the slow Pi-Zero conversion. See docs/b2scache_writer_spec.md.
//
// Cache-only cabinet: the caller pushes the generated cache, NOT the .directb2s.

import { invoke } from "@tauri-apps/api/core";
import type { B2SDoc } from "./b2s";
import { cacheWriteBinary, dbMarkDirty } from "./api";

const BASE_W = 1920;
const BASE_H = 1080;
const BASE_PITCH = BASE_W * 4;

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function decodeBitmap(b64: string): Promise<ImageBitmap> {
  const clean = b64.replace(/^data:image\/[^;]+;base64,/, "");
  const blob = new Blob([b64ToBytes(clean)], { type: "image/png" });
  return await createImageBitmap(blob);
}

/** Swap R↔B in place (canvas getImageData is RGBA; the cache stores BGRA). Keeps
 *  G and A. Matches the reader's inverse mask (b2s_cache.ts). */
function rgbaToBgra(data: Uint8ClampedArray): Uint8Array {
  const u32 = new Uint32Array(data.buffer, data.byteOffset, data.length >>> 2);
  for (let i = 0; i < u32.length; i++) {
    const v = u32[i];
    u32[i] = (v & 0xff00ff00) | ((v & 0xff) << 16) | ((v >>> 16) & 0xff);
  }
  return new Uint8Array(data.buffer, data.byteOffset, data.length);
}

/** floor(x + 0.5) — matches the Pi's (int)(x + 0.5f) rounding. */
const round = (x: number) => Math.floor(x + 0.5);

function ctx2d(c: OffscreenCanvas): OffscreenCanvasRenderingContext2D {
  const g = c.getContext("2d", { willReadFrequently: true });
  if (!g) throw new Error("2d context unavailable");
  return g;
}

/**
 * Build `backglass.b2scache` for `doc` and write it to the local mirror (marked
 * dirty so the caller's syncPushDirty ships it). `srcBytes` is the raw .directb2s
 * (only its length is recorded — mtime is unused in the cache-only workflow).
 * Returns the cache size in bytes.
 */
export async function generateAndWriteB2sCache(
  doc: B2SDoc,
  srcBytes: Uint8Array,
  tableId: number,
  host: string,
  folder: string,
  cacheDir: string | null,
): Promise<number> {
  // ── 1. Base: native source dims → visible top region stretched into 1080p ──
  const baseBmp = await decodeBitmap(doc.baseDataUrl);
  const sourceW = baseBmp.width;
  const sourceH = baseBmp.height;
  let useH = sourceH - doc.grillHeight;
  if (useH < 1) useH = sourceH;
  for (const sc of doc.scores) useH = Math.max(useH, sc.y + sc.height); // score-reel extension
  const sx = BASE_W / sourceW;
  const sy = BASE_H / useH;

  const baseCanvas = new OffscreenCanvas(BASE_W, BASE_H);
  const bctx = ctx2d(baseCanvas);
  bctx.imageSmoothingEnabled = true; // linear, closest to the Pi's SoftStretchLinear
  bctx.imageSmoothingQuality = "high";
  bctx.drawImage(baseBmp, 0, 0, sourceW, useH, 0, 0, BASE_W, BASE_H);
  const baseBgra = rgbaToBgra(bctx.getImageData(0, 0, BASE_W, BASE_H).data);

  // ── 2. Bulb sprites ──
  const spriteBlobs: Uint8Array[] = [];
  const bulbs = [];
  for (const b of doc.bulbs) {
    let sprite_off_x = 0, sprite_off_y = 0, sprite_w = 0, sprite_h = 0, sprite_pitch = 0, sprite_len = 0;
    if (b.litB64) {
      const bmp = await decodeBitmap(b.litB64);
      const rawW = bmp.width;
      const rawH = bmp.height;
      const rc = new OffscreenCanvas(rawW, rawH);
      const rctx = ctx2d(rc);
      rctx.drawImage(bmp, 0, 0);
      const raw = rctx.getImageData(0, 0, rawW, rawH).data;

      // bbox crop where alpha > 8; rotators (SnippitType==2) keep the full frame
      // so the rotation pivot doesn't shift.
      let cx = 0, cy = 0, cw = rawW, ch = rawH;
      if (b.snippitType !== 2) {
        let minx = rawW, miny = rawH, maxx = -1, maxy = -1;
        for (let y = 0; y < rawH; y++) {
          const row = y * rawW;
          for (let x = 0; x < rawW; x++) {
            if (raw[(row + x) * 4 + 3] > 8) {
              if (x < minx) minx = x;
              if (x > maxx) maxx = x;
              if (y < miny) miny = y;
              if (y > maxy) maxy = y;
            }
          }
        }
        if (maxx >= minx && maxy >= miny) {
          cx = minx; cy = miny; cw = maxx - minx + 1; ch = maxy - miny + 1;
        } // fully transparent → keep full frame
      }

      const rx = b.width / rawW;
      const ry = b.height / rawH;
      sprite_off_x = round(cx * rx);
      sprite_off_y = round(cy * ry);
      const srcW = round(cw * rx);
      const srcH = round(ch * ry);
      const dw = round(srcW * sx);
      const dh = round(srcH * sy);
      if (dw > 0 && dh > 0) {
        const sc = new OffscreenCanvas(dw, dh);
        const sctx = ctx2d(sc);
        sctx.imageSmoothingEnabled = false; // nearest-neighbor, matches SDL_BlitScaled
        sctx.drawImage(bmp, cx, cy, cw, ch, 0, 0, dw, dh);
        spriteBlobs.push(rgbaToBgra(sctx.getImageData(0, 0, dw, dh).data));
        sprite_w = dw; sprite_h = dh; sprite_pitch = dw * 4; sprite_len = dw * 4 * dh;
      }
      bmp.close();
    }
    bulbs.push({
      id: b.id, rom_id: b.romId, rom_id_type: b.romIdType,
      x: b.x, y: b.y, w: b.width, h: b.height,
      initial_state: b.initialState,
      snippit_type: b.snippitType,
      rotating_steps: b.rotatingSteps,
      rotating_direction: b.rotatingDirection,
      rotating_stop_behaviour: b.rotatingStopBehaviour,
      name: b.name,
      sprite_off_x, sprite_off_y, sprite_w, sprite_h, sprite_pitch, sprite_len,
    });
  }
  baseBmp.close();

  // concat sprite BGRA in bulb order
  let total = 0;
  for (const s of spriteBlobs) total += s.length;
  const sprites = new Uint8Array(total);
  let o = 0;
  for (const s of spriteBlobs) { sprites.set(s, o); o += s.length; }

  // Score reels (used by PBA tables) — real colors + reel type from the
  // .directb2s, so the cabinet renders the right digit colors/style.
  const scores = doc.scores.map((s) => ({
    id: s.id, x: s.x, y: s.y, w: s.width, h: s.height,
    digits: s.digits, spacing: s.spacing, player_no: s.playerNo, start_digit: s.startDigit,
    lit_r: s.litR, lit_g: s.litG, lit_b: s.litB,
    dark_r: s.darkR, dark_g: s.darkG, dark_b: s.darkB,
    reel_type: s.reelType,
  }));

  const animations = doc.animations.map((a) => ({
    name: a.name, interval_ms: a.intervalMs, loops: a.loops,
    lights_at_start: a.lightsAtStart, lights_at_end: a.lightsAtEnd, stop_behaviour: a.stopBehaviour,
    start_at_startup: a.startAtStartup, lock_involved: a.lockInvolved,
    steps: a.steps.map((st) => ({
      wait_after_on: st.waitAfterOn, wait_after_off: st.waitAfterOff,
      on: st.onNames, off: st.offNames,
    })),
  }));

  const header = {
    src_size: srcBytes.length,
    src_mtime: 0, // cache-only cabinet: Pi has no source to stat against
    source_width: sourceW, source_height: sourceH, grill_height: doc.grillHeight,
    table_name: doc.tableName,
    base_w: BASE_W, base_h: BASE_H, base_pitch: BASE_PITCH,
    bulbs, scores, animations,
  };

  const cacheBuf = await invoke<ArrayBuffer>("generate_b2scache", { header, baseBgra, sprites });
  const bytes = new Uint8Array(cacheBuf);
  await cacheWriteBinary(host, folder, "default_image", "backglass.b2scache", bytes, cacheDir);
  await dbMarkDirty(tableId, "default_image", "backglass.b2scache");
  return bytes.length;
}
