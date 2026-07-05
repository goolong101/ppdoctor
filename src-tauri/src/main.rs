// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Portable WebView2 runtime discovery.
///
/// When a `WebView2\` runtime folder ships next to the executable, point the
/// WebView2 loader at it so PP Doctor runs on machines that have never had the
/// Evergreen runtime installed (true USB-portable). When the folder is absent,
/// this is a no-op and the app uses the system-installed runtime as usual.
///
/// Why the env var works: wry passes a null `browserExecutableFolder` to
/// `CreateCoreWebView2EnvironmentWithOptions` (wry 0.55 `webview2/mod.rs:344`),
/// and Microsoft's loader resolves a null folder via
/// `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` first, then the registry. This must run
/// before any webview is created, so it lives here in `main()` ahead of `run()`.
#[cfg(windows)]
fn use_portable_webview2() {
    // Never clobber an override the user/launcher already set.
    if std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_some() {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let rt = dir.join("WebView2");
            if rt.join("msedgewebview2.exe").is_file() {
                std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &rt);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    use_portable_webview2();
    pp_doctor_lib::run()
}
