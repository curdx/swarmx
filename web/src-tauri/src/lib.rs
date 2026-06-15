// flockmux-tauri desktop shell entry.
//
// Sidecar policy:
//   * release build:  Tauri owns the lifecycle of the bundled
//                     flockmux-server binary — spawn at startup,
//                     SUPERVISE it (back-off respawn on crash), and
//                     terminate it when the app exits (closing the main
//                     window hides to tray; real quit is the tray's Quit).
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
//
// Crash honesty (P0): a sidecar that dies after startup must NOT leave the
// user a frozen app that can never reach the backend and never says why.
// The supervisor below (1) keeps a 20-line stderr tail, (2) emits
// `backend-sidecar-down` (with that tail + whether it will auto-retry) so the
// webview can show an honest banner, (3) auto-restarts with a fixed back-off
// up to a small cap, resetting the budget once the process has run stably,
// and (4) exposes a `restart_backend` command the banner's button calls.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};
use tauri_plugin_shell::process::CommandChild;

#[cfg(not(debug_assertions))]
use tauri_plugin_shell::ShellExt;

/// Supervisor state for the bundled server sidecar. Always managed (so the
/// exit hook + `restart_backend` command compile in both build profiles); the
/// respawn fields are only exercised in release builds, where Tauri actually
/// owns the sidecar lifecycle.
#[allow(dead_code)] // generation/attempts are read only by the release-only supervisor
struct ServerSidecar {
    /// Current child handle, so the exit hook (and a restart) can kill it.
    child: Mutex<Option<CommandChild>>,
    /// Bumped on every (re)spawn. A pending back-off respawn whose captured
    /// generation no longer matches has been superseded (by a manual restart
    /// or a newer auto-respawn) and must abort — this is what prevents two
    /// servers racing for port 7777.
    generation: AtomicU64,
    /// Consecutive rapid-failure counter. Reset to 0 once a process has run
    /// long enough to be considered stable, or on a manual restart.
    attempts: AtomicU32,
    /// Set during app exit so the supervisor stays quiet and never respawns a
    /// zombie server as we're quitting.
    shutting_down: AtomicBool,
}

impl ServerSidecar {
    fn new() -> Self {
        Self {
            child: Mutex::new(None),
            generation: AtomicU64::new(0),
            attempts: AtomicU32::new(0),
            shutting_down: AtomicBool::new(false),
        }
    }
}

/// Payload for the `backend-sidecar-down` event the webview turns into a
/// banner. camelCase so it reads naturally from TypeScript.
#[allow(dead_code)] // constructed only in the release-only supervisor
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SidecarDown {
    /// Human-facing reason + the captured stderr tail.
    message: String,
    /// True once we've exhausted the retry budget — the banner should stop
    /// promising an auto-retry and lean on the manual "Restart backend" button.
    permanent: bool,
    /// True while an automatic back-off respawn is still scheduled.
    will_retry: bool,
    /// Which consecutive attempt just failed (for the banner's wording).
    attempt: u32,
}

#[cfg(not(debug_assertions))]
const MAX_SIDECAR_ATTEMPTS: u32 = 3;
#[cfg(not(debug_assertions))]
const SIDECAR_RESTART_DELAY_MS: u64 = 2000;
/// A process that ran at least this long before dying is treated as "was
/// healthy" — its death resets the rapid-failure budget so a one-off crash
/// after hours of uptime doesn't count against the next startup.
#[cfg(not(debug_assertions))]
const SIDECAR_STABLE_MS: u128 = 30_000;

/// Spawn the bundled server sidecar and supervise it. Bumps the generation
/// (superseding any in-flight respawn), kills+replaces any existing child,
/// and spawns an async task that keeps a stderr tail and drives respawn on
/// exit. Release-only: in debug nothing is bundled.
#[cfg(not(debug_assertions))]
fn start_sidecar(app: &tauri::AppHandle) {
    use tauri::Emitter;
    use tauri_plugin_shell::process::CommandEvent;

    let state = app.state::<ServerSidecar>();
    // A fresh generation supersedes any pending back-off respawn / older
    // supervisor: its post-sleep generation check will fail and it will abort.
    let my_gen = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let attempt = state.attempts.fetch_add(1, Ordering::SeqCst) + 1;

    let cmd = match app.shell().sidecar("flockmux-server") {
        Ok(c) => c,
        Err(e) => {
            log::error!("failed to locate flockmux-server sidecar: {e}");
            let _ = app.emit(
                "backend-sidecar-down",
                SidecarDown {
                    message: format!("找不到后端二进制（打包缺失？）：{e}"),
                    permanent: true,
                    will_retry: false,
                    attempt,
                },
            );
            return;
        }
    };

    let (mut rx, child) = match cmd.spawn() {
        Ok(v) => v,
        Err(e) => {
            log::error!("failed to spawn flockmux-server sidecar: {e}");
            handle_sidecar_gone(app, my_gen, 0, format!("启动后端失败：{e}"));
            return;
        }
    };
    log::info!(
        "flockmux-server sidecar started (pid={}, attempt={})",
        child.pid(),
        attempt
    );

    // Kill+replace any previous child (manual-restart path), store the new one.
    {
        let mut slot = state.child.lock().unwrap();
        if let Some(old) = slot.take() {
            let _ = old.kill();
        }
        *slot = Some(child);
    }
    // Tell the webview the backend is (back) up so any down-banner can clear.
    let _ = app.emit("backend-sidecar-up", ());

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        let mut tail: std::collections::VecDeque<String> =
            std::collections::VecDeque::with_capacity(20);
        while let Some(ev) = rx.recv().await {
            match ev {
                CommandEvent::Stderr(bytes) => {
                    let line = String::from_utf8_lossy(&bytes).trim_end().to_string();
                    if !line.is_empty() {
                        if tail.len() == 20 {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                }
                CommandEvent::Error(err) => {
                    // Stream-level error: record it, but let Terminated (or the
                    // stream closing below) drive the respawn decision.
                    log::error!("flockmux-server sidecar error: {err}");
                    if tail.len() == 20 {
                        tail.pop_front();
                    }
                    tail.push_back(format!("error: {err}"));
                }
                CommandEvent::Terminated(payload) => {
                    let trail = tail.iter().cloned().collect::<Vec<_>>().join("\n");
                    log::error!(
                        "flockmux-server sidecar terminated (code={:?}, signal={:?})\n\
                         stderr tail:\n{trail}",
                        payload.code,
                        payload.signal
                    );
                    handle_sidecar_gone(
                        &app,
                        my_gen,
                        started.elapsed().as_millis(),
                        format!("后端进程退出（code={:?}）\n{trail}", payload.code),
                    );
                    return;
                }
                _ => {}
            }
        }
        // Event stream closed without a Terminated we handled — treat the
        // backend as gone so the user still gets an honest banner + respawn.
        let trail = tail.iter().cloned().collect::<Vec<_>>().join("\n");
        handle_sidecar_gone(
            &app,
            my_gen,
            started.elapsed().as_millis(),
            format!("后端连接意外断开\n{trail}"),
        );
    });
}

/// Decide what to do when the supervised sidecar is gone: emit the honest
/// down event and either schedule a back-off respawn or declare it permanent.
/// No-ops if superseded (generation changed) or shutting down.
#[cfg(not(debug_assertions))]
fn handle_sidecar_gone(app: &tauri::AppHandle, my_gen: u64, ran_ms: u128, message: String) {
    use tauri::Emitter;

    let state = app.state::<ServerSidecar>();
    if state.shutting_down.load(Ordering::SeqCst) {
        return; // quitting — never respawn a zombie
    }
    if state.generation.load(Ordering::SeqCst) != my_gen {
        return; // a newer (re)spawn already superseded this one
    }
    // A process that stayed up long enough was healthy — its death starts a
    // fresh budget rather than counting toward the rapid-crash cap.
    if ran_ms >= SIDECAR_STABLE_MS {
        state.attempts.store(0, Ordering::SeqCst);
    }
    let attempt = state.attempts.load(Ordering::SeqCst);
    let permanent = attempt >= MAX_SIDECAR_ATTEMPTS;
    let _ = app.emit(
        "backend-sidecar-down",
        SidecarDown {
            message,
            permanent,
            will_retry: !permanent,
            attempt,
        },
    );
    if permanent {
        // Give up auto-restarting, but reset the budget so the banner's manual
        // "Restart backend" starts from a clean slate.
        state.attempts.store(0, Ordering::SeqCst);
        log::error!(
            "flockmux-server sidecar gave up after {MAX_SIDECAR_ATTEMPTS} rapid attempts; \
             waiting for a manual restart"
        );
    } else {
        schedule_sidecar_retry(app, my_gen, SIDECAR_RESTART_DELAY_MS);
    }
}

/// Schedule a back-off respawn. Uses a plain OS thread + sleep (no tokio dep);
/// re-checks shutdown + generation after the delay so a manual restart during
/// the wait cancels the stale auto-respawn (no double server on 7777).
#[cfg(not(debug_assertions))]
fn schedule_sidecar_retry(app: &tauri::AppHandle, my_gen: u64, delay_ms: u64) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let state = app.state::<ServerSidecar>();
        if state.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        if state.generation.load(Ordering::SeqCst) != my_gen {
            return; // superseded during the back-off — abort
        }
        start_sidecar(&app);
    });
}

/// Manually (re)start the backend — invoked by the down-banner's button.
/// Resets the failure budget so the user always gets a fresh set of attempts.
/// In debug builds the backend is developer-run, so this just explains that.
#[tauri::command]
fn restart_backend(_app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(not(debug_assertions))]
    {
        _app.state::<ServerSidecar>()
            .attempts
            .store(0, Ordering::SeqCst);
        start_sidecar(&_app);
        Ok(())
    }
    #[cfg(debug_assertions)]
    {
        Err("dev 模式下后端由你手动 `cargo run -p flockmux-server` 运行，不能从这里重启".into())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .manage(ServerSidecar::new())
        .invoke_handler(tauri::generate_handler![restart_backend])
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

            // ── Release-only sidecar spawn + supervision ─────────────
            #[cfg(not(debug_assertions))]
            {
                let handle = app.handle().clone();
                start_sidecar(&handle);
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
            // burning agent tokens. Setting shutting_down first stops the
            // supervisor from respawning it as we quit. (No-op in debug:
            // nothing was spawned.)
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<ServerSidecar>();
                state.shutting_down.store(true, Ordering::SeqCst);
                // Take the child out on its own statement so the MutexGuard
                // temporary is dropped at the `;`, not held across the if-let
                // body (which would outlive `state` — E0597).
                let child = state.child.lock().unwrap().take();
                if let Some(child) = child {
                    log::info!("terminating flockmux-server sidecar on app exit");
                    let _ = child.kill();
                }
            }
        });
}
