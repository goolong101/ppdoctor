// Tiny shared-state singleton for the currently-selected table.
// Used by +page (writer) and StatusBar (reader) so the "Pull this table"
// button knows which row's media to fetch without prop-drilling through
// the layout.
export const selection = $state<{
  id: number | null;
  name: string;
  piFolder: string;
}>({
  id: null,
  name: "",
  piFolder: "",
});
