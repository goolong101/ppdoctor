# Assembles a true USB-portable PP Doctor: the built exe + a bundled WebView2
# runtime, side by side. main.rs points the WebView2 loader at the adjacent
# WebView2\ folder (via WEBVIEW2_BROWSER_EXECUTABLE_FOLDER), so the result runs
# on machines that have never installed the Evergreen runtime.
#
# Usage:
#   .\build-portable.ps1 -RuntimeSource "C:\path\to\Microsoft.WebView2.FixedVersionRuntime.<ver>.x64"
#
# RuntimeSource must be a folder containing msedgewebview2.exe at its root
# (the extracted official Fixed Version .cab -- lean ~180MB, the recommended
# redistributable from https://developer.microsoft.com/microsoft-edge/webview2/
# "Fixed Version" section). A local Evergreen install folder
# (C:\Program Files (x86)\Microsoft\EdgeWebView\Application\<ver>) also works
# but is much larger.

param(
    [Parameter(Mandatory = $true)]
    [string]$RuntimeSource,
    [string]$Exe = "$PSScriptRoot\..\target\release\pp-doctor.exe",
    [string]$OutDir = "$PSScriptRoot\..\target\portable\PP Doctor Portable",
    # -Trim strips Edge-browser extras that WebView2 hosting doesn't need. Use it
    # when RuntimeSource is a local Evergreen install folder (which carries DRM,
    # PDF, Copilot, repair links, ~100 extra locales, and copied user-data). Do
    # NOT use it with an official Fixed Version cab -- that is already trimmed.
    [switch]$Trim
)

$ErrorActionPreference = "Stop"

# Top-level items an Evergreen install carries that a WebView2 host never loads.
# Verified against a launch-test: PP Doctor renders with all of these removed.
$TrimNames = @(
    "EBWebView",                    # copied browsing user-data cache (not runtime)
    "Installer",                    # per-machine installer/updater
    "ResiliencyLinks",              # Edge self-repair hardlink set
    "WidevineCdm",                  # DRM CDM (no DRM playback in PP Doctor)
    "mspdf.dll",                    # PDF viewer engine
    "mscopilot.exe",                # Copilot
    "oneauth.dll",                  # MS account auth
    "uc_connector.exe",            # update connector
    "elevated_tracing_service.exe", # diagnostics service
    "dual_engine_adapter_x64.dll",  # IE-mode dual engine
    "SetupMetrics"
)

if (-not (Test-Path $Exe)) {
    throw "Built exe not found at '$Exe'. Run 'npm run tauri build' first."
}
$rtExe = Join-Path $RuntimeSource "msedgewebview2.exe"
if (-not (Test-Path $rtExe)) {
    throw "RuntimeSource '$RuntimeSource' has no msedgewebview2.exe at its root. Point it at the extracted Fixed Version runtime folder."
}

Write-Host "Assembling portable build -> $OutDir"
if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

Copy-Item $Exe (Join-Path $OutDir "pp-doctor.exe")
$wvOut = Join-Path $OutDir "WebView2"

if ($Trim) {
    New-Item -ItemType Directory -Force -Path $wvOut | Out-Null
    Get-ChildItem -Force $RuntimeSource | Where-Object { $TrimNames -notcontains $_.Name } | ForEach-Object {
        if ($_.Name -eq "Locales") {
            # Keep only en-US / en-GB paks; drop the ~100 other locales.
            $locOut = Join-Path $wvOut "Locales"
            New-Item -ItemType Directory -Force -Path $locOut | Out-Null
            Get-ChildItem -Force $_.FullName | Where-Object { $_.Name -match '^en-(US|GB)\.pak$' } |
                ForEach-Object { Copy-Item $_.FullName $locOut }
        } else {
            Copy-Item -Recurse -Force $_.FullName (Join-Path $wvOut $_.Name)
        }
    }
    Write-Host "Trimmed: dropped $($TrimNames -join ', ') + non-en locales."
} else {
    Copy-Item -Recurse $RuntimeSource $wvOut
}

$sizeMb = "{0:N0}" -f ((Get-ChildItem -Recurse $OutDir | Measure-Object Length -Sum).Sum / 1MB)
Write-Host "Done. Portable folder is ${sizeMb} MB:"
Write-Host "  $OutDir\pp-doctor.exe"
Write-Host "  $OutDir\WebView2\  (bundled runtime)"
Write-Host "Copy the whole 'PP Doctor Portable' folder to a USB stick and run pp-doctor.exe."
