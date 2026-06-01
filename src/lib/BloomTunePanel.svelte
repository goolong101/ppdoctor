<script lang="ts">
  import { bloomState, saveBloom, revertBloom, DEFAULTS } from "./bloom.svelte";

  let { open = $bindable(false) }: { open?: boolean } = $props();
  let savedFlash = $state(false);

  type BloomKey = "litPasses" | "minPasses" | "layerMin" | "layerMax" | "cycleSeconds";
  type Knob = {
    label: string;
    key: BloomKey;
    min: number;
    max: number;
    step: number;
    decimals?: number;
    hint?: string;
  };
  const knobs: Knob[] = [
    { label: "Underlying floor (min passes)", key: "minPasses",    min: 1,   max: 5,    step: 1,   hint: "Floor of the stack-blit count — raises bloom on dimly-lit bulbs." },
    { label: "Max bloom (lit_passes)",        key: "litPasses",    min: 3,   max: 12,   step: 1,   hint: "Peak stack-blit count — higher = brighter at full alpha." },
    { label: "Layer alpha min",               key: "layerMin",     min: 0,   max: 255,  step: 1,   hint: "FxB→base composite alpha at the dim end of the breathing pulse." },
    { label: "Layer alpha max",               key: "layerMax",     min: 0,   max: 255,  step: 1,   hint: "FxB→base composite alpha at the bright end of the breathing pulse." },
    { label: "Cycle seconds",                 key: "cycleSeconds", min: 0.5, max: 10.0, step: 0.1, decimals: 1, hint: "Breathing pulse period (s)." },
  ];

  function clamp(k: Knob, v: number) {
    return Math.max(k.min, Math.min(k.max, v));
  }
  function setVal(k: Knob, v: number) {
    const c = clamp(k, v);
    bloomState[k.key] = k.decimals != null ? +c.toFixed(k.decimals) : c;
  }
  function nudge(k: Knob, dir: -1 | 1) {
    setVal(k, (bloomState[k.key] as number) + dir * k.step);
  }
  function isDirty(k: Knob) {
    return (bloomState[k.key] as number) !== (DEFAULTS[k.key] as number);
  }
  function fmt(k: Knob) {
    const v = bloomState[k.key] as number;
    return k.decimals != null ? v.toFixed(k.decimals) : String(v);
  }
  function doSave() {
    saveBloom();
    savedFlash = true;
    setTimeout(() => (savedFlash = false), 1200);
  }
  function close() { open = false; }
</script>

{#if open}
  <div
    class="fixed top-12 right-3 z-40 w-80 p-4 rounded-xl
           bg-zinc-950/90 backdrop-blur-sm border border-white/8 shadow-2xl"
  >
    <div class="flex items-center justify-between mb-3">
      <h3 class="text-sm font-semibold tracking-tight">Bloom tuning</h3>
      <button
        onclick={close}
        class="w-6 h-6 flex items-center justify-center rounded-md
               text-zinc-500 hover:text-zinc-200 hover:bg-white/8 transition-colors"
        aria-label="Close"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M0 0L10 10M10 0L0 10"/></svg>
      </button>
    </div>

    <div class="space-y-3">
      {#each knobs as k (k.key)}
        <div>
          <div class="flex items-baseline justify-between mb-1">
            <label for={"bloom-" + k.key} class="text-[10px] uppercase tracking-wider text-zinc-400">{k.label}</label>
            <span class="text-xs font-mono text-zinc-200 tabular-nums">
              {fmt(k)}
              {#if isDirty(k)}<span class="ml-1 text-amber-400">●</span>{/if}
            </span>
          </div>
          <div class="flex items-center gap-1.5">
            <button
              onclick={() => nudge(k, -1)}
              class="w-6 h-6 flex items-center justify-center rounded-md
                     bg-black/30 border border-white/8 text-zinc-300
                     hover:bg-white/8 hover:text-zinc-100 transition-colors"
              aria-label={"Decrease " + k.label}
            >−</button>
            <input
              id={"bloom-" + k.key}
              type="range"
              min={k.min}
              max={k.max}
              step={k.step}
              value={bloomState[k.key]}
              oninput={(e) => setVal(k, parseFloat((e.target as HTMLInputElement).value))}
              class="flex-1 accent-amber-400"
            />
            <button
              onclick={() => nudge(k, 1)}
              class="w-6 h-6 flex items-center justify-center rounded-md
                     bg-black/30 border border-white/8 text-zinc-300
                     hover:bg-white/8 hover:text-zinc-100 transition-colors"
              aria-label={"Increase " + k.label}
            >+</button>
          </div>
          {#if k.hint}<p class="text-[10px] text-zinc-500 mt-0.5">{k.hint}</p>{/if}
        </div>
      {/each}
    </div>

    <div class="flex items-center gap-2 mt-4 pt-3 border-t border-white/8">
      <button
        onclick={doSave}
        class="flex-1 py-1.5 rounded-lg text-xs font-medium
               bg-gradient-to-b from-amber-400 to-amber-500 text-amber-950
               hover:from-amber-300 hover:to-amber-400 transition-all
               shadow-lg shadow-amber-500/20"
      >Save</button>
      <button
        onclick={revertBloom}
        class="px-3 py-1.5 rounded-lg text-xs text-zinc-300 border border-white/8
               hover:bg-white/5 transition-colors"
      >Revert</button>
    </div>

    {#if savedFlash}
      <div class="mt-2 text-center text-[10px] text-emerald-300">Saved ✓</div>
    {/if}
  </div>
{/if}
