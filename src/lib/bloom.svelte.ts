/** Shared bloom-tuning state for B2SCanvas preview.
 *
 *  Pi cabinet bloom math (renderer.cpp:4441 + 4569-4590):
 *    passes = MIN_PASSES + (LIT_PASSES - MIN_PASSES) * runtime_alpha
 *  with N stacked BLENDMODE_BLEND blits on the FxB layer, then FxB
 *  composited onto base additively. Defaults match the Pi (MIN_PASSES=1,
 *  LIT_PASSES=5, layerPulse 230→255 over 3s) so a fresh PP Doctor install
 *  renders identically to the cabinet — adjustments here diverge from the
 *  Pi until pushed (renderer.cpp + rebuild + deploy).
 *
 *  Values persist to localStorage under ppe.bloom.* and reload on next
 *  app start. B2SCanvas reads bloomState every frame, so slider changes
 *  are reflected live in the preview without needing a save.
 */

const LS = {
  litPasses:    "ppe.bloom.lit_passes",
  minPasses:    "ppe.bloom.min_passes",
  layerMin:     "ppe.bloom.layer_min",
  layerMax:     "ppe.bloom.layer_max",
  cycleSeconds: "ppe.bloom.cycle_seconds",
} as const;

export const DEFAULTS = {
  litPasses: 5,
  minPasses: 1,
  layerMin: 230,
  layerMax: 255,
  cycleSeconds: 3.0,
} as const;

function loadNum(key: string, def: number): number {
  const v = typeof localStorage !== "undefined" ? localStorage.getItem(key) : null;
  if (v === null) return def;
  const n = parseFloat(v);
  return Number.isFinite(n) ? n : def;
}

export const bloomState = $state({
  litPasses:    loadNum(LS.litPasses,    DEFAULTS.litPasses),
  minPasses:    loadNum(LS.minPasses,    DEFAULTS.minPasses),
  layerMin:     loadNum(LS.layerMin,     DEFAULTS.layerMin),
  layerMax:     loadNum(LS.layerMax,     DEFAULTS.layerMax),
  cycleSeconds: loadNum(LS.cycleSeconds, DEFAULTS.cycleSeconds),
});

export function saveBloom() {
  localStorage.setItem(LS.litPasses,    String(bloomState.litPasses));
  localStorage.setItem(LS.minPasses,    String(bloomState.minPasses));
  localStorage.setItem(LS.layerMin,     String(bloomState.layerMin));
  localStorage.setItem(LS.layerMax,     String(bloomState.layerMax));
  localStorage.setItem(LS.cycleSeconds, String(bloomState.cycleSeconds));
}

export function revertBloom() {
  bloomState.litPasses    = DEFAULTS.litPasses;
  bloomState.minPasses    = DEFAULTS.minPasses;
  bloomState.layerMin     = DEFAULTS.layerMin;
  bloomState.layerMax     = DEFAULTS.layerMax;
  bloomState.cycleSeconds = DEFAULTS.cycleSeconds;
}
