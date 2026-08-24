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
  }: {
    fxDoc?: FxDoc | null;
    fxImageResolver?: ((path: string) => Promise<string | null>) | null;
  } = $props();

  let canvas = $state<HTMLCanvasElement | undefined>();
  const sim = new FxSim();
  let key = "";
  let raf = 0;

  $effect(() => { sim.imageResolver = fxImageResolver; });

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
    ctx.setTransform(fit, 0, 0, fit, (w - 1920 * fit) / 2, (h - 1080 * fit) / 2);
    sim.draw(ctx, 0, now);
    sim.draw(ctx, 1, now);
    sim.draw(ctx, 2, now);
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

<canvas bind:this={canvas} use:fitToParent class="absolute inset-0 w-full h-full pointer-events-none"></canvas>
