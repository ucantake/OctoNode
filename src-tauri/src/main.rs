// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    linux_webkit_workarounds();
    octonode_lib::run()
}

/// WebKitGTK workarounds. Must run before GTK/WebKit initialize and before any
/// other thread exists (environment mutation is not thread-safe).
///
/// On Wayland, WebKitGTK's DMA-BUF renderer fails on many compositor/driver
/// combinations (NVIDIA with explicit sync above all, and AppImages that bundle
/// their own libraries): the window dies with
/// `Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display`.
/// Disabling that renderer trades a little GPU efficiency for a window that
/// opens everywhere. Every variable is only set when the user has not set it,
/// so each workaround can be reverted (e.g. `WEBKIT_DISABLE_DMABUF_RENDERER=0`).
#[cfg(target_os = "linux")]
fn linux_webkit_workarounds() {
    fn set_default(key: &str, value: &str) {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
        }
    }

    set_default("WEBKIT_DISABLE_DMABUF_RENDERER", "1");

    // NVIDIA proprietary driver + Wayland explicit sync (driver >= 555).
    if std::path::Path::new("/proc/driver/nvidia/version").exists() {
        set_default("__NV_DISABLE_EXPLICIT_SYNC", "1");
    }

    // Last resort: run through XWayland.
    if std::env::var_os("OCTONODE_FORCE_X11").is_some_and(|v| v == "1") {
        std::env::set_var("GDK_BACKEND", "x11");
    }
}
