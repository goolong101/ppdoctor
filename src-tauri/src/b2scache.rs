// v11 `.b2scache` writer — mirrors PinnerPi `b2s_cache.cpp::writeB2SCache`.
// See docs/b2scache_writer_spec.md for the authoritative byte layout.
//
// Cache-only workflow: PP Doctor renders the base + bulb sprites to BGRA in the
// webview (reusing the b2s.ts / B2SCanvas pipeline), hands the raw buffers here,
// and this serializes them into the exact on-disk format the Pi renderer reads —
// offloading the slow Pi-Zero conversion to the PC.
//
// All integers little-endian. `str` = u32 len + len UTF-8 bytes.

use serde::Deserialize;

#[derive(Deserialize)]
pub struct BulbMeta {
    pub id: i32,
    pub rom_id: i32,
    pub rom_id_type: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub initial_state: i32,
    pub snippit_type: i32,
    pub rotating_steps: i32,
    pub rotating_direction: i32,
    pub rotating_stop_behaviour: i32,
    pub name: String,
    pub sprite_off_x: i32,
    pub sprite_off_y: i32,
    pub sprite_w: i32,
    pub sprite_h: i32,
    pub sprite_pitch: i32,
    /// BGRA byte count for this bulb's sprite in the concatenated `sprites`
    /// blob (== sprite_pitch*sprite_h). 0 = no sprite (dims written as 0,0,0).
    pub sprite_len: u32,
}

#[derive(Deserialize)]
pub struct ScoreMeta {
    pub id: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub digits: i32,
    pub spacing: i32,
    pub player_no: i32,
    pub start_digit: i32,
    pub lit_r: u8,
    pub lit_g: u8,
    pub lit_b: u8,
    pub dark_r: u8,
    pub dark_g: u8,
    pub dark_b: u8,
    pub reel_type: String,
}

#[derive(Deserialize)]
pub struct StepMeta {
    pub wait_after_on: i32,
    pub wait_after_off: i32,
    pub on: Vec<String>,
    pub off: Vec<String>,
}

#[derive(Deserialize)]
pub struct AnimMeta {
    pub name: String,
    pub interval_ms: i32,
    pub loops: i32,
    pub lights_at_start: i32,
    pub lights_at_end: i32,
    pub stop_behaviour: i32,
    pub start_at_startup: bool,
    pub lock_involved: bool,
    pub steps: Vec<StepMeta>,
}

#[derive(Deserialize)]
pub struct CacheHeader {
    pub src_size: u64,
    pub src_mtime: u64,
    pub source_width: i32,
    pub source_height: i32,
    pub grill_height: i32,
    pub table_name: String,
    pub base_w: i32,
    pub base_h: i32,
    pub base_pitch: i32,
    pub bulbs: Vec<BulbMeta>,
    pub scores: Vec<ScoreMeta>,
    pub animations: Vec<AnimMeta>,
}

#[inline]
fn w_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
#[inline]
fn w_i32(b: &mut Vec<u8>, v: i32) {
    b.extend_from_slice(&v.to_le_bytes());
}
#[inline]
fn w_u64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}
#[inline]
fn w_u8(b: &mut Vec<u8>, v: u8) {
    b.push(v);
}
fn w_str(b: &mut Vec<u8>, s: &str) {
    let by = s.as_bytes();
    w_u32(b, by.len() as u32);
    b.extend_from_slice(by);
}

/// LZ4 block: `u32 orig_size | u32 comp_size | comp_size bytes`. Compress with a
/// raw LZ4 block (LZ4_compress_default-compatible, decodable by the Pi's
/// LZ4_decompress_safe and the lz4js reader); fall back to stored-uncompressed
/// (orig_size == comp_size) if compression doesn't shrink it. The reader requires
/// orig_size to equal the expected pitch*h, which it does here.
fn w_block(b: &mut Vec<u8>, raw: &[u8]) {
    let orig = raw.len() as u32;
    let comp = lz4_flex::block::compress(raw);
    if comp.len() < raw.len() {
        w_u32(b, orig);
        w_u32(b, comp.len() as u32);
        b.extend_from_slice(&comp);
    } else {
        // stored uncompressed
        w_u32(b, orig);
        w_u32(b, orig);
        b.extend_from_slice(raw);
    }
}

fn serialize(h: &CacheHeader, base_bgra: &[u8], sprites: &[u8]) -> Result<Vec<u8>, String> {
    // Reader-enforced caps (b2s_cache.ts / b2s_cache.cpp) — reject early so we
    // never write a cache the Pi will discard as corrupt.
    if h.bulbs.len() > 256 {
        return Err(format!("bulbs {} > 256", h.bulbs.len()));
    }
    if h.scores.len() > 32 {
        return Err(format!("scores {} > 32", h.scores.len()));
    }
    if h.animations.len() > 256 {
        return Err(format!("animations {} > 256", h.animations.len()));
    }
    if h.base_w <= 0 || h.base_h <= 0 {
        return Err("base dims <= 0".into());
    }
    if h.base_pitch < h.base_w * 4 {
        return Err(format!("base_pitch {} < base_w*4 {}", h.base_pitch, h.base_w * 4));
    }
    let exp_base = h.base_pitch as usize * h.base_h as usize;
    if base_bgra.len() != exp_base {
        return Err(format!("base_bgra {} != expected {}", base_bgra.len(), exp_base));
    }

    let mut buf: Vec<u8> = Vec::with_capacity(exp_base + sprites.len() + 8192);
    buf.extend_from_slice(b"B2SC");
    w_u32(&mut buf, 11);
    w_u64(&mut buf, h.src_size);
    w_u64(&mut buf, h.src_mtime);
    w_i32(&mut buf, h.source_width);
    w_i32(&mut buf, h.source_height);
    w_i32(&mut buf, h.grill_height);
    w_str(&mut buf, &h.table_name);
    w_i32(&mut buf, h.base_w);
    w_i32(&mut buf, h.base_h);
    w_i32(&mut buf, h.base_pitch);
    w_block(&mut buf, base_bgra);
    w_u32(&mut buf, h.bulbs.len() as u32);

    let mut off: usize = 0;
    for b in &h.bulbs {
        w_i32(&mut buf, b.id);
        w_i32(&mut buf, b.rom_id);
        w_i32(&mut buf, b.rom_id_type);
        w_i32(&mut buf, b.x);
        w_i32(&mut buf, b.y);
        w_i32(&mut buf, b.w);
        w_i32(&mut buf, b.h);
        w_i32(&mut buf, b.initial_state);
        w_i32(&mut buf, b.snippit_type);
        w_i32(&mut buf, b.rotating_steps);
        w_i32(&mut buf, b.rotating_direction);
        w_i32(&mut buf, b.rotating_stop_behaviour);
        w_str(&mut buf, &b.name);
        w_i32(&mut buf, b.sprite_off_x);
        w_i32(&mut buf, b.sprite_off_y);

        let has_sprite = b.sprite_w > 0 && b.sprite_h > 0 && b.sprite_len > 0;
        if has_sprite {
            if b.sprite_w > 4096 || b.sprite_h > 4096 {
                return Err(format!("sprite {}x{} > 4096 (bulb {})", b.sprite_w, b.sprite_h, b.id));
            }
            if b.sprite_pitch < b.sprite_w * 4 {
                return Err(format!("sprite_pitch {} < w*4 (bulb {})", b.sprite_pitch, b.id));
            }
            let exp = b.sprite_pitch as usize * b.sprite_h as usize;
            if b.sprite_len as usize != exp {
                return Err(format!("sprite_len {} != {} (bulb {})", b.sprite_len, exp, b.id));
            }
            let end = off + exp;
            if end > sprites.len() {
                return Err(format!("sprites blob underrun at bulb {} ({} > {})", b.id, end, sprites.len()));
            }
            w_i32(&mut buf, b.sprite_w);
            w_i32(&mut buf, b.sprite_h);
            w_i32(&mut buf, b.sprite_pitch);
            w_block(&mut buf, &sprites[off..end]);
            off = end;
        } else {
            // no sprite → dims 0,0,0 and NO pixel block
            w_i32(&mut buf, 0);
            w_i32(&mut buf, 0);
            w_i32(&mut buf, 0);
        }
    }

    w_u32(&mut buf, h.scores.len() as u32);
    for s in &h.scores {
        w_i32(&mut buf, s.id);
        w_i32(&mut buf, s.x);
        w_i32(&mut buf, s.y);
        w_i32(&mut buf, s.w);
        w_i32(&mut buf, s.h);
        w_i32(&mut buf, s.digits);
        w_i32(&mut buf, s.spacing);
        w_i32(&mut buf, s.player_no);
        w_i32(&mut buf, s.start_digit);
        w_u8(&mut buf, s.lit_r);
        w_u8(&mut buf, s.lit_g);
        w_u8(&mut buf, s.lit_b);
        w_u8(&mut buf, s.dark_r);
        w_u8(&mut buf, s.dark_g);
        w_u8(&mut buf, s.dark_b);
        w_str(&mut buf, &s.reel_type);
    }

    w_u32(&mut buf, h.animations.len() as u32);
    for a in &h.animations {
        if a.steps.len() > 4096 {
            return Err(format!("anim '{}' steps {} > 4096", a.name, a.steps.len()));
        }
        w_str(&mut buf, &a.name);
        w_i32(&mut buf, a.interval_ms);
        w_i32(&mut buf, a.loops);
        w_i32(&mut buf, a.lights_at_start);
        w_i32(&mut buf, a.lights_at_end);
        w_i32(&mut buf, a.stop_behaviour);
        w_u8(&mut buf, if a.start_at_startup { 1 } else { 0 });
        w_u8(&mut buf, if a.lock_involved { 1 } else { 0 });
        w_u32(&mut buf, a.steps.len() as u32);
        for st in &a.steps {
            if st.on.len() > 64 || st.off.len() > 64 {
                return Err(format!("anim '{}' step on/off > 64", a.name));
            }
            w_i32(&mut buf, st.wait_after_on);
            w_i32(&mut buf, st.wait_after_off);
            w_u32(&mut buf, st.on.len() as u32);
            for s in &st.on {
                w_str(&mut buf, s);
            }
            w_u32(&mut buf, st.off.len() as u32);
            for s in &st.off {
                w_str(&mut buf, s);
            }
        }
    }

    Ok(buf)
}

/// Serialize a v11 `.b2scache` from webview-rendered BGRA buffers.
///
/// `base_bgra` and `sprites` are passed as top-level byte args (Tauri's efficient
/// raw-byte path — the same mechanism `cache_write_binary` uses) rather than
/// nested in `header`, to avoid JSON-encoding megabytes. `sprites` is every
/// bulb's BGRA sprite concatenated in bulb order; each bulb's `sprite_len` slices
/// it. Returns the finished cache bytes (JS writes them via cacheWriteBinary).
#[tauri::command]
pub fn generate_b2scache(
    header: CacheHeader,
    base_bgra: Vec<u8>,
    sprites: Vec<u8>,
) -> Result<tauri::ipc::Response, String> {
    let bytes = serialize(&header, &base_bgra, &sprites)?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_header() -> CacheHeader {
        CacheHeader {
            src_size: 12345,
            src_mtime: 0,
            source_width: 3000,
            source_height: 2649,
            grill_height: 574,
            table_name: "Test".into(),
            base_w: 1920,
            base_h: 1080,
            base_pitch: 1920 * 4,
            bulbs: vec![],
            scores: vec![],
            animations: vec![],
        }
    }

    fn read_block(buf: &[u8], p: &mut usize) -> Vec<u8> {
        let orig = u32::from_le_bytes(buf[*p..*p + 4].try_into().unwrap()) as usize;
        *p += 4;
        let comp = u32::from_le_bytes(buf[*p..*p + 4].try_into().unwrap()) as usize;
        *p += 4;
        let bytes = &buf[*p..*p + comp];
        *p += comp;
        if orig == comp {
            bytes.to_vec()
        } else {
            lz4_flex::block::decompress(bytes, orig).unwrap()
        }
    }

    #[test]
    fn header_and_base_round_trip() {
        // A patterned base so LZ4 has to actually reconstruct it (not all-zero).
        let mut base = vec![0u8; 1920 * 1080 * 4];
        for (i, b) in base.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let out = serialize(&empty_header(), &base, &[]).unwrap();

        let mut p = 0usize;
        assert_eq!(&out[0..4], b"B2SC");
        p = 4;
        assert_eq!(u32::from_le_bytes(out[p..p + 4].try_into().unwrap()), 11);
        p += 4;
        assert_eq!(u64::from_le_bytes(out[p..p + 8].try_into().unwrap()), 12345);
        p += 8;
        p += 8; // src_mtime
        assert_eq!(i32::from_le_bytes(out[p..p + 4].try_into().unwrap()), 3000);
        p += 4;
        p += 4 + 4; // source_height + grill_height
        let nlen = u32::from_le_bytes(out[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        assert_eq!(&out[p..p + nlen], b"Test");
        p += nlen;
        assert_eq!(i32::from_le_bytes(out[p..p + 4].try_into().unwrap()), 1920);
        p += 4;
        assert_eq!(i32::from_le_bytes(out[p..p + 4].try_into().unwrap()), 1080);
        p += 4;
        assert_eq!(i32::from_le_bytes(out[p..p + 4].try_into().unwrap()), 1920 * 4);
        p += 4;
        let decoded = read_block(&out, &mut p);
        assert_eq!(decoded, base, "base BGRA must survive the LZ4 round-trip");
        assert_eq!(u32::from_le_bytes(out[p..p + 4].try_into().unwrap()), 0, "bulb_count");
    }

    #[test]
    fn one_bulb_with_sprite() {
        let base = vec![7u8; 1920 * 1080 * 4];
        let (sw, sh) = (10i32, 8i32);
        let sprite = vec![9u8; (sw * 4 * sh) as usize];
        let mut h = empty_header();
        h.bulbs.push(BulbMeta {
            id: 3, rom_id: 0, rom_id_type: 0, x: 100, y: 200, w: 50, h: 40,
            initial_state: 0, snippit_type: 0, rotating_steps: 1,
            rotating_direction: 0, rotating_stop_behaviour: 0, name: "b".into(),
            sprite_off_x: 1, sprite_off_y: 2, sprite_w: sw, sprite_h: sh,
            sprite_pitch: sw * 4, sprite_len: (sw * 4 * sh) as u32,
        });
        let out = serialize(&h, &base, &sprite).unwrap();
        assert_eq!(&out[0..4], b"B2SC");
        // sanity: it produced something larger than the header alone
        assert!(out.len() > 4 + 4);
    }

    #[test]
    fn rejects_wrong_base_size() {
        assert!(serialize(&empty_header(), &[0u8; 10], &[]).is_err());
    }

    #[test]
    fn rejects_too_many_bulbs() {
        let mut h = empty_header();
        for i in 0..257 {
            h.bulbs.push(BulbMeta {
                id: i, rom_id: 0, rom_id_type: 0, x: 0, y: 0, w: 1, h: 1,
                initial_state: 0, snippit_type: 0, rotating_steps: 1,
                rotating_direction: 0, rotating_stop_behaviour: 0, name: "".into(),
                sprite_off_x: 0, sprite_off_y: 0, sprite_w: 0, sprite_h: 0,
                sprite_pitch: 0, sprite_len: 0,
            });
        }
        assert!(serialize(&h, &vec![0u8; 1920 * 1080 * 4], &[]).is_err());
    }
}
