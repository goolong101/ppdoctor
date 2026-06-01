<script lang="ts">
  import "../app.css";
  import TitleBar from "$lib/TitleBar.svelte";
  import StatusBar from "$lib/StatusBar.svelte";
  let { children } = $props();
</script>

<div class="flex flex-col h-screen overflow-hidden">
  <TitleBar />
  <div class="flex-1 overflow-hidden">
    {@render children()}
  </div>
  <StatusBar />
</div>
