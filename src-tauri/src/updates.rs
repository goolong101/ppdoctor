// PP Doctor update checks.
//
// Two update channels:
//   1. ppdoctor (self) — compares Cargo.toml version against
//      github.com/goolong101/ppdoctor latest release tag.
//   2. ppdoctor (Pi) — compares /home/pi/PinnerPi/VERSION on the
//      user's cabinet against the latest ppenhancer-updates release tag,
//      then installs by downloading individual release assets and
//      pushing them to their target paths on the Pi.
//
// All GitHub access is anonymous (both repos are public). Anon rate
// limit is 60 req/hour per IP, which is plenty for launch-time checks.
//
// Install-robustness rules (learned from field reports of "says updated
// but the panel still shows the old version"):
//
//   * VERSION is a COMMIT MARKER. It is written last, and only after the
//     running processes have been verified against the release hashes.
//     Writing it early (the old code iterated a HashMap, i.e. random
//     order) meant a partial failure could leave VERSION=new with old
//     binaries — and every later check would report "up to date" forever.
//   * Some assets live at TWO paths, and the second one is the one that
//     actually executes: the daemon execs /home/pi/PinnerPi/pinnerpi_sdl
//     (NOT build/pinnerpi_sdl), and pinnerpi.service execs
//     /usr/local/sbin/pinnerpi-launcher.sh (NOT the app-dir copy). Every
//     copy is hash-checked on every install, so a desynced mirror from an
//     interrupted earlier install is re-deployed even when the canonical
//     copy already matches.
//   * Never write a target file in place. build/pinnerpi_power_daemon is
//     always running while the service is up, so an in-place SFTP write
//     fails with ETXTBSY (and a dropped connection would leave a
//     truncated file). Files are staged, hash-verified on the Pi, then
//     cp'd to <target>.new and atomically mv'd over the target — rename()
//     replaces even a running executable safely.
//   * Success is judged by hashing the RUNNING processes
//     (/proc/<pid>/exe) against SHA256SUMS, not by re-reading files.

use crate::ssh;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const PPDOCTOR_REPO: &str = "goolong101/ppdoctor";
// PUBLIC OTA artifacts repo (Releases). The source repo `goolong101/ppenhancer`
// was flipped PRIVATE 2026-07-07, so anonymous release access must use the
// public `-updates` repo.
const PPENHANCER_REPO: &str = "goolong101/ppenhancer-updates";
const USER_AGENT: &str = "pp-doctor-updater";

const PI_APP_DIR: &str = "/home/pi/PinnerPi";
const STAGE_DIR: &str = "/home/pi/PinnerPi/.ppdr_staging";

/// Deterministic install order. VERSION is deliberately absent — it is the
/// commit marker, deployed separately after the post-restart verification
/// passes (see install_pi_update).
const INSTALL_ORDER: &[&str] = &[
    "pinnerpi_sdl",
    "pinnerpi_power_daemon",
    "commands.json",
    "pinball_tables.json",
    "apply_wifi.sh",
    "pinnerpi-launcher.sh",
    "refresh_golden.sh",
];

/// Every path an asset must exist at on the Pi. The first entry is the
/// canonical location; later entries are mirrors that the system actually
/// executes. All of them are hash-checked and (re-)deployed on install.
fn pi_target_paths(asset_name: &str) -> Vec<String> {
    match asset_name {
        // The daemon execs the ROOT copy (power_daemon.cpp: execl
        // "/home/pi/PinnerPi/pinnerpi_sdl"); build/ is just the canonical
        // deposit the __golden machinery tracks. Both must match.
        "pinnerpi_sdl" => vec![
            format!("{}/build/pinnerpi_sdl", PI_APP_DIR),
            format!("{}/pinnerpi_sdl", PI_APP_DIR),
        ],
        "pinnerpi_power_daemon" => vec![format!("{}/build/pinnerpi_power_daemon", PI_APP_DIR)],
        "commands.json" => vec![format!("{}/commands.json", PI_APP_DIR)],
        "pinball_tables.json" => vec![format!("{}/pinball_tables.json", PI_APP_DIR)],
        "apply_wifi.sh" => vec![format!("{}/apply_wifi.sh", PI_APP_DIR)],
        // pinnerpi.service execs the /usr/local/sbin copy — a separate
        // file, not a symlink. Updating only the app-dir copy silently
        // changes nothing.
        "pinnerpi-launcher.sh" => vec![
            format!("{}/pinnerpi-launcher.sh", PI_APP_DIR),
            "/usr/local/sbin/pinnerpi-launcher.sh".to_string(),
        ],
        "refresh_golden.sh" => vec![format!("{}/refresh_golden.sh", PI_APP_DIR)],
        "VERSION" => vec![format!("{}/VERSION", PI_APP_DIR)],
        _ => vec![],
    }
}

/// chmod mode for a deployed asset.
fn asset_mode(asset_name: &str) -> &'static str {
    if asset_name == "pinnerpi_sdl"
        || asset_name == "pinnerpi_power_daemon"
        || asset_name.ends_with(".sh")
    {
        "755"
    } else {
        "644"
    }
}

#[derive(Serialize, Clone)]
pub struct UpdateCheckResult {
    pub installed: String,
    pub latest: String,
    pub has_update: bool,
    pub release_url: String,
    /// Body of the release notes (markdown). Useful for the UI to show
    /// "what's in this update" before the user clicks install.
    pub release_notes: String,
}

#[derive(Serialize, Clone)]
pub struct InstallReport {
    pub files_updated: Vec<String>,
    pub files_skipped: Vec<String>,
    pub service_restarted: bool,
    pub final_version: String,
    /// True when the RUNNING renderer/daemon processes were hash-verified
    /// against the release after the restart (always true on success —
    /// a failed verification returns an error and does not bump VERSION).
    pub running_verified: bool,
}

// ─── GitHub API helpers ──────────────────────────────────────────────

/// Fetch the latest release JSON for a public repo.
/// Returns (tag, html_url, notes_body, asset_name→download_url).
fn fetch_latest_release(
    repo: &str,
) -> Result<(String, String, String, HashMap<String, String>), String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let resp = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("github api: {}", e))?;
    let v: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("parse releases json: {}", e))?;

    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or("missing tag_name")?
        .to_string();
    let html_url = v
        .get("html_url")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let body = v
        .get("body")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let mut assets = HashMap::new();
    if let Some(arr) = v.get("assets").and_then(|a| a.as_array()) {
        for asset in arr {
            let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let dl = asset
                .get("browser_download_url")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if !name.is_empty() && !dl.is_empty() {
                assets.insert(name.to_string(), dl.to_string());
            }
        }
    }
    Ok((tag, html_url, body, assets))
}

/// Download + parse the release's SHA256SUMS: asset_name → expected hash.
fn fetch_expected_hashes(
    assets: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let sums_url = assets
        .get("SHA256SUMS")
        .ok_or("release missing SHA256SUMS asset")?;
    let sums_bytes = download_bytes(sums_url)?;
    let sums_text = String::from_utf8(sums_bytes).map_err(|e| format!("SHA256SUMS utf8: {}", e))?;

    // Parse "hash *filename" lines.
    let mut expected: HashMap<String, String> = HashMap::new();
    for line in sums_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let hash = parts.next().unwrap_or("").to_lowercase();
        let mut name = parts.next().unwrap_or("").trim().to_string();
        if name.starts_with('*') {
            name.remove(0);
        }
        if !hash.is_empty() && !name.is_empty() {
            expected.insert(name, hash);
        }
    }
    Ok(expected)
}

/// Compare semver-like tags. Strips a leading `v`, parses each
/// dot-separated component as u32 (truncating at the first non-digit),
/// returns true when `latest > installed`. Defensive against missing
/// or weird formats: unparseable parts become 0.
fn version_gt(latest: &str, installed: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.trim().trim_start_matches('v').trim_start_matches('V')
            .split('.')
            .map(|p| {
                let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u32>().unwrap_or(0)
            })
            .collect()
    };
    let mut a = parse(latest);
    let mut b = parse(installed);
    // Right-pad shorter side with zeros so 0.1 vs 0.1.0 compares equal.
    while a.len() < b.len() {
        a.push(0);
    }
    while b.len() < a.len() {
        b.push(0);
    }
    a > b
}

fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("download {}: {}", url, e))?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut buf)
        .map_err(|e| format!("read body: {}", e))?;
    Ok(buf)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

// ─── SSH helpers (delegated to the native russh pool in ssh.rs) ──────

async fn ssh_capture(pool: &ssh::SshPool, host: &str, command: &str) -> Result<String, String> {
    let r = ssh::exec(pool, host, command).await?;
    if !r.ok {
        return Err(r.stderr.trim().to_string());
    }
    Ok(r.stdout.trim().to_string())
}

/// sha256 of a file on the Pi, or "none" when missing/unreadable.
async fn pi_file_hash(pool: &ssh::SshPool, host: &str, path: &str) -> String {
    ssh_capture(
        pool,
        host,
        &format!(
            "[ -f {p} ] && sha256sum {p} | awk '{{print $1}}' || echo none",
            p = path
        ),
    )
    .await
    .unwrap_or_else(|_| "none".to_string())
    .to_lowercase()
}

/// Hash the RUNNING renderer + daemon process images (/proc/<pid>/exe).
/// This is the ground truth for "did the update take effect" — file hashes
/// can't tell whether the old binary is still executing. `delay_secs`
/// sleeps on the Pi side first (used while polling after a restart).
/// Returns (renderer_hash, daemon_hash); None = process not running.
async fn pi_running_hashes(
    pool: &ssh::SshPool,
    host: &str,
    delay_secs: u32,
) -> (Option<String>, Option<String>) {
    // Renderer: plain comm match ("pinnerpi_sdl", 12 chars). Daemon: comm is
    // kernel-truncated to 15 chars so a name match finds nothing — match the
    // full cmdline anchored to the exec'd path instead (launcher does
    // `exec /home/pi/PinnerPi/build/pinnerpi_power_daemon`); the ^ anchor
    // also prevents matching this very shell command's own cmdline.
    let cmd = format!(
        "sleep {}; \
         S=$(pgrep -o pinnerpi_sdl); \
         D=$(pgrep -of '^/home/pi/PinnerPi/build/pinnerpi_power_daemon'); \
         if [ -n \"$S\" ]; then echo \"sdl=$(sudo sha256sum /proc/$S/exe 2>/dev/null | awk '{{print $1}}')\"; else echo sdl=; fi; \
         if [ -n \"$D\" ]; then echo \"dmn=$(sudo sha256sum /proc/$D/exe 2>/dev/null | awk '{{print $1}}')\"; else echo dmn=; fi",
        delay_secs
    );
    let out = match ssh_capture(pool, host, &cmd).await {
        Ok(o) => o,
        Err(_) => return (None, None),
    };
    let mut sdl = None;
    let mut dmn = None;
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("sdl=") {
            if !h.trim().is_empty() {
                sdl = Some(h.trim().to_lowercase());
            }
        } else if let Some(h) = line.strip_prefix("dmn=") {
            if !h.trim().is_empty() {
                dmn = Some(h.trim().to_lowercase());
            }
        }
    }
    (sdl, dmn)
}

/// Does a running-process hash satisfy the release? Assets absent from the
/// release can't be checked and pass; a missing process fails.
fn running_matches(actual: &Option<String>, expected: Option<&String>) -> bool {
    match expected {
        None => true,
        Some(e) => actual.as_deref() == Some(e.as_str()),
    }
}

/// Download an asset, stage it on the Pi, hash-verify the staged copy,
/// then atomically install it to every target path (cp to <target>.new +
/// mv). rename() replaces running executables cleanly, where an in-place
/// write would fail with ETXTBSY.
async fn deploy_asset(
    pool: &ssh::SshPool,
    host: &str,
    asset_name: &str,
    dl_url: &str,
    expected_hash: &str,
    targets: &[String],
) -> Result<(), String> {
    let bytes = download_bytes(dl_url)?;
    let got_hash = sha256_hex(&bytes);
    if got_hash != expected_hash {
        return Err(format!(
            "{}: downloaded hash {} != expected {}",
            asset_name, got_hash, expected_hash
        ));
    }

    let stage = format!("{}/{}", STAGE_DIR, asset_name);
    ssh::sftp_write(pool, host, &stage, &bytes).await?;

    // Verify the staged copy before anything goes live — catches a
    // truncated transfer (dropped connection) that would otherwise be
    // installed as-is.
    let staged_hash = pi_file_hash(pool, host, &stage).await;
    if staged_hash != expected_hash {
        return Err(format!(
            "{}: staged copy hash {} != expected {} (transfer corrupted?)",
            asset_name, staged_hash, expected_hash
        ));
    }

    let mode = asset_mode(asset_name);
    for t in targets {
        ssh_capture(
            pool,
            host,
            &format!(
                "sudo cp -f {stage} {t}.new && sudo chown pi:pi {t}.new \
                 && sudo chmod {mode} {t}.new && sudo mv -f {t}.new {t}",
                stage = stage,
                t = t,
                mode = mode
            ),
        )
        .await
        .map_err(|e| format!("{}: install to {}: {}", asset_name, t, e))?;
    }
    Ok(())
}

// ─── Tauri commands ──────────────────────────────────────────────────

/// Compare PP Doctor's own bundled version against ppdoctor's latest
/// GitHub release. Frontend uses this on app launch to show an
/// "Update PP Doctor" banner.
#[tauri::command]
pub fn check_self_update() -> Result<UpdateCheckResult, String> {
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let (latest, release_url, body, _assets) = fetch_latest_release(PPDOCTOR_REPO)?;
    Ok(UpdateCheckResult {
        has_update: version_gt(&latest, &installed),
        installed,
        latest,
        release_url,
        release_notes: body,
    })
}

/// Compare the Pi's installed PinnerPi version (`/home/pi/PinnerPi/VERSION`)
/// against ppdoctor's latest GitHub release — then, even when VERSION says
/// current, verify the RUNNING renderer/daemon against the release hashes.
/// The deep check catches cabinets where an earlier (pre-verification)
/// updater bumped VERSION without the binaries actually taking effect:
/// those must surface the Install button again so a re-install can heal them.
#[tauri::command]
pub async fn check_pi_update(
    host: String,
    pool: tauri::State<'_, ssh::SshPool>,
) -> Result<UpdateCheckResult, String> {
    let installed = ssh_capture(&pool, &host, "cat /home/pi/PinnerPi/VERSION 2>/dev/null || echo 0.0.0")
        .await
        .unwrap_or_else(|_| "0.0.0".to_string());
    let installed = if installed.is_empty() {
        "0.0.0".to_string()
    } else {
        installed
    };
    let (latest, release_url, body, assets) = fetch_latest_release(PPENHANCER_REPO)?;
    let mut has_update = version_gt(&latest, &installed);

    if !has_update {
        // Deep check, best-effort: never fail the version check over it.
        if let Ok(expected) = fetch_expected_hashes(&assets) {
            let (sdl, dmn) = pi_running_hashes(&pool, &host, 0).await;
            if !running_matches(&sdl, expected.get("pinnerpi_sdl"))
                || !running_matches(&dmn, expected.get("pinnerpi_power_daemon"))
            {
                has_update = true;
            }
        }
    }

    Ok(UpdateCheckResult {
        has_update,
        installed,
        latest,
        release_url,
        release_notes: body,
    })
}

/// Install the latest ppdoctor release on the Pi.
///
/// Algorithm:
///   1. Fetch release JSON + SHA256SUMS.
///   2. For each asset (fixed order, binaries first): hash EVERY target
///      copy on the Pi — including the exec'd mirrors (root pinnerpi_sdl,
///      /usr/local/sbin launcher). All match → skip; any stale → stage,
///      verify staged hash, atomically install to every target.
///   3. Restart pinnerpi.service if anything changed OR the running
///      processes don't match the release (heals a cabinet whose files
///      were updated by an earlier run that never took effect).
///   4. Poll up to ~30 s and hash the RUNNING processes against the
///      release. Mismatch → error, VERSION untouched.
///   5. Only then write VERSION (the commit marker) and refresh __golden.
#[tauri::command]
pub async fn install_pi_update(
    host: String,
    pool: tauri::State<'_, ssh::SshPool>,
) -> Result<InstallReport, String> {
    let (_tag, _html, _body, assets) = fetch_latest_release(PPENHANCER_REPO)?;
    let expected = fetch_expected_hashes(&assets)?;

    ssh_capture(&pool, &host, &format!("mkdir -p {}", STAGE_DIR)).await?;

    let mut updated = Vec::new();
    let mut skipped = Vec::new();

    for asset_name in INSTALL_ORDER {
        let expected_hash = match expected.get(*asset_name) {
            Some(h) => h,
            None => continue, // not shipped in this release
        };
        let targets = pi_target_paths(asset_name);

        let mut stale: Vec<String> = Vec::new();
        for t in &targets {
            if pi_file_hash(&pool, &host, t).await != *expected_hash {
                stale.push(t.clone());
            }
        }
        if stale.is_empty() {
            skipped.push(asset_name.to_string());
            continue;
        }

        let dl_url = assets
            .get(*asset_name)
            .ok_or_else(|| format!("asset {} not in release", asset_name))?;
        deploy_asset(&pool, &host, asset_name, dl_url, expected_hash, &stale).await?;
        updated.push(asset_name.to_string());
    }

    // Restart when files changed — or when they didn't but the running
    // processes still don't match the release (files were correct on disk
    // from an earlier attempt, yet the old binaries kept executing).
    let (run_sdl, run_dmn) = pi_running_hashes(&pool, &host, 0).await;
    let needs_restart = !updated.is_empty()
        || !running_matches(&run_sdl, expected.get("pinnerpi_sdl"))
        || !running_matches(&run_dmn, expected.get("pinnerpi_power_daemon"));

    let service_restarted = if needs_restart {
        ssh_capture(&pool, &host, "sudo systemctl restart pinnerpi.service").await?;
        true
    } else {
        false
    };

    // Verify what is actually EXECUTING — this is what the user sees on
    // the panel. Poll because the launcher (fsck, USB gadget) plus the
    // renderer spawn take several seconds; also long enough to catch the
    // daemon's early-crash __golden auto-revert swapping the old binary
    // back in after repeated renderer crashes.
    if service_restarted {
        let mut verified = false;
        let mut last_state = String::new();
        for attempt in 0..10 {
            let delay = if attempt == 0 { 4 } else { 3 };
            let (sdl, dmn) = pi_running_hashes(&pool, &host, delay).await;
            if running_matches(&sdl, expected.get("pinnerpi_sdl"))
                && running_matches(&dmn, expected.get("pinnerpi_power_daemon"))
            {
                verified = true;
                break;
            }
            last_state = format!(
                "running renderer={} daemon={}",
                sdl.as_deref().unwrap_or("not running"),
                dmn.as_deref().unwrap_or("not running")
            );
        }
        if !verified {
            return Err(format!(
                "files installed, but the running binaries do not match this release after restart ({}). \
                 The Pi may have crash-looped and auto-reverted to its __golden baseline — \
                 check /home/pi/PinnerPi/crashlogs/ on the Pi. VERSION was NOT bumped, \
                 so the update will correctly show as still pending.",
                last_state
            ));
        }
    }

    // Commit marker: VERSION goes last, only after the running system is
    // verified, so check_pi_update can trust it.
    if let Some(vhash) = expected.get("VERSION") {
        let vpath = format!("{}/VERSION", PI_APP_DIR);
        if pi_file_hash(&pool, &host, &vpath).await != *vhash {
            let dl_url = assets
                .get("VERSION")
                .ok_or("asset VERSION not in release")?;
            deploy_asset(&pool, &host, "VERSION", dl_url, vhash, &[vpath]).await?;
            updated.push("VERSION".to_string());
        } else {
            skipped.push("VERSION".to_string());
        }
    }

    // Refresh __golden to the verified runtime, so a later crash-loop
    // auto-restore reverts to THIS version instead of a stale one.
    // Best-effort: never fail the install over a golden refresh, and
    // no-op on cabs that don't yet have refresh_golden.sh.
    if service_restarted {
        let _ = ssh_capture(
            &pool,
            &host,
            "[ -x /home/pi/PinnerPi/refresh_golden.sh ] && \
             sudo /home/pi/PinnerPi/refresh_golden.sh || true",
        )
        .await;
    }

    // Best-effort cleanup of the staging dir.
    let _ = ssh_capture(&pool, &host, &format!("rm -rf {}", STAGE_DIR)).await;

    let final_version = ssh_capture(&pool, &host, "cat /home/pi/PinnerPi/VERSION 2>/dev/null || echo unknown")
        .await
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(InstallReport {
        files_updated: updated,
        files_skipped: skipped,
        service_restarted,
        final_version,
        running_verified: true,
    })
}

// ─── Unit-ish tests for version_gt (run with `cargo test`) ───────────
#[cfg(test)]
mod tests {
    use super::version_gt;
    #[test]
    fn basic() {
        assert!(version_gt("v0.1.1", "v0.1.0"));
        assert!(version_gt("0.2.0", "0.1.99"));
        assert!(!version_gt("v0.1.0", "v0.1.0"));
        assert!(!version_gt("v0.1.0", "v0.1.1"));
    }
    #[test]
    fn length_diff() {
        assert!(!version_gt("0.1", "0.1.0"));
        assert!(!version_gt("0.1.0", "0.1"));
        assert!(version_gt("0.2", "0.1.99"));
    }
    #[test]
    fn pre_release_tags() {
        // "0.1.0-beta1" parses as 0.1.0 (digits-only per component).
        assert!(!version_gt("0.1.0-beta1", "0.1.0"));
        assert!(version_gt("0.2.0-beta1", "0.1.99"));
    }
}
