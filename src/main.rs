#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod download;
mod update_check;

use download::{CancelToken, ScannedFile, Update};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    thread,
};
use update_check::{AppUpdate, CheckResult, EMBEDDED_ENGINE_VERSION, RELEASES_PAGE_URL};

slint::include_modules!();

struct SelectableFile {
    scanned: ScannedFile,
    name: String,
    rel_folder: String,
    size_text: String,
    checked: bool,
}

struct ScannedCache {
    url: String,
    key: String,
    items: Vec<SelectableFile>,
}

struct AppState {
    running: bool,
    checking_engine: bool,
    updating_app: bool,
    cancel: Option<CancelToken>,
    last_folder: Option<PathBuf>,
    engine_release_url: String,
    available_app_update: Option<AppUpdate>,
    scanned: Option<ScannedCache>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            running: false,
            checking_engine: false,
            updating_app: false,
            cancel: None,
            last_folder: None,
            engine_release_url: RELEASES_PAGE_URL.to_owned(),
            available_app_update: None,
            scanned: None,
        }
    }
}

fn default_downloads_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let downloads = home.join("Downloads");
    if downloads.is_dir() {
        downloads
    } else {
        home
    }
}

fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1_048_576.0;
    const GIB: f64 = 1_073_741_824.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn format_mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}

fn split_rel_path(rel_path: &str) -> (String, String) {
    match rel_path.rsplit_once('/') {
        Some((dir, name)) => (name.to_owned(), dir.to_owned()),
        None => (rel_path.to_owned(), String::new()),
    }
}

fn item_matches_filter(item: &SelectableFile, filter_lower: &str) -> bool {
    filter_lower.is_empty() || item.scanned.rel_path.to_lowercase().contains(filter_lower)
}

fn sync_selector_ui(ui: &AppWindow, state: &AppState) {
    let Some(cache) = state.scanned.as_ref() else {
        ui.set_has_selection(false);
        ui.set_selected_count(0);
        ui.set_total_scanned(0);
        ui.set_selection_summary(SharedString::default());
        ui.set_visible_files(ModelRc::new(VecModel::from(Vec::<FileEntry>::new())));
        return;
    };

    let filter_lower = ui.get_filter_text().to_string().trim().to_lowercase();
    let mut visible = Vec::new();
    let mut selected_count = 0i32;
    let mut selected_bytes = 0u64;

    for (idx, item) in cache.items.iter().enumerate() {
        if item.checked {
            selected_count += 1;
            selected_bytes = selected_bytes.saturating_add(item.scanned.job.size);
        }
        if item_matches_filter(item, &filter_lower) {
            visible.push(FileEntry {
                id: idx as i32,
                name: SharedString::from(&item.name),
                rel_folder: SharedString::from(&item.rel_folder),
                size_text: SharedString::from(&item.size_text),
                checked: item.checked,
            });
        }
    }

    let total = cache.items.len() as i32;
    ui.set_has_selection(true);
    ui.set_selected_count(selected_count);
    ui.set_total_scanned(total);
    ui.set_selection_summary(SharedString::from(format!(
        "{selected_count} di {total} file · {}",
        format_size(selected_bytes)
    )));
    ui.set_visible_files(ModelRc::new(VecModel::from(visible)));
}

fn invalidate_selection_if_url_changed(
    ui: &AppWindow,
    state: &Arc<Mutex<AppState>>,
    new_url: &str,
) {
    let trimmed = new_url.trim();
    if let Ok(mut guard) = state.lock() {
        let should_clear = guard.scanned.as_ref().is_some_and(|c| c.url != trimmed);
        if should_clear {
            guard.scanned = None;
            sync_selector_ui(ui, &guard);
        }
    }
}

fn spawn_engine_check(ui_weak: slint::Weak<AppWindow>, state: Arc<Mutex<AppState>>) {
    if let Ok(mut guard) = state.lock() {
        if guard.checking_engine || guard.updating_app {
            return;
        }
        guard.checking_engine = true;
    }
    if let Some(ui) = ui_weak.upgrade() {
        ui.set_is_checking_engine(true);
        ui.set_engine_status_text(SharedString::from(format!(
            "Motore v{EMBEDDED_ENGINE_VERSION} · Controllo aggiornamenti su GitHub..."
        )));
    }

    thread::spawn(move || {
        let result = update_check::check_updates();
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut guard) = state.lock() {
                guard.checking_engine = false;
                match &result {
                    CheckResult::AppUpdateAvailable { update, .. } => {
                        guard.available_app_update = Some(update.clone());
                    }
                    CheckResult::UpstreamEngineNotice { url, .. } => {
                        guard.available_app_update = None;
                        guard.engine_release_url = url.clone();
                    }
                    _ => {
                        guard.available_app_update = None;
                    }
                }
            }
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_is_checking_engine(false);
            match result {
                CheckResult::AppUpdateAvailable {
                    current_app,
                    update,
                } => {
                    ui.set_app_update_available(true);
                    ui.set_engine_update_available(false);
                    ui.set_engine_update_banner(SharedString::from(format!(
                        "Nuova versione {} disponibile (attuale v{current_app}). Aggiorna con un clic senza creare cartelle.",
                        update.version
                    )));
                    ui.set_engine_status_text(SharedString::from(format!(
                        "Aggiornamento disponibile: {} (attuale v{current_app})",
                        update.version
                    )));
                }
                CheckResult::UpstreamEngineNotice {
                    installed_engine,
                    latest_engine,
                    ..
                } => {
                    ui.set_app_update_available(false);
                    ui.set_engine_update_available(true);
                    ui.set_engine_update_banner(SharedString::from(format!(
                        "Nuova release upstream mediafire_rs {latest_engine} disponibile (motore integrato: v{installed_engine})."
                    )));
                    ui.set_engine_status_text(SharedString::from(format!(
                        "Motore integrato v{installed_engine} · Upstream {latest_engine} disponibile"
                    )));
                }
                CheckResult::UpToDate {
                    app_version,
                    engine_version,
                } => {
                    ui.set_app_update_available(false);
                    ui.set_engine_update_available(false);
                    ui.set_engine_update_banner(SharedString::default());
                    ui.set_engine_status_text(SharedString::from(format!(
                        "v{app_version} · Motore v{engine_version} · Tutto aggiornato"
                    )));
                }
                CheckResult::Error(err) => {
                    ui.set_app_update_available(false);
                    ui.set_engine_update_available(false);
                    ui.set_engine_status_text(SharedString::from(format!(
                        "Motore v{EMBEDDED_ENGINE_VERSION} · Verifica GitHub non riuscita ({err})"
                    )));
                }
            }
        });
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_app_version(SharedString::from(format!(
        "v{}",
        env!("CARGO_PKG_VERSION")
    )));
    ui.set_folder_text(SharedString::from(
        default_downloads_dir().to_string_lossy().as_ref(),
    ));

    let state = Arc::new(Mutex::new(AppState::default()));

    // Non-blocking read-only check for mediafire_rs releases on startup
    spawn_engine_check(ui.as_weak(), state.clone());

    // Manual engine update check
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_check_engine_updates(move || {
            spawn_engine_check(ui_weak.clone(), state.clone());
        });
    }

    // One-click single-EXE self-update
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_apply_app_update(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let update_opt = {
                let Ok(mut guard) = state.lock() else { return };
                if guard.running || guard.updating_app {
                    return;
                }
                let Some(upd) = guard.available_app_update.clone() else {
                    return;
                };
                guard.updating_app = true;
                upd
            };

            ui.set_is_updating_app(true);
            ui.set_is_indeterminate(true);
            ui.set_banner_error(SharedString::default());
            ui.set_status_title(SharedString::from(format!(
                "Aggiornamento a {} in corso...",
                update_opt.version
            )));
            ui.set_status_detail(SharedString::from(
                "Scarico MediaFireMini.exe e verifico SHA256...",
            ));
            ui.set_progress_summary(SharedString::default());

            let ui_thread_weak = ui.as_weak();
            let state_thread = state.clone();
            thread::spawn(move || {
                let res = update_check::download_and_launch_self_update(&update_opt);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Ok(mut guard) = state_thread.lock() {
                        guard.updating_app = false;
                    }
                    let Some(ui) = ui_thread_weak.upgrade() else {
                        return;
                    };
                    ui.set_is_updating_app(false);
                    ui.set_is_indeterminate(false);
                    match res {
                        Ok(()) => {
                            ui.set_status_title(SharedString::from("Riavvio in corso..."));
                            ui.set_status_detail(SharedString::from(
                                "Sostituzione dell'eseguibile e riavvio automatico...",
                            ));
                            std::process::exit(0);
                        }
                        Err(err) => {
                            ui.set_status_title(SharedString::from("Aggiornamento non riuscito"));
                            ui.set_status_detail(SharedString::from(
                                "Il file attuale non è stato modificato.",
                            ));
                            ui.set_banner_error(SharedString::from(err));
                        }
                    }
                });
            });
        });
    }

    // Open GitHub release page in default browser
    {
        let state = state.clone();
        ui.on_open_engine_release(move || {
            if let Ok(guard) = state.lock() {
                let url = &guard.engine_release_url;
                if url.starts_with("https://github.com/NicKoehler/mediafire_rs/") {
                    let _ = Command::new("rundll32.exe")
                        .args(["url.dll,FileProtocolHandler", url])
                        .spawn();
                }
            }
        });
    }

    // Paste link from clipboard
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_paste_url(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(text) = clipboard.get_text() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        ui.set_url_text(SharedString::from(trimmed));
                        ui.set_url_error(SharedString::default());
                        ui.set_banner_error(SharedString::default());
                        invalidate_selection_if_url_changed(&ui, &state, trimmed);
                    }
                }
            }
        });
    }

    // Clear URL input
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_clear_url(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_url_text(SharedString::default());
            ui.set_url_error(SharedString::default());
            invalidate_selection_if_url_changed(&ui, &state, "");
        });
    }

    // Track URL edits to clear stale selection
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_url_edited(move |txt| {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_url_error(SharedString::default());
            invalidate_selection_if_url_changed(&ui, &state, txt.as_str());
        });
    }

    // Native Windows folder picker
    {
        let ui_weak = ui.as_weak();
        ui.on_browse_folder(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let current = PathBuf::from(ui.get_folder_text().as_str().trim());
            let mut dialog = rfd::FileDialog::new().set_title("Scegli la cartella di destinazione");
            if current.is_dir() {
                dialog = dialog.set_directory(&current);
            }
            if let Some(selected) = dialog.pick_folder() {
                ui.set_folder_text(SharedString::from(selected.to_string_lossy().as_ref()));
                ui.set_folder_error(SharedString::default());
                ui.set_banner_error(SharedString::default());
            }
        });
    }

    // Dismiss error banner
    {
        let ui_weak = ui.as_weak();
        ui.on_dismiss_error(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_banner_error(SharedString::default());
        });
    }

    // Open destination folder in Explorer
    {
        let state = state.clone();
        ui.on_open_destination(move || {
            if let Ok(guard) = state.lock() {
                if let Some(folder) = guard.last_folder.as_ref() {
                    let _ = Command::new("explorer.exe").arg(folder).spawn();
                }
            }
        });
    }

    // Cancel active scan/download/zip
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_cancel_download(move || {
            if let Ok(guard) = state.lock() {
                if let Some(cancel) = guard.cancel.as_ref() {
                    cancel.cancel();
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_is_cancelling(true);
                        ui.set_is_indeterminate(true);
                        ui.set_status_title(SharedString::from("Annullamento in corso..."));
                    }
                }
            }
        });
    }

    // Open or scan for the File Selector
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_open_file_selector(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if state.lock().map(|s| s.running).unwrap_or(false) {
                return;
            }

            let url = ui.get_url_text().to_string().trim().to_owned();
            ui.set_url_error(SharedString::default());
            ui.set_banner_error(SharedString::default());

            if let Err(err) = download::parse_key(&url) {
                ui.set_url_error(SharedString::from(err));
                return;
            }

            // If already scanned for this exact URL, open the selector immediately.
            if let Ok(guard) = state.lock() {
                if guard.scanned.as_ref().is_some_and(|c| c.url == url) {
                    sync_selector_ui(&ui, &guard);
                    ui.set_selector_open(true);
                    return;
                }
            }

            let cancel = CancelToken::new();
            if let Ok(mut cur) = state.lock() {
                cur.running = true;
                cur.cancel = Some(cancel.clone());
            }

            ui.set_is_running(true);
            ui.set_is_indeterminate(true);
            ui.set_is_cancelling(false);
            ui.set_progress_value(0.0);
            ui.set_status_title(SharedString::from("Analisi dei file in corso..."));
            ui.set_status_detail(SharedString::from("Recupero elenco da MediaFire..."));
            ui.set_progress_summary(SharedString::default());

            let ui_thread_weak = ui.as_weak();
            let state_thread = state.clone();
            let scanned_url = url.clone();

            thread::spawn(move || {
                download::scan(&url, cancel, move |update| {
                    let ui_weak = ui_thread_weak.clone();
                    let state = state_thread.clone();
                    let scanned_url = scanned_url.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui_weak.upgrade() else { return };
                        handle_update(&ui, &state, Some(&scanned_url), update);
                    });
                });
            });
        });
    }

    // Selector UI controls
    {
        let ui_weak = ui.as_weak();
        ui.on_close_selector(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_selector_open(false);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_reset_selection(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(mut guard) = state.lock() {
                guard.scanned = None;
                sync_selector_ui(&ui, &guard);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_filter_changed(move |txt| {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_filter_text(txt);
            if let Ok(guard) = state.lock() {
                sync_selector_ui(&ui, &guard);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_toggle_file(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(mut guard) = state.lock() {
                if let Some(cache) = guard.scanned.as_mut() {
                    if let Some(item) = cache.items.get_mut(id as usize) {
                        item.checked = !item.checked;
                    }
                }
                sync_selector_ui(&ui, &guard);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_select_all_visible(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let filter_lower = ui.get_filter_text().to_string().trim().to_lowercase();
            if let Ok(mut guard) = state.lock() {
                if let Some(cache) = guard.scanned.as_mut() {
                    for item in &mut cache.items {
                        if item_matches_filter(item, &filter_lower) {
                            item.checked = true;
                        }
                    }
                }
                sync_selector_ui(&ui, &guard);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_deselect_all_visible(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let filter_lower = ui.get_filter_text().to_string().trim().to_lowercase();
            if let Ok(mut guard) = state.lock() {
                if let Some(cache) = guard.scanned.as_mut() {
                    for item in &mut cache.items {
                        if item_matches_filter(item, &filter_lower) {
                            item.checked = false;
                        }
                    }
                }
                sync_selector_ui(&ui, &guard);
            }
        });
    }

    // Start download from selector or main screen
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_start_selected_download(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_selector_open(false);
            trigger_download(&ui, &state);
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_start_download(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            trigger_download(&ui, &state);
        });
    }

    // Prevent closing while an operation is active
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.window().on_close_requested(move || {
            if state
                .lock()
                .map(|s| s.running || s.updating_app)
                .unwrap_or(false)
            {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.invoke_show_busy_dialog();
                }
                CloseRequestResponse::KeepWindowShown
            } else {
                CloseRequestResponse::HideWindow
            }
        });
    }

    ui.run()
}

fn trigger_download(ui: &AppWindow, state: &Arc<Mutex<AppState>>) {
    if state
        .lock()
        .map(|s| s.running || s.updating_app)
        .unwrap_or(false)
    {
        return;
    }

    let url = ui.get_url_text().to_string().trim().to_owned();
    let folder_str = ui.get_folder_text().to_string().trim().to_owned();

    ui.set_url_error(SharedString::default());
    ui.set_folder_error(SharedString::default());
    ui.set_banner_error(SharedString::default());

    if let Err(err) = download::parse_key(&url) {
        ui.set_url_error(SharedString::from(err));
        return;
    }

    let output = PathBuf::from(&folder_str);
    if !output.is_dir() {
        ui.set_folder_error(SharedString::from(
            "Seleziona una cartella di destinazione esistente.",
        ));
        return;
    }

    // Use pre-scanned selection if available for this exact URL.
    let selected_jobs = {
        let guard = state.lock().ok();
        guard.and_then(|g| {
            g.scanned.as_ref().filter(|c| c.url == url).map(|c| {
                let jobs: Vec<_> = c
                    .items
                    .iter()
                    .filter(|i| i.checked)
                    .map(|i| i.scanned.job.clone())
                    .collect();
                (c.key.clone(), jobs)
            })
        })
    };

    if let Some((_, ref jobs)) = selected_jobs {
        if jobs.is_empty() {
            ui.set_banner_error(SharedString::from(
                "Nessun file selezionato. Scegli almeno un file oppure clicca 'Scarica tutti'.",
            ));
            return;
        }
    }

    let make_zip = ui.get_make_zip();
    let cancel = CancelToken::new();

    if let Ok(mut cur) = state.lock() {
        cur.running = true;
        cur.cancel = Some(cancel.clone());
        cur.last_folder = None;
    }

    ui.set_is_running(true);
    ui.set_is_indeterminate(true);
    ui.set_is_cancelling(false);
    ui.set_can_open_folder(false);
    ui.set_progress_value(0.0);
    ui.set_status_title(SharedString::from("Scansione link MediaFire..."));
    ui.set_status_detail(SharedString::from("Connessione in corso..."));
    ui.set_progress_summary(SharedString::default());

    let ui_thread_weak = ui.as_weak();
    let state_thread = state.clone();

    thread::spawn(move || match selected_jobs {
        Some((key, jobs)) => {
            download::run_selected(&key, jobs, &output, make_zip, cancel, move |update| {
                let ui_weak = ui_thread_weak.clone();
                let state = state_thread.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    handle_update(&ui, &state, None, update);
                });
            });
        }
        None => {
            download::run(&url, &output, make_zip, cancel, move |update| {
                let ui_weak = ui_thread_weak.clone();
                let state = state_thread.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    handle_update(&ui, &state, None, update);
                });
            });
        }
    });
}

fn handle_update(
    ui: &AppWindow,
    state: &Arc<Mutex<AppState>>,
    scanned_url: Option<&str>,
    update: Update,
) {
    match update {
        Update::Scanning(msg) => {
            ui.set_is_indeterminate(true);
            ui.set_status_title(SharedString::from("Analisi cartella MediaFire..."));
            ui.set_status_detail(SharedString::from(msg));
            ui.set_progress_summary(SharedString::default());
        }
        Update::ScanReady { key, files } => {
            let total = files.len();
            let items: Vec<SelectableFile> = files
                .into_iter()
                .map(|scanned| {
                    let (name, rel_folder) = split_rel_path(&scanned.rel_path);
                    let size_text = format_size(scanned.job.size);
                    SelectableFile {
                        scanned,
                        name,
                        rel_folder,
                        size_text,
                        checked: true,
                    }
                })
                .collect();

            if let Ok(mut cur) = state.lock() {
                cur.running = false;
                cur.cancel = None;
                cur.scanned = Some(ScannedCache {
                    url: scanned_url.unwrap_or_default().to_owned(),
                    key,
                    items,
                });
                sync_selector_ui(ui, &cur);
            }
            ui.set_is_running(false);
            ui.set_is_indeterminate(false);
            ui.set_is_cancelling(false);
            ui.set_progress_value(0.0);
            ui.set_filter_text(SharedString::default());
            if let Ok(cur) = state.lock() {
                sync_selector_ui(ui, &cur);
            }
            ui.set_status_title(SharedString::from("Elenco pronto"));
            ui.set_status_detail(SharedString::from(format!(
                "Trovati {total} file. Seleziona quelli da scaricare."
            )));
            ui.set_selector_open(true);
        }
        Update::Downloading {
            completed_files,
            total_files,
            downloaded_bytes,
            total_bytes,
            current_file,
        } => {
            ui.set_is_indeterminate(total_bytes == 0);
            let ratio = if total_bytes > 0 {
                (downloaded_bytes as f32 / total_bytes as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ui.set_progress_value(ratio);
            ui.set_status_title(SharedString::from(format!(
                "Download in corso ({completed_files}/{total_files} file)"
            )));
            let pct = (ratio * 100.0).round() as u32;
            let summary = if total_bytes > 0 {
                format!(
                    "{} / {} · {}%",
                    format_mib(downloaded_bytes),
                    format_mib(total_bytes),
                    pct
                )
            } else {
                format_mib(downloaded_bytes)
            };
            ui.set_progress_summary(SharedString::from(summary));
            ui.set_status_detail(SharedString::from(format!("File: {current_file}")));
        }
        Update::Zipping {
            current,
            total,
            current_file,
        } => {
            ui.set_is_indeterminate(total == 0);
            let ratio = if total > 0 {
                (current as f32 / total as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ui.set_progress_value(ratio);
            ui.set_status_title(SharedString::from(format!(
                "Creazione archivio ZIP ({current}/{total})..."
            )));
            ui.set_progress_summary(SharedString::from(format!(
                "{}%",
                (ratio * 100.0).round() as u32
            )));
            ui.set_status_detail(SharedString::from(format!("Compressione: {current_file}")));
        }
        Update::Done {
            folder,
            zip,
            files,
            bytes,
        } => {
            if let Ok(mut cur) = state.lock() {
                cur.running = false;
                cur.cancel = None;
                cur.last_folder = Some(folder.clone());
            }
            ui.set_is_running(false);
            ui.set_is_indeterminate(false);
            ui.set_is_cancelling(false);
            ui.set_can_open_folder(true);
            ui.set_progress_value(1.0);
            ui.set_status_title(SharedString::from("Download completato"));
            ui.set_progress_summary(SharedString::from(format!(
                "{files} file · {} · 100%",
                format_mib(bytes)
            )));
            let detail = match zip {
                Some(zip_path) => format!(
                    "Salvato in {} e compresso in {}",
                    folder.display(),
                    zip_path.file_name().unwrap_or_default().to_string_lossy()
                ),
                None => format!("Salvato in {}", folder.display()),
            };
            ui.set_status_detail(SharedString::from(detail));
        }
        Update::Cancelled { folder, has_files } => {
            if let Ok(mut cur) = state.lock() {
                cur.running = false;
                cur.cancel = None;
                cur.last_folder = folder;
            }
            ui.set_is_running(false);
            ui.set_is_indeterminate(false);
            ui.set_is_cancelling(false);
            ui.set_can_open_folder(has_files);
            ui.set_progress_value(0.0);
            ui.set_status_title(SharedString::from("Operazione annullata"));
            ui.set_progress_summary(SharedString::default());
            ui.set_status_detail(SharedString::from(if has_files {
                "I file già scaricati sono stati conservati."
            } else {
                "Nessun file scaricato."
            }));
        }
        Update::Error(err) => {
            if let Ok(mut cur) = state.lock() {
                cur.running = false;
                cur.cancel = None;
            }
            ui.set_is_running(false);
            ui.set_is_indeterminate(false);
            ui.set_is_cancelling(false);
            ui.set_progress_value(0.0);
            ui.set_status_title(SharedString::from("Operazione non riuscita"));
            ui.set_progress_summary(SharedString::default());
            ui.set_status_detail(SharedString::from(
                "Controlla il messaggio di errore qui sopra.",
            ));
            ui.set_banner_error(SharedString::from(err));
        }
    }
}
