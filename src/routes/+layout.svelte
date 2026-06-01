<script lang="ts">
  import "../app.css";
  import TitleBar from "$lib/TitleBar.svelte";
  import StatusBar from "$lib/StatusBar.svelte";
  import FirstSyncModal from "$lib/FirstSyncModal.svelte";
  let { children } = $props();
</script>

<div class="flex flex-col h-screen overflow-hidden">
  <TitleBar />
  <div class="flex-1 overflow-hidden">
    {@render children()}
  </div>
  <StatusBar />
  <!-- Shows ONLY when sessionStorage("ppe.first-sync-host") is set
       (by Connect screen after a clean-cache cab choice). Self-mounts,
       runs sync_pull_all with progress, self-dismisses on completion. -->
  <FirstSyncModal />
</div>
