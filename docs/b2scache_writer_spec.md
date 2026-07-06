# PP Doctor `.b2scache` v11 writer — implementation reference

Authoritative sources: PinnerPi `b2s_cache.cpp` (`writeB2SCache`, `pre_decode_bulb_sprite`,
readers), PP Doctor `src/lib/b2s_cache.ts` (byte-exact v11 reader — the inverse of this writer),
`src/lib/b2s.ts` (parser), `src/lib/B2SCanvas.svelte` (base/bulb render).

Workflow decision (2026-07-05): **cache-only cabinet**. PP Doctor generates `backglass.b2scache`
(fixed name, slot `default_image`) and pushes it; it does **NOT** push the source `.directb2s` to
the cabinet (kept in the local mirror / gitea). With no `.directb2s` present the Pi trusts any valid
cache (magic+version), sidestepping the FAT32 / no-`scp -p` mtime-mismatch that would otherwise make
the Pi regenerate on the Pi Zero. Pi still generates the hidden `backglass.b2s_base.thumb.jpg`.

All integers little-endian. `str` = `u32 len` + `len` UTF-8 bytes (empty = `u32 0`).

## Byte layout (exact write order)
Header: `"B2SC"` | u32 version=11 | u64 src_size | u64 src_mtime | i32 source_width | i32 source_height
| i32 grill_height | str table_name | i32 base_w(=1920) | i32 base_h(=1080) | i32 base_pitch(=base_w*4=7680)
| LZ4-block base BGRA (uncompressed size = base_pitch*base_h) | u32 bulb_count.

Per bulb: i32 id, rom_id, rom_id_type, x, y, w, h, initial_state, snippit_type, rotating_steps,
rotating_direction, rotating_stop_behaviour; str name; i32 sprite_off_x, sprite_off_y;
i32 sprite_w, sprite_h, sprite_pitch; **if sprite_w>0 && sprite_h>0**: LZ4-block sprite BGRA
(size = sprite_pitch*sprite_h). Else write dims as 0,0,0 and **no block**.

Scores: u32 score_count; per score i32 id,x,y,w,h,digits,spacing,player_no,start_digit;
u8 lit_r,lit_g,lit_b,dark_r,dark_g,dark_b; str reel_type.

Animations: u32 anim_count; per anim str name; i32 interval_ms,loops,lights_at_start,lights_at_end,
stop_behaviour; u8 start_at_startup, lock_involved; u32 step_count; per step i32 wait_after_on,
wait_after_off; u32 on_count + on_count×str; u32 off_count + off_count×str. File ends after last anim.

## LZ4 block: `u32 orig_size | u32 comp_size | comp_size bytes`
- orig_size MUST equal expected pitch*h (reader rejects otherwise).
- orig_size == comp_size ⇒ stored uncompressed (raw bytes follow). Always valid.
- else raw LZ4 block (LZ4_compress_default-compatible = Rust `lz4_flex::block::compress`).
- orig_size == 0 ⇒ empty (no bytes) — never emitted for real base/sprites.

## Bulb sprite scaling (mirror pre_decode_bulb_sprite)
Global (once): useH = source_height - grill_height (min 1); for each score useH = max(useH, sc.y+sc.h);
sx = base_w/source_width; sy = base_h/useH.
Per bulb: decode lit PNG (raw_w×raw_h). Bbox crop where alpha>8 (SKIP for snippit_type==2 rotators →
full frame). raw→src map: rx=b.w/raw_w, ry=b.h/raw_h; sprite_off_x=round(crop_x*rx),
sprite_off_y=round(crop_y*ry), src_w=round(crop_w*rx), src_h=round(crop_h*ry). Scale to base:
dw=round(src_w*sx), dh=round(src_h*sy) (round = floor(x+0.5)); if dw<=0||dh<=0 → no sprite.
Store sprite_w=dw, sprite_h=dh, sprite_pitch=dw*4. Nearest-neighbor scale for cabinet parity (Pi
validates dims only, not pixels).

## Channel order: cache is BGRA; canvas getImageData is RGBA → swap byte[0]<->byte[2] (R<->B, keep G,A).

## Validation caps (reader rejects): version==11; base_w/h>0, base_pitch>=base_w*4; bulbs<=256;
sprite_pitch>=sprite_w*4, sprite_w/h<=4096; scores<=32; anims<=256; steps<=4096; on/off<=64;
str len < 1<<20; every block orig_size == pitch*h.

## Dims gotcha: source_width/height are the NATIVE decoded base image dims
(B2SCanvas naturalWidth/Height), NOT parseDirectB2S's hardcoded 1920×1080.

## Hash sidecar: DEAD in v11 (writeB2SCache never writes it; readers never read it). Do not emit.

## Integration: ingestDirectb2s (+page.svelte ~line 1015-1075). After the .directb2s is written to
the local mirror (~1032), parse it, render base+bulb BGRA in JS, invoke the Rust serializer →
write backglass.b2scache to the mirror + dbMarkDirty(default_image, backglass.b2scache). Do NOT
dbMarkDirty the .directb2s (cache-only: keep it local, don't push). syncPushDirty ships the cache.
