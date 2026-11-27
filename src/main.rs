#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod download;
mod selector;
mod update_check;

use download::{CancelToken, Update};
use selector::{SelectableFile, Selection};
use serde::{Deserialize, Serialize};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Arc, Mutex},
    thread,
};
use update_check::{AppUpdate, CheckResult, EMBEDDED_ENGINE_VERSION, RELEASES_PAGE_URL};

slint::include_modules!();

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct PersistentCache {
    last_url: String,
}

fn cache_file_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("MediaFireMini").join("cache.json")
}

fn load_cached_url_from(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: PersistentCache = serde_json::from_str(&raw).ok()?;
    let trimmed = parsed.last_url.trim();
    download::parse_key(trimmed).ok()?;
    Some(trimmed.to_owned())
}

fn save_cached_url_to(path: &Path, url: &str) -> io::Result<()> {
    let trimmed = url.trim();
    if download::parse_key(trimmed).is_err() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = PersistentCache {
        last_url: trimmed.to_owned(),
    };
    let json = serde_json::to_vec(&payload)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[derive(Clone)]
struct LastLink {
    pending: Arc<Mutex<Option<String>>>,
    wake: mpsc::SyncSender<()>,
}

impl LastLink {
    fn remember(&self, url: &str) {
        let url = url.trim();
        if download::parse_key(url).is_ok() {
            if let Ok(mut pending) = self.pending.lock() {
                *pending = Some(url.to_owned());
                let _ = self.wake.try_send(());
            }
        }
    }
}

fn start_link_writer(
    path: PathBuf,
    mut saved: Option<String>,
    ui: slint::Weak<AppWindow>,
) -> (LastLink, thread::JoinHandle<()>) {
    let (wake, requests) = mpsc::sync_channel(1);
    let pending = Arc::new(Mutex::new(None::<String>));
    let worker_pending = pending.clone();
    let worker = thread::spawn(move || {
        // Keep only the latest valid edit when input changes faster than disk writes.
        for () in requests {
            let url = worker_pending
                .lock()
                .ok()
                .and_then(|mut pending| pending.take());
            let Some(url) = url else { continue };
            if saved.as_deref() == Some(&url) {
                continue;
            }
            match save_cached_url_to(&path, &url) {
                Ok(()) => saved = Some(url),
                Err(err) => {
                    let ui = ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_banner_error(SharedString::from(format!(
                                "Impossibile ricordare l'ultimo link: {err}"
                            )));
                        }
                    });
                }
            }
        }
    });
    (LastLink { pending, wake }, worker)
}

struct ScannedCache {
    url: String,
    key: String,
    selection: Selection,
}

struct AppState {
    running: bool,
    checking_engine: bool,
    updating_app: bool,
    sort_name_asc: bool,
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
            sort_name_asc: true,
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

fn filter(ui: &AppWindow) -> String {
    ui.get_filter_text().as_str().trim().to_lowercase()
}

fn sync_selector_ui_with_url(ui: &AppWindow, state: &AppState, current_url: &str) {
    let current_trimmed = current_url.trim();
    let Some(cache) = state.scanned.as_ref().filter(|c| c.url == current_trimmed) else {
        ui.set_has_selection(false);
        ui.set_selected_count(0);
        ui.set_total_scanned(0);
        ui.set_selection_summary(SharedString::default());
        ui.set_selector_scope_summary(SharedString::default());
        ui.set_selector_entries(ModelRc::new(VecModel::from(Vec::<SelectorEntry>::new())));
        return;
    };

    let selection = &cache.selection;
    let active_filter = filter(ui);
    let rows = selection.rows(&active_filter, state.sort_name_asc);
    let scoped_selected_count: usize = rows.iter().map(|row| row.selected).sum();
    let visible = rows
        .into_iter()
        .map(|row| {
            let (id, detail, size_text) = match row.file_index {
                Some(index) => (
                    index as i32,
                    if selection.selected_only {
                        row.path
                            .rsplit_once('/')
                            .map_or("", |(folder, _)| folder)
                            .to_owned()
                    } else {
                        String::new()
                    },
                    format_size(selection.items[index].scanned.job.size),
                ),
                None => (
                    -1,
                    format!(
                        "Cartella · {} di {} {}",
                        row.selected,
                        row.total,
                        if active_filter.is_empty() {
                            "file"
                        } else {
                            "file corrispondenti"
                        }
                    ),
                    String::new(),
                ),
            };
            SelectorEntry {
                id,
                name: SharedString::from(row.name),
                path: SharedString::from(row.path),
                detail: SharedString::from(detail),
                size_text: SharedString::from(size_text),
                checked: row.selected == row.total,
                partial: row.selected > 0 && row.selected < row.total,
            }
        })
        .collect::<Vec<_>>();
    let (selected_count, selected_bytes) = selection
        .selected_items()
        .fold((0i32, 0u64), |(count, bytes), item| {
            (count + 1, bytes.saturating_add(item.scanned.job.size))
        });
    let total = selection.items.len() as i32;
    ui.set_selector_path(SharedString::from(if selection.folder.is_empty() {
        "Cartella principale"
    } else {
        &selection.folder
    }));
    ui.set_selector_can_go_up(selection.folder != selection.root && !selection.selected_only);
    ui.set_selector_selected_only(selection.selected_only);
    ui.set_has_selection(true);
    ui.set_selected_count(selected_count);
    ui.set_total_scanned(total);
    ui.set_selection_summary(SharedString::from(format!(
        "{selected_count} di {total} file · {}",
        format_size(selected_bytes)
    )));
    let outside_scope = (selected_count as usize).saturating_sub(scoped_selected_count);
    let scope = if selection.selected_only {
        "Vista globale dei selezionati"
    } else {
        "Azioni sui risultati, anche nelle sottocartelle"
    };
    ui.set_selector_scope_summary(SharedString::from(format!(
        "{scope} · {outside_scope} selezionati fuori ambito"
    )));
    ui.set_selector_entries(ModelRc::new(VecModel::from(visible)));
}

fn sync_selector_ui(ui: &AppWindow, state: &AppState) {
    let current_url = ui.get_url_text().to_string();
    sync_selector_ui_with_url(ui, state, &current_url);
}

fn invalidate_selection_if_url_changed(
    ui: &AppWindow,
    state: &Arc<Mutex<AppState>>,
    new_url: &str,
) {
    if let Ok(guard) = state.lock() {
        sync_selector_ui_with_url(ui, &guard, new_url);
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
    update_check::finish_self_update();
    let ui = AppWindow::new()?;
    ui.set_app_version(SharedString::from(format!(
        "v{}",
        env!("CARGO_PKG_VERSION")
    )));
    ui.set_folder_text(SharedString::from(
        default_downloads_dir().to_string_lossy().as_ref(),
    ));
    let cache_path = cache_file_path();
    let initial_url = load_cached_url_from(&cache_path);
    if let Some(url) = &initial_url {
        ui.set_url_text(SharedString::from(url));
    }
    let (last_link, link_writer) = start_link_writer(cache_path, initial_url, ui.as_weak());

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
                            let _ = slint::quit_event_loop();
                        }
                        Err(err) => {
                            ui.set_status_title(SharedString::from("Aggiornamento non riuscito"));
                            ui.set_status_detail(SharedString::default());
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

    // Invalid clipboard content never overwrites the last usable link.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let last_link = last_link.clone();
        ui.on_paste_url(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let pasted = arboard::Clipboard::new()
                .ok()
                .and_then(|mut clipboard| clipboard.get_text().ok());
            let Some(pasted) = pasted else {
                ui.set_url_error(SharedString::from("Copia un link MediaFire negli appunti."));
                return;
            };
            let url = pasted.trim();
            if let Err(err) = download::parse_key(url) {
                ui.set_url_error(SharedString::from(err));
                return;
            }

            ui.set_url_text(SharedString::from(url));
            ui.set_url_error(SharedString::default());
            ui.set_banner_error(SharedString::default());
            last_link.remember(url);
            invalidate_selection_if_url_changed(&ui, &state, url);
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

    // This also receives Ctrl+V and pastes from the text field's context menu.
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let last_link = last_link.clone();
        ui.on_url_edited(move |txt| {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_url_error(SharedString::default());
            last_link.remember(txt.as_str());
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
        ui.on_manage_selector(move |action, id, path| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Ok(mut guard) = state.lock() else { return };
            if action == SelectorAction::ToggleSort {
                guard.sort_name_asc = !guard.sort_name_asc;
                ui.set_sort_name_asc(guard.sort_name_asc);
            }
            let Some(cache) = guard.scanned.as_mut() else {
                return;
            };
            let selection = &mut cache.selection;
            let active_filter = filter(&ui);
            match action {
                SelectorAction::ToggleRow => {
                    if let Ok(index) = usize::try_from(id) {
                        selection.toggle_file(index);
                    } else {
                        selection.toggle_folder(path.as_str(), &active_filter);
                    }
                }
                SelectorAction::OpenFolder => selection.open_folder(path.to_string()),
                SelectorAction::GoUp => selection.go_up(),
                SelectorAction::ToggleSelectedOnly => {
                    let current_filter = ui.get_filter_text();
                    let next_filter = selection.toggle_selected_only(current_filter.as_str());
                    ui.set_filter_text(SharedString::from(next_filter));
                }
                SelectorAction::SelectVisible => selection.set_visible(&active_filter, true),
                SelectorAction::DeselectVisible => selection.set_visible(&active_filter, false),
                SelectorAction::ToggleSort => {}
            }
            sync_selector_ui(&ui, &guard);
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

    let result = ui.run();
    drop(ui);
    drop(last_link);
    let _ = link_writer.join();
    result
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
        let Ok(guard) = state.lock() else {
            ui.set_banner_error(SharedString::from(
                "Impossibile leggere la selezione. Download non avviato.",
            ));
            return;
        };
        guard.scanned.as_ref().filter(|c| c.url == url).map(|c| {
            let jobs: Vec<_> = c
                .selection
                .selected_items()
                .map(|i| i.scanned.job.clone())
                .collect();
            (c.key.clone(), jobs)
        })
    };

    if let Some((_, ref jobs)) = selected_jobs {
        if jobs.is_empty() {
            ui.set_banner_error(SharedString::from(
                "Nessun file selezionato. Scegli almeno un file oppure clicca 'Ripristina tutti'.",
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
            let items = files
                .into_iter()
                .map(|scanned| {
                    let path = scanned.rel_path.clone();
                    SelectableFile::new(scanned, path)
                })
                .collect();

            ui.set_is_running(false);
            ui.set_is_indeterminate(false);
            ui.set_is_cancelling(false);
            ui.set_progress_value(0.0);
            ui.set_filter_text(SharedString::default());
            ui.set_sort_name_asc(true);

            if let Ok(mut cur) = state.lock() {
                cur.running = false;
                cur.cancel = None;
                cur.sort_name_asc = true;
                cur.scanned = Some(ScannedCache {
                    url: scanned_url.unwrap_or_default().to_owned(),
                    key,
                    selection: Selection::new(items),
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_cache_roundtrip_and_ignores_empty() {
        let dir =
            std::env::temp_dir().join(format!("mediafire-mini-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("cache.json");

        assert_eq!(load_cached_url_from(&path), None);

        save_cached_url_to(&path, "   ").unwrap();
        assert!(!path.exists());

        let url = "https://www.mediafire.com/folder/r1xp6x0i043wx/Test";
        save_cached_url_to(&path, &format!("  {url}  ")).unwrap();
        assert_eq!(load_cached_url_from(&path), Some(url.to_owned()));

        let url2 = "https://www.mediafire.com/file/abc123/example.zip";
        save_cached_url_to(&path, url2).unwrap();
        save_cached_url_to(&path, "Clipboard non valido").unwrap();
        assert_eq!(load_cached_url_from(&path), Some(url2.to_owned()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
