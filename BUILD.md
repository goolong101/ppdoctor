# Building & Releasing PP Doctor

Windows Tauri 2 app (Svelte 5 frontend + Rust backend). Built natively on Windows.

## Prereqs (already on the dev box)
- Node + npm (`npm install` once after clone)
- Rust stable toolchain (MSVC)
- Tauri CLI comes from devDependencies (`@tauri-apps/cli`) — use `npx tauri`, there is no `npm run tauri` script.

## Dev build / checks
```
cd src-tauri
cargo check          # fast backend compile check
cargo test --lib     # unit tests (updates::version_gt, b2scache round-trip)
```

## Release build
```
npx tauri build
```
~3-4 min. Outputs:
- `src-tauri/target/release/pp-doctor.exe` — raw portable exe
- `src-tauri/target/release/bundle/nsis/PP Doctor_<ver>_x64-setup.exe` — NSIS installer
- `src-tauri/target/release/bundle/msi/PP Doctor_<ver>_x64_en-US.msi` — MSI

## Cutting a release (what users' update check consumes)
The in-app update badge reads the **latest GitHub release tag** of `goolong101/ppdoctor`
(`updates.rs::check_self_update`, numeric compare after stripping `v`). A git push alone
does nothing — you must publish a release, and the new tag must compare **greater** than
every installed version.

1. Bump the version in ALL THREE: `package.json`, `src-tauri/tauri.conf.json`,
   `src-tauri/Cargo.toml` (keep them identical; tauri.conf.json is what the app reports).
2. `npx tauri build`
3. Rename assets to the release convention (lowercase, dashed):
   - `pp-doctor-<ver>-x64-setup.exe` (from the NSIS bundle)
   - `pp-doctor-<ver>-x64.msi` (from the MSI bundle)
   - `pp-doctor.exe` (raw, unversioned name)
4. `sha256sum <the three files> > SHA256SUMS`
5. Commit + push the bump, then:
   ```
   gh release create v<ver> --repo goolong101/ppdoctor \
     --title "v<ver> — <summary>" --notes-file <notes.md> \
     pp-doctor-<ver>-x64-setup.exe pp-doctor-<ver>-x64.msi pp-doctor.exe SHA256SUMS
   ```
6. Verify: `gh api repos/goolong101/ppdoctor/releases/latest` shows the new tag + 4 assets.

Binaries are unsigned — release notes should keep the SmartScreen "More info → Run anyway" blurb.

## Related channels
- Pi-side OTA binaries live in `goolong101/ppenhancer-updates` releases (separate
  versioning, currently 1.x) — consumed by `updates.rs::check_pi_update/install_pi_update`.
