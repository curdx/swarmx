// flockmux-tauri desktop shell entry.
//
// Sidecar policy:
//   * release build:  Tauri owns the lifecycle of the bundled
//                     flockmux-server binary — spawn at startup,
//                     terminate when the app exits (closing the main
//                     window quits the app; see on_window_event below).
//   * debug build:    we DON'T spawn — local dev workflow expects the
//                     developer to run `cargo run -p flockmux-server`
//                     in a separate terminal so server changes
//                     hot-reload without a Tauri rebuild.
//
// In both modes the web frontend talks to 127.0.0.1:7777 via the vite
// proxy (dev) or directly (prod).
//
// System tray exposes Show / Hide / Quit. Clicking the dock icon or the
// "Show" item brings the main window back if it was hidden.

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};
use tauri_plugin_shell::process::CommandChild;

#[cfg(not(debug_assertions))]
use tauri_plugin_shell::ShellExt;

/// Holds the bundled server sidecar's child handle so we can kill it on exit.
/// Always managed (so the exit hook compiles in both build profiles); only
/// populated in release builds, where Tauri actually spawns the sidecar.
struct ServerSidecar(std::sync::Mutex<Option<CommandChild>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(ServerSidecar(std::sync::Mutex::new(None)))
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // ── System tray ──────────────────────────────────────────
            let show = MenuItemBuilder::with_id("show", "Show flockmux").build(app)?;
            let hide = MenuItemBuilder::with_id("hide", "Hide window").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&show, &hide, &quit]).build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // ── Release-only sidecar spawn ───────────────────────────
            #[cfg(not(debug_assertions))]
            {
                match app.shell().sidecar("flockmux-server") {
                    Ok(cmd) => match cmd.spawn() {
                        Ok((_rx, child)) => {
                            log::info!("flockmux-server sidecar started (pid={})", child.pid());
                            // Stash the child so the exit hook can kill it.
                            *app.state::<ServerSidecar>().0.lock().unwrap() = Some(child);
                        }
                        Err(e) => {
                            log::error!("failed to spawn flockmux-server sidecar: {e}");
                        }
                    },
                    Err(e) => {
                        log::error!("failed to locate flockmux-server sidecar: {e}");
                    }
                }
            }

            Ok(())
        })
        // Closing the main window quits the whole app. This is a single-window
        // tool; "close" should mean "exit", not "leave a headless server +
        // tray behind". macOS keeps apps alive on window-close by default, so
        // we make the quit explicit — which then runs the RunEvent::Exit hook
        // below and tears down the sidecar.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                window.app_handle().exit(0);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app_handle, event| {
            // Kill the bundled server sidecar when the app exits, so it never
            // outlives the window as an orphan still holding port 7777 and
            // burning agent tokens. (No-op in debug: nothing was spawned.)
            if let tauri::RunEvent::Exit = event {
                if let Some(child) = app_handle
                    .state::<ServerSidecar>()
                    .0
                    .lock()
                    .unwrap()
                    .take()
                {
                    log::info!("terminating flockmux-server sidecar on app exit");
                    let _ = child.kill();
                }
            }
        });
}
