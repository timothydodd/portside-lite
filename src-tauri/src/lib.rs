mod commands;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use portside_core::{ClusterSnapshot, Settings};
use portside_monitor::{EventSink, Monitor, Status};
use portside_store::Store;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

const TRAY_ID: &str = "main";

/// Bring the main window back from the tray (or from behind other windows).
fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Forwards monitor callbacks to the webview as global events.
struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn snapshot(&self, snapshot: &ClusterSnapshot) {
        let _ = self.0.emit("cluster:snapshot", snapshot);
    }
    fn status(&self, status: &Status) {
        let _ = self.0.emit("cluster:status", status);
    }
    fn settings_changed(&self, settings: &Settings) {
        let _ = self.0.emit("settings:changed", settings.redacted());
        if let Some(toggle) = self.0.try_state::<TrayToggle>() {
            let _ = toggle.0.set_text(toggle_label(settings.monitoring_paused));
        }
    }
    fn logs_synced(&self, lines: usize) {
        let _ = self.0.emit("logs:synced", lines);
    }
    fn notify(&self, title: &str, body: &str) {
        let _ = self.0.notification().builder().title(title).body(body).show();
    }
    fn tray_status(&self, tooltip: &str) {
        if let Some(tray) = self.0.tray_by_id(TRAY_ID) {
            let _ = tray.set_tooltip(Some(tooltip));
        }
    }
    fn forwards_changed(&self, forwards: &[portside_monitor::forwards::ForwardInfo]) {
        let _ = self.0.emit("forwards:changed", forwards);
    }
    fn file_progress(&self, progress: &portside_monitor::files::FileProgress) {
        let _ = self.0.emit("files:progress", progress);
    }
}

pub struct AppState {
    pub monitor: Arc<Monitor>,
    /// App data folder; archives default to `archives` inside it.
    pub data_dir: std::path::PathBuf,
}

/// The tray's pause/resume item, kept so its label can follow the setting.
struct TrayToggle(MenuItem<tauri::Wry>);

fn toggle_label(paused: bool) -> &'static str {
    if paused { "Resume monitoring" } else { "Pause monitoring" }
}

/// Tray icon with Open / Check now / Quit. Left click reopens the window.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Portside Lite", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", "Check all clusters now", true, None::<&str>)?;
    let paused = app.state::<AppState>().monitor.settings().monitoring_paused;
    let toggle = MenuItem::with_id(app, "toggle", toggle_label(paused), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &check, &toggle, &PredefinedMenuItem::separator(app)?, &quit],
    )?;
    app.manage(TrayToggle(toggle));

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Portside Lite")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "check" => app.state::<AppState>().monitor.check_all_now(),
            "toggle" => {
                let monitor = Arc::clone(&app.state::<AppState>().monitor);
                tauri::async_runtime::spawn(async move {
                    let paused = monitor.settings().monitoring_paused;
                    let _ = monitor.set_paused(!paused).await;
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Set once the "still running in the tray" hint has been shown this session.
    let tray_hint_shown = Arc::new(AtomicBool::new(false));

    tauri::Builder::default()
        // Must be registered first: a second launch just focuses this instance.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main_window(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = Arc::new(Store::open(&data_dir.join("portside-lite.db"))?);

            let monitor = Monitor::new(store, Arc::new(TauriSink(app.handle().clone())));
            tauri::async_runtime::spawn(Arc::clone(&monitor).run_poll_loop());
            tauri::async_runtime::spawn(Arc::clone(&monitor).run_log_loop());
            tauri::async_runtime::spawn(Arc::clone(&monitor).run_sweep_loop());
            app.manage(AppState { monitor, data_dir });
            build_tray(app)?;
            // Windows start hidden (see tauri.conf.json) so `--tray` (used by the
            // installer's "start with Windows" option) never flashes a window.
            if !std::env::args().any(|a| a == "--tray") {
                show_main_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(move |window, event| {
            // Close → hide to tray (service mode) unless the user turned it off.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                if app.state::<AppState>().monitor.settings().close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                    if !tray_hint_shown.swap(true, Ordering::Relaxed) {
                        let _ = app
                            .notification()
                            .builder()
                            .title("Portside Lite is still running")
                            .body("Monitoring continues in the system tray. Right-click the tray icon to quit.")
                            .show();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::test_connection,
            commands::list_kube_contexts,
            commands::get_status,
            commands::get_snapshot,
            commands::refresh_now,
            commands::sync_logs_now,
            commands::cluster_history,
            commands::node_history,
            commands::pod_history,
            commands::query_logs,
            commands::log_histogram,
            commands::top_error_pods,
            commands::pod_log_counts,
            commands::error_patterns,
            commands::issue_history,
            commands::storage_stats,
            commands::prune_now,
            commands::clear_cluster_data,
            commands::live_logs,
            commands::delete_pod,
            commands::rollout_restart,
            commands::scale_workload,
            commands::set_cordon,
            commands::get_manifest,
            commands::object_events,
            commands::export_manifests,
            commands::edit_manifest,
            commands::apply_manifest,
            commands::save_text_file,
            commands::related_objects,
            commands::export_bundle,
            commands::get_config,
            commands::save_config,
            commands::copy_to_cluster,
            commands::set_workload_disabled,
            commands::delete_workload,
            commands::test_notification,
            commands::check_all_now,
            commands::set_monitoring_paused,
            commands::read_text_files,
            commands::parse_manifests,
            commands::import_manifests,
            commands::start_port_forward,
            commands::stop_port_forward,
            commands::list_port_forwards,
            commands::log_sources,
            commands::archive_root,
            commands::archive_plan,
            commands::archive_workload,
            commands::list_archives,
            commands::archive_manifest,
            commands::restore_archive,
            commands::delete_archive,
            commands::open_archive_folder,
            commands::open_volume_files,
            commands::refresh_volume_files,
            commands::close_volume_files,
            commands::release_volume_files,
            commands::list_volume_files,
            commands::make_volume_dir,
            commands::delete_volume_path,
            commands::delete_volume_claim,
            commands::rename_volume_path,
            commands::download_volume_path,
            commands::upload_volume_files,
            commands::cancel_transfer,
            commands::local_path_info,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Don't leave file-browser helper pods running on the cluster.
            if let tauri::RunEvent::Exit = event {
                let monitor = Arc::clone(&app.state::<AppState>().monitor);
                tauri::async_runtime::block_on(monitor.close_all_files());
            }
        });
}
