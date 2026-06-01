<script lang="ts">
  /** Logo mark for PP Doctor — a tilted blue medicine capsule. */
  let { size = 24, class: cls = "" }: { size?: number; class?: string } = $props();
</script>

<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  class={cls}
  xmlns="http://www.w3.org/2000/svg"
>
  <defs>
    <linearGradient id="pill-dark" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#38bdf8" />
      <stop offset="100%" stop-color="#0369a1" />
    </linearGradient>
    <linearGradient id="pill-light" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#f0f9ff" />
      <stop offset="100%" stop-color="#bae6fd" />
    </linearGradient>
    <filter id="pill-shadow" x="-20%" y="-20%" width="140%" height="140%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="0.4" />
      <feOffset dx="0" dy="0.4" result="off" />
      <feComponentTransfer><feFuncA type="linear" slope="0.4" /></feComponentTransfer>
      <feMerge><feMergeNode /><feMergeNode in="SourceGraphic" /></feMerge>
    </filter>
  </defs>

  <!-- 45° tilted capsule: half deep-blue / half pale-blue, joined seam -->
  <g transform="rotate(-35 12 12)" filter="url(#pill-shadow)">
    <!-- Dark (top) half -->
    <path d="M 3.5 12 A 4.5 4.5 0 0 1 8 7.5 L 12 7.5 L 12 16.5 L 8 16.5 A 4.5 4.5 0 0 1 3.5 12 Z" fill="url(#pill-dark)" />
    <!-- Light (bottom) half -->
    <path d="M 20.5 12 A 4.5 4.5 0 0 0 16 7.5 L 12 7.5 L 12 16.5 L 16 16.5 A 4.5 4.5 0 0 0 20.5 12 Z" fill="url(#pill-light)" />
    <!-- Seam highlight -->
    <line x1="12" y1="7.5" x2="12" y2="16.5" stroke="#0c4a6e" stroke-width="0.5" opacity="0.5" />
    <!-- Glossy highlight on dark side -->
    <ellipse cx="6.5" cy="10" rx="1.5" ry="0.8" fill="white" opacity="0.4" />
  </g>
</svg>
