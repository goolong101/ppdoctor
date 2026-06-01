# PPEnhancer

PinnerPi cabinet manager — media uploads, B2S editor, updates. Windows app.

## Stack

- **Tauri 2** — native window, ~5 MB shipped binary
- **SvelteKit + TypeScript** — UI framework
- **Tailwind CSS v4** — styling, dark theme baseline with amber accent
- **Rust backend** — shells to Windows OpenSSH (`ssh.exe`) for SSH/SCP

## Run

Dev mode (hot reload, opens a native window):

```bash
npm run tauri dev
```

First `tauri dev` will fetch + compile the Tauri Rust crates (~3-5 min). Subsequent runs are fast.

Production build:

```bash
npm run tauri build
```

Output: `src-tauri/target/release/PPEnhancer.exe` (~5 MB) + an MSI/EXE installer in `src-tauri/target/release/bundle/`.

## SSH prerequisite

The app uses `ssh.exe` with `BatchMode=yes` — never prompts for passwords, only accepts key auth. First-time setup on the user's PC:

```bash
ssh-copy-id pi@<pi-ip>
```

After that, the app connects silently.

## What works (MVP)

- `/` — Connect screen: IP input, ssh test, persists last IP in localStorage
- `/tables` — Grid of tables from `pinball_tables.json` (fetched via `cat` over SSH)

## What's next

- Thumbnail fetching (SCP `backglass.b2s_base.thumb.jpg` for each table)
- Per-table side panel: current files in each media folder, drag-drop upload
- `.directb2s` editor (canvas-based, zoom/pan/click-to-edit)
- Attract-mode preview (port the renderer's motion engine to JS)
- Update channel integration (push binaries through `__updates/` on the Pi)

## Files

```
src/
  app.css                  Tailwind v4 + dark theme tokens + glass effect
  app.html                 Shell HTML
  routes/
    +layout.svelte         Imports app.css
    +page.svelte           Connect screen
    tables/+page.svelte    Tables grid
  lib/
    api.ts                 Typed wrappers for Rust commands (sshRun, sshCatText)

src-tauri/
  src/lib.rs               ssh_run Tauri command (shells to ssh.exe)
  Cargo.toml               Rust deps
  tauri.conf.json          Window config (dark, 1280x800, centered)
```
