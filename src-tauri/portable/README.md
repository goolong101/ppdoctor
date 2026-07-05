# PP Doctor -- distribution & portability

## Default: ship the standalone exe (~15.5 MB). **Windows 10 or higher required.**

`npm run tauri build` produces `target\release\pp-doctor.exe` (~15.5 MB). Ship that
file by itself. It uses the **system WebView2 runtime**, which is a built-in OS
component: guaranteed present on Windows 11, and delivered to Windows 10 via
Windows Update / Edge. So on any normal, current Windows box it just runs.

There is no way to make a bundled-runtime build ~100 MB: WebView2 *is* Chromium and
its core `msedge.dll` alone is ~313 MB. The small exe (relying on the OS runtime) is
the only path under 100 MB, and it's 15.5 MB.

## `main.rs` portable hook (optional, no rebuild needed)

`use_portable_webview2()` in `src/main.rs` runs before `run()`: if a `WebView2\`
folder containing `msedgewebview2.exe` sits next to the exe, it points the WebView2
loader at it (`WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`). No folder -> uses the system
runtime. So the *same* 15.5 MB exe covers both cases:

- ship it alone -> system runtime (needs Windows 10+ with WebView2, i.e. ~everything).
- ship it next to a `WebView2\` folder -> runs even on a stripped Windows lacking WebView2.

Verified 2026-07-05: with a full runtime folder adjacent, the exe spawned its
`msedgewebview2.exe` children from the bundled folder (distinct path from the system
runtime) and the window rendered.

## If you must bundle for a stripped machine

Realistic size is **~480 MB** (Chromium is big). Assemble with:

```
.\portable\build-portable.ps1 -RuntimeSource "C:\path\to\runtime-folder"
```

- Best runtime source: the official **Fixed Version** cab (x64) from
  <https://developer.microsoft.com/microsoft-edge/webview2/> -> "Fixed Version".
  The link is generated client-side by that page (a Nuxt SPA), so it can't be
  curl'd -- download it in a browser and extract the `.cab` with
  `expand -F:* <file>.cab <dest>`.
- A local Evergreen install
  (`C:\Program Files (x86)\Microsoft\EdgeWebView\Application\<ver>`) also works but
  is ~854 MB (carries Edge extras + user-data).
- **Do NOT naively trim the Evergreen folder.** A `-Trim` mode exists on the script,
  but a 480 MB trim of the Evergreen set FAILED to init (window title "Error") --
  one of `dual_engine_adapter_x64.dll`, `elevated_tracing_service.exe`,
  `mscopilot.exe`, `mspdf.dll`, `oneauth.dll`, `uc_connector.exe` is load-bearing
  for environment creation and can't just be dropped. Use the official Fixed
  Version cab, which is trimmed correctly by Microsoft.

## Installer path (not portable, but bulletproof)

The `.msi` / `-setup.exe` bundles use the default `downloadBootstrapper` install
mode, which auto-installs WebView2 at install time if it's missing. Use these when
you want a guaranteed install on any Windows 10+ machine.
