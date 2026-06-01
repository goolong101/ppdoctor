// PP Doctor update checks.
//
// Two update channels:
//   1. ppdoctor (self) — compares Cargo.toml version against
//      github.com/goolong101/ppdoctor latest release tag.
//   2. ppenhancer (Pi) — compares /home/pi/PinnerPi/VERSION on the
//      user's cabinet against github.com/goolong101/ppenhancer latest
//      release tag, then installs by downloading individual release
//      assets and SCP'ing them to their target paths on the Pi.
//
// All GitHub access is anonymous (both repos are public). Anon rate
// limit is 60 req/hour per IP, which is plenty for launch-time checks.
//
// `SHA256SUMS` in each release lets us skip downloading files that
// already match on the Pi — useful for partial updates where only
// pinnerpi_sdl changed.

use crate::ssh;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const PPDOCTOR_REPO: &str = "goolong101/ppdoctor";
const PPENHANCER_REPO: &str = "goolong101/ppenhancer";
const USER_AGENT: &str = "pp-doctor-updater";

/// Where each ppenhancer release asset lands on the Pi.
/// Binaries go under build/, configs at the repo root.
fn pi_target_path(asset_name: &str) -> Option<&'static str> {
    match asset_name {
        "pinnerpi_sdl" => Some("/home/pi/PinnerPi/build/pinnerpi_sdl"),
        "pinnerpi_power_daemon" => Some("/home/pi/PinnerPi/build/pinnerpi_power_daemon"),
        "commands.json" => Some("/home/pi/PinnerPi/commands.json"),
        "pinball_tables.json" => Some("/home/pi/PinnerPi/pinball_tables.json"),
        "VERSION" => Some("/home/pi/PinnerPi/VERSION"),
        _ => None,
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
/// against ppenhancer's latest GitHub release. Uses the held-open native
/// SSH pool (see ssh.rs) so this is one ~3 ms cat over an existing
/// channel rather than a 200-500 ms ssh.exe spawn + fresh TCP handshake.
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
    let (latest, release_url, body, _assets) = fetch_latest_release(PPENHANCER_REPO)?;
    Ok(UpdateCheckResult {
        has_update: version_gt(&latest, &installed),
        installed,
        latest,
        release_url,
        release_notes: body,
    })
}

/// Install the latest ppenhancer release on the Pi.
///
/// Algorithm:
///   1. Fetch release JSON → asset list (incl. SHA256SUMS).
///   2. Download SHA256SUMS → parse expected hash for each file.
///   3. For each file with a known Pi target path:
///        - SSH `sha256sum <path>` for the currently-installed file.
///        - If hashes match: skip (file already at this version).
///        - If hashes differ or remote file missing: download asset,
///          stash in a temp file, SCP to the target path, chmod +x
///          for binaries.
///   4. `sudo systemctl restart pinnerpi.service` on the Pi.
///   5. Verify by re-reading VERSION.
///
/// Returns an InstallReport so the UI can show what changed.
#[tauri::command]
pub async fn install_pi_update(
    host: String,
    pool: tauri::State<'_, ssh::SshPool>,
) -> Result<InstallReport, String> {
    let (_tag, _html, _body, assets) = fetch_latest_release(PPENHANCER_REPO)?;

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

    let mut updated = Vec::new();
    let mut skipped = Vec::new();

    for (asset_name, expected_hash) in &expected {
        let pi_path = match pi_target_path(asset_name) {
            Some(p) => p,
            None => continue, // unknown asset, skip
        };

        // Get current Pi-side hash (empty if file doesn't exist).
        let cur_hash = ssh_capture(
            &pool,
            &host,
            &format!(
                "[ -f {} ] && sha256sum {} | awk '{{print $1}}' || echo none",
                pi_path, pi_path
            ),
        )
        .await
        .unwrap_or_else(|_| "none".to_string())
        .to_lowercase();

        if &cur_hash == expected_hash {
            skipped.push(asset_name.clone());
            continue;
        }

        // Download new file.
        let dl_url = assets
            .get(asset_name)
            .ok_or_else(|| format!("asset {} not in release", asset_name))?;
        let bytes = download_bytes(dl_url)?;
        let got_hash = sha256_hex(&bytes);
        if &got_hash != expected_hash {
            return Err(format!(
                "{}: downloaded hash {} != expected {}",
                asset_name, got_hash, expected_hash
            ));
        }

        // SFTP-upload directly to the Pi target path. Native russh path
        // means no temp file + scp.exe spawn — bytes go straight over
        // the held-open session.
        ssh::sftp_write(&pool, &host, pi_path, &bytes).await?;

        // chmod +x for the binaries.
        if asset_name == "pinnerpi_sdl" || asset_name == "pinnerpi_power_daemon" {
            ssh_capture(&pool, &host, &format!("chmod +x {}", pi_path)).await?;
        }

        updated.push(asset_name.clone());
    }

    // Only restart the service if something actually changed.
    let service_restarted = if !updated.is_empty() {
        ssh_capture(&pool, &host, "sudo systemctl restart pinnerpi.service").await?;
        true
    } else {
        false
    };

    let final_version = ssh_capture(&pool, &host, "cat /home/pi/PinnerPi/VERSION 2>/dev/null || echo unknown")
        .await
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(InstallReport {
        files_updated: updated,
        files_skipped: skipped,
        service_restarted,
        final_version,
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
