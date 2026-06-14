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
        .plugin(tauri_plugin_notification::init())
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
                use tauri::Emitter;
                use tauri_plugin_shell::process::CommandEvent;
                match app.shell().sidecar("flockmux-server") {
                    Ok(cmd) => match cmd.spawn() {
                        Ok((mut rx, child)) => {
                            log::info!("flockmux-server sidecar started (pid={})", child.pid());
                            // Stash the child so the exit hook can kill it.
                            *app.state::<ServerSidecar>().0.lock().unwrap() = Some(child);
                            // P0-5: do NOT drop the event stream. If the sidecar
                            // dies or errors after startup (port 7777 taken, a
                            // read-only HOME, a panic), surface it instead of
                            // leaving the user a frozen app that can never reach
                            // the backend and never says why. We keep a short
                            // stderr tail, log it on exit, and emit an event the
                            // webview can turn into a "backend stopped — restart"
                            // banner.
                            let handle = app.handle().clone();
                            tauri::async_runtime::spawn(async move {
                                let mut tail: std::collections::VecDeque<String> =
                                    std::collections::VecDeque::with_capacity(20);
                                while let Some(ev) = rx.recv().await {
                                    match ev {
                                        CommandEvent::Stderr(bytes) => {
                                            let line =
                                                String::from_utf8_lossy(&bytes).trim_end().to_string();
                                            if !line.is_empty() {
                                                if tail.len() == 20 {
                                                    tail.pop_front();
                                                }
                                                tail.push_back(line);
                                            }
                                        }
                                        CommandEvent::Error(err) => {
                                            log::error!("flockmux-server sidecar error: {err}");
                                            let _ = handle.emit("backend-sidecar-down", err);
                                        }
                                        CommandEvent::Terminated(payload) => {
                                            let trail =
                                                tail.iter().cloned().collect::<Vec<_>>().join("\n");
                                            log::error!(
                                                "flockmux-server sidecar terminated (code={:?}, \
                                                 signal={:?})\nstderr tail:\n{trail}",
                                                payload.code,
                                                payload.signal
                                            );
                                            let _ = handle.emit(
                                                "backend-sidecar-down",
                                                format!(
                                                    "backend exited (code={:?})\n{trail}",
                                                    payload.code
                                                ),
                                            );
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            });
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
        // P1-26: hide the window to the tray on close instead of quitting, so
        // the app (+ its bundled sidecar) keeps running and stays reachable from
        // the tray's "Show". Real quit is the tray's "Quit" item, which calls
        // app.exit(0) → the RunEvent::Exit hook below tears down the sidecar.
        // This makes the "open main window on launch" setting meaningful (the
        // app can now live in the tray) instead of a no-op.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
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
