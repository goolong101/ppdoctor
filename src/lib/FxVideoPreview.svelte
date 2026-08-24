<script lang="ts">
  // FX preview over a VIDEO table: runs the same FxSim as B2SCanvas in a
  // transparent canvas laid over the <video>. Authoring space for video
  // tables = the 1920×1080 screen (sx = sy = 1), matching the Pi's
  // loadFxFromImageMap fallback scale.
  import { onMount, onDestroy } from "svelte";
  import { FxSim, type FxDoc } from "./fx";

  let {
    fxDoc = null,
    fxImageResolver = null,
    fxOverlaySelected = -1,
    onFxOverlayMove = null,
    onFxOverlayResize = null,
    fxGlowSelected = -1,
    onFxGlowMove = null,
  }: {
    fxDoc?: FxDoc | null;
    fxImageResolver?: ((path: string) => Promise<string | null>) | null;
    /** Selected overlay: outlined + draggable; bottom-right handle resizes (Shift = keep aspect). */
    fxOverlaySelected?: number;
    onFxOverlayMove?: ((index: number, x: number, y: number) => void) | null;
    onFxOverlayResize?: ((index: number, w: number, h: number) => void) | null;
    fxGlowSelected?: number;
    onFxGlowMove?: ((index: number, x: number, y: number) => void) | null;
  } = $props();

  let canvas = $state<HTMLCanvasElement | undefined>();
  const sim = new FxSim();
  let key = "";
  let raf = 0;

  $effect(() => { sim.imageResolver = fxImageResolver; });

  // ── editing (source space = 1920×1080, so sx = sy = 1)
  let geom = { fit: 1, ox: 0, oy: 0 };
  const HANDLE = 14;
  let ovDrag: { idx: number; gx: number; gy: number; x: number; y: number } | null = null;
  let ovResize: { idx: number; x0: number; y0: number; w: number; h: number; aspect: number } | null = null;
  let glowDrag: { idx: number } | null = null;
  function toLogical(e: PointerEvent): { x: number; y: number } | null {
    if (!canvas) return null;
    const r = canvas.getBoundingClientRect();
    const px = (e.clientX - r.left) * (canvas.width / r.width);
    const py = (e.clientY - r.top) * (canvas.height / r.height);
    return { x: (px - geom.ox) / geom.fit, y: (py - geom.oy) / geom.fit };
  }
  function pDown(e: PointerEvent) {
    const p = toLogical(e); if (!p) return;
    if (fxGlowSelected >= 0) {
      const g = sim.glowCenter(fxGlowSelected);
      if (g && Math.abs(p.x - g.x) <= Math.max(24, g.rx) && Math.abs(p.y - g.y) <= Math.max(24, g.ry)) {
        glowDrag = { idx: fxGlowSelected };
        (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId); e.preventDefault(); return;
      }
    }
    if (fxOverlaySelected >= 0) {
      const r = sim.overlayRect(fxOverlaySelected);
      const hs = HANDLE / geom.fit;
      if (r && Math.abs(p.x - (r.x + r.w)) <= hs && Math.abs(p.y - (r.y + r.h)) <= hs) {
        ovResize = { idx: fxOverlaySelected, x0: r.x, y0: r.y, w: r.w, h: r.h, aspect: r.w / Math.max(1, r.h) };
        (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId); e.preventDefault(); return;
      }
      if (r && p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h) {
        ovDrag = { idx: fxOverlaySelected, gx: p.x - r.x, gy: p.y - r.y, x: r.x, y: r.y };
        (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId); e.preventDefault(); return;
      }
    }
  }
  function pMove(e: PointerEvent) {
    const p = toLogical(e); if (!p) return;
    if (glowDrag) { sim.setGlowPos(glowDrag.idx, p.x, p.y); return; }
    if (ovResize) {
      let w = Math.max(4, p.x - ovResize.x0), h = Math.max(4, p.y - ovResize.y0);
      if (e.shiftKey) h = w / ovResize.aspect;
      ovResize.w = w; ovResize.h = h;
      sim.setOverlaySize(ovResize.idx, w, h); return;
    }
    if (ovDrag) { ovDrag.x = p.x - ovDrag.gx; ovDrag.y = p.y - ovDrag.gy; sim.setOverlayPos(ovDrag.idx, ovDrag.x, ovDrag.y); }
  }
  function pUp(e: PointerEvent) {
    const p = toLogical(e);
    if (glowDrag) { const d = glowDrag; glowDrag = null; if (p) onFxGlowMove?.(d.idx, Math.round(p.x), Math.round(p.y)); return; }
    if (ovResize) { const d = ovResize; ovResize = null; onFxOverlayResize?.(d.idx, Math.round(d.w), Math.round(d.h)); return; }
    if (ovDrag) { const d = ovDrag; ovDrag = null; onFxOverlayMove?.(d.idx, Math.round(d.x), Math.round(d.y)); }
  }
  function drawOutline(ctx: CanvasRenderingContext2D) {
    if (fxGlowSelected >= 0) {
      const g = sim.glowCenter(fxGlowSelected);
      if (g) {
        ctx.save(); ctx.lineWidth = 2 / geom.fit; ctx.setLineDash([5 / geom.fit, 4 / geom.fit]);
        ctx.strokeStyle = "rgba(251,191,36,0.9)";
        ctx.beginPath(); ctx.ellipse(g.x, g.y, g.rx, g.ry, 0, 0, Math.PI * 2); ctx.stroke(); ctx.restore();
      }
    }
    if (fxOverlaySelected >= 0) {
      const r = sim.overlayRect(fxOverlaySelected);
      if (r) {
        ctx.save(); ctx.lineWidth = 2 / geom.fit; ctx.setLineDash([6 / geom.fit, 4 / geom.fit]);
        ctx.strokeStyle = "rgba(56,189,248,0.9)";
        ctx.strokeRect(r.x, r.y, r.w, r.h);
        const hs = HANDLE / geom.fit;
        ctx.setLineDash([]); ctx.fillStyle = "rgba(56,189,248,0.9)";
        ctx.fillRect(r.x + r.w - hs / 2, r.y + r.h - hs / 2, hs, hs);
        ctx.restore();
      }
    }
  }
  let editing = $derived(fxOverlaySelected >= 0 || fxGlowSelected >= 0);

  function frame(now: number) {
    raf = requestAnimationFrame(frame);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const k = fxDoc ? JSON.stringify(fxDoc) : "";
    if (k !== key) { key = k; sim.load(fxDoc, 1, 1); }
    const w = canvas.width, h = canvas.height;
    ctx.clearRect(0, 0, w, h);
    if (!fxDoc || (!fxDoc.particles.length && !fxDoc.overlays.length && !fxDoc.glows.length)) return;
    // letterbox 1920×1080 into the element (same as the video's object-fit: contain)
    const fit = Math.min(w / 1920, h / 1080);
    geom = { fit, ox: (w - 1920 * fit) / 2, oy: (h - 1080 * fit) / 2 };
    ctx.setTransform(fit, 0, 0, fit, geom.ox, geom.oy);
    sim.draw(ctx, 0, now);
    sim.draw(ctx, 1, now);
    sim.draw(ctx, 2, now);
    drawOutline(ctx);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
  }

  export function fxTrigger(k: string) { sim.trigger(k, performance.now()); }
  export function fxStats() { return sim.stats; }

  onMount(() => {
    raf = requestAnimationFrame(frame);
    const vis = () => { if (document.hidden) { cancelAnimationFrame(raf); raf = 0; } else if (!raf) raf = requestAnimationFrame(frame); };
    document.addEventListener("visibilitychange", vis);
    return () => document.removeEventListener("visibilitychange", vis);
  });
  onDestroy(() => { if (raf) cancelAnimationFrame(raf); });

  function fitToParent(el: HTMLCanvasElement) {
    const parent = el.parentElement;
    if (!parent) return;
    const ro = new ResizeObserver(() => {
      const r = parent.getBoundingClientRect();
      el.width = Math.round(r.width); el.height = Math.round(r.height);
    });
    ro.observe(parent);
    const r = parent.getBoundingClientRect();
    el.width = Math.round(r.width); el.height = Math.round(r.height);
    return { destroy() { ro.disconnect(); } };
  }
</script>

<!-- Pointer events only while a layer is selected for editing — otherwise the
     canvas stays click-through so the <video> controls keep working. -->
<canvas bind:this={canvas} use:fitToParent
        class="absolute inset-0 w-full h-full {editing ? 'cursor-move' : 'pointer-events-none'}"
        onpointerdown={pDown} onpointermove={pMove} onpointerup={pUp} onpointercancel={pUp}></canvas>
