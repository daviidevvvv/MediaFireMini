#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod download;
mod updater;

use nwg;
use std::{
    cell::RefCell,
    path::PathBuf,
    process::Command,
    rc::Rc,
    sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc},
    thread,
};
use download::Update;
use updater::{Available, Kind};

enum UiEvent {
    Download(Update),
    Checked(Vec<Available>, Vec<String>),
    Updating(String),
    Updated(String),
    GuiStaged(PathBuf, String),
    UpdateError(String),
}

#[derive(Default)]
struct AppState {
    running: bool,
    updating: bool,
    checking: bool,
    cancel: Option<Arc<AtomicBool>>,
    last_folder: Option<PathBuf>,
    updates: Vec<Available>,
}

fn update_status(updates: &[Available]) -> String {
    if updates.is_empty() {
        let v = updater::app_dir().ok().and_then(|p| updater::engine_version(&p).ok())
            .unwrap_or_else(|| "sconosciuta".into());
        return format!("Aggiornato. MDRS {v} | GUI v{}", env!("CARGO_PKG_VERSION"));
    }
    let versions: Vec<String> = updates.iter().map(|v| {
        let name = if v.kind == Kind::Engine { "MDRS" } else { "GUI" };
        format!("{name} {}", v.version)
    }).collect();
    format!("Nuova release: {}", versions.join(", "))
}

fn main() {
    nwg::init().expect("Impossibile inizializzare Windows");
    nwg::Font::set_global_family("Segoe UI").expect("Font di sistema mancante");

    let mut window = nwg::Window::default();
    nwg::Window::builder()
        .size((640, 401))
        .position((250, 140))
        .title("MediaFire Mini  |  Rust + MDRS")
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::VISIBLE)
        .build(&mut window).unwrap();

    let mut title = nwg::Label::default();
    nwg::Label::builder()
        .text(&format!("MediaFire Mini v{}", env!("CARGO_PKG_VERSION")))
        .position((20, 16)).size((590, 27)).parent(&window)
        .build(&mut title).unwrap();

    let mut subtitle = nwg::Label::default();
    nwg::Label::builder()
        .text("Incolla il link, scarica la cartella, crea un unico ZIP.")
        .position((20, 45)).size((585, 19)).parent(&window)
        .build(&mut subtitle).unwrap();

    let mut link_label = nwg::Label::default();
    nwg::Label::builder().text("Link MediaFire")
        .position((20, 79)).size((585, 18)).parent(&window)
        .build(&mut link_label).unwrap();

    let mut link = nwg::TextInput::default();
    nwg::TextInput::builder()
        .text("https://www.mediafire.com/folder/r1xp6x0i043wx")
        .position((20, 99)).size((590, 28)).parent(&window)
        .build(&mut link).unwrap();

    let mut folder_label = nwg::Label::default();
    nwg::Label::builder().text("Salva in")
        .position((20, 141)).size((585, 18)).parent(&window)
        .build(&mut folder_label).unwrap();

    let default_path = std::env::var_os("USERPROFILE")
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads");
    let mut folder = nwg::TextInput::default();
    nwg::TextInput::builder()
        .text(&default_path.to_string_lossy())
        .position((20, 161)).size((482, 28)).parent(&window)
        .build(&mut folder).unwrap();

    let mut browse = nwg::Button::default();
    nwg::Button::builder().text("Sfoglia...")
        .position((514, 161)).size((96, 28)).parent(&window)
        .build(&mut browse).unwrap();

    let mut folder_picker = nwg::FileDialog::default();
    nwg::FileDialog::builder()
        .title("Scegli la cartella di destinazione")
        .action(nwg::FileDialogAction::OpenDirectory)
        .build(&mut folder_picker).unwrap();

    let mut checkbox = nwg::CheckBox::default();
    nwg::CheckBox::builder()
        .text("Crea anche un archivio ZIP unico (conserva gli originali)")
        .check_state(nwg::CheckBoxState::Checked)
        .position((20, 207)).size((590, 22)).parent(&window)
        .build(&mut checkbox).unwrap();

    let mut start = nwg::Button::default();
    nwg::Button::builder().text("Scarica cartella")
        .position((20, 243)).size((234, 35)).parent(&window)
        .build(&mut start).unwrap();

    let mut stop = nwg::Button::default();
    nwg::Button::builder().text("Annulla").enabled(false)
        .position((265, 243)).size((108, 35)).parent(&window)
        .build(&mut stop).unwrap();

    let mut open = nwg::Button::default();
    nwg::Button::builder().text("Apri destinazione").enabled(false)
        .position((385, 243)).size((225, 35)).parent(&window)
        .build(&mut open).unwrap();

    let mut update_label = nwg::Label::default();
    nwg::Label::builder().text("Controllo aggiornamenti GitHub...")
        .position((20, 293)).size((395, 21)).parent(&window)
        .build(&mut update_label).unwrap();

    let mut check_button = nwg::Button::default();
    nwg::Button::builder().text("Verifica").enabled(false)
        .position((417, 287)).size((86, 28)).parent(&window)
        .build(&mut check_button).unwrap();

    let mut update_button = nwg::Button::default();
    nwg::Button::builder().text("Aggiorna").enabled(false)
        .position((511, 287)).size((99, 28)).parent(&window)
        .build(&mut update_button).unwrap();

    let mut status = nwg::Label::default();
    nwg::Label::builder().text("Pronto. I download non modificano i file del gioco.")
        .position((20, 330)).size((590, 19)).parent(&window)
        .build(&mut status).unwrap();

    let mut progress = nwg::ProgressBar::default();
    nwg::ProgressBar::builder().range(0..100)
        .flags(nwg::ProgressBarFlags::VISIBLE | nwg::ProgressBarFlags::MARQUEE)
        .position((20, 354)).size((590, 12)).parent(&window)
        .build(&mut progress).unwrap();
    progress.set_marquee(false, 60);

    let mut notice = nwg::Notice::default();
    nwg::Notice::builder().parent(&window).build(&mut notice).unwrap();

    let (sender, receiver) = mpsc::channel::<UiEvent>();
    let receiver = Rc::new(RefCell::new(receiver));
    let state = Rc::new(RefCell::new(AppState::default()));
    state.borrow_mut().checking = true;
    let ui_window = Rc::new(window);
    let handler_window = ui_window.clone();
    let notifier = notice.sender();

    // Read-only startup check, off UI thread. No downloads until user chooses Update.
    {
        let tx = sender.clone();
        let alert = notifier.clone();
        thread::spawn(move || {
            let (available, errors) = match updater::app_dir() {
                Ok(path) => updater::check(&path),
                Err(e) => (Vec::new(), vec![e]),
            };
            let _ = tx.send(UiEvent::Checked(available, errors));
            alert.notice();
        });
    }

    let handler = nwg::full_bind_event_handler(&ui_window.handle, move |event, _data, handle| {
        use nwg::Event as E;
        match event {
            E::OnWindowClose if handle == handler_window.handle => {
                if state.borrow().running || state.borrow().updating {
                    nwg::modal_info_message(&handler_window.handle, "Operazione in corso",
                        "Completa o annulla l'operazione prima di chiudere.");
                } else { nwg::stop_thread_dispatch(); }
            }
            E::OnButtonClick if handle == browse.handle => {
                if folder_picker.run(Some(&*handler_window)) {
                    if let Ok(selected) = folder_picker.get_selected_item() {
                        folder.set_text(&selected.to_string_lossy());
                    }
                }
            }
            E::OnButtonClick if handle == start.handle => {
                if state.borrow().running || state.borrow().updating { return; }
                let url = link.text().trim().to_owned();
                if let Err(err) = download::parse_key(&url) {
                    nwg::modal_error_message(&handler_window.handle, "Link non valido", &err);
                    return;
                }
                let output = PathBuf::from(folder.text().trim());
                if !output.is_dir() {
                    nwg::modal_error_message(&handler_window.handle, "Cartella non valida",
                        "Seleziona una cartella di destinazione esistente.");
                    return;
                }
                let mdrs = std::env::current_exe().unwrap_or_default().with_file_name("mdrs.exe");
                if !mdrs.is_file() {
                    nwg::modal_error_message(&handler_window.handle, "MDRS mancante",
                        "mdrs.exe deve trovarsi accanto a MediaFireMini.exe.");
                    return;
                }
                let zip = checkbox.check_state() == nwg::CheckBoxState::Checked;
                let cancelled = Arc::new(AtomicBool::new(false));
                {
                    let mut current = state.borrow_mut();
                    current.running = true;
                    current.cancel = Some(cancelled.clone());
                    current.last_folder = None;
                }
                link.set_enabled(false); folder.set_enabled(false);
                browse.set_enabled(false); checkbox.set_enabled(false);
                start.set_enabled(false); stop.set_enabled(true);
                open.set_enabled(false); update_button.set_enabled(false);
                check_button.set_enabled(false);
                progress.set_pos(0); progress.set_marquee(true, 45);
                status.set_text("Inizio download...");
                let tx = sender.clone();
                let alert = notifier.clone();
                thread::spawn(move || {
                    download::run(&mdrs, &url, &output, zip, cancelled, move |msg| {
                        let _ = tx.send(UiEvent::Download(msg));
                        alert.notice();
                    });
                });
            }
            E::OnButtonClick if handle == stop.handle => {
                if let Some(flag) = state.borrow().cancel.as_ref() {
                    flag.store(true, Ordering::Relaxed);
                    status.set_text("Annullamento in corso...");
                    stop.set_enabled(false);
                }
            }
            E::OnButtonClick if handle == open.handle => {
                if let Some(last) = state.borrow().last_folder.as_ref() {
                    let _ = Command::new("explorer.exe").arg(last).spawn();
                }
            }
            E::OnButtonClick if handle == check_button.handle => {
                if state.borrow().running || state.borrow().updating { return; }
                state.borrow_mut().checking = true;
                update_label.set_text("Controllo aggiornamenti GitHub...");
                check_button.set_enabled(false);
                update_button.set_enabled(false);
                let tx = sender.clone();
                let alert = notifier.clone();
                thread::spawn(move || {
                    let (found, errors) = match updater::app_dir() {
                        Ok(path) => updater::check(&path),
                        Err(e) => (Vec::new(), vec![e]),
                    };
                    let _ = tx.send(UiEvent::Checked(found, errors));
                    alert.notice();
                });
            }
            E::OnButtonClick if handle == update_button.handle => {
                if state.borrow().running || state.borrow().updating || state.borrow().checking {
                    return;
                }
                let packages = state.borrow().updates.clone();
                if packages.is_empty() { return; }
                state.borrow_mut().updating = true;
                link.set_enabled(false); folder.set_enabled(false); browse.set_enabled(false);
                checkbox.set_enabled(false); start.set_enabled(false);
                stop.set_enabled(false); open.set_enabled(false);
                check_button.set_enabled(false); update_button.set_enabled(false);
                update_label.set_text("Aggiornamento in corso...");
                progress.set_pos(0); progress.set_marquee(true, 45);
                let tx = sender.clone();
                let alert = notifier.clone();
                thread::spawn(move || {
                    let folder = match updater::app_dir() {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = tx.send(UiEvent::UpdateError(e));
                            alert.notice(); return;
                        }
                    };
                    for package in &packages {
                        let name = if package.kind == Kind::Engine { "MDRS" } else { "GUI" };
                        let _ = tx.send(UiEvent::Updating(format!(
                            "Scarico {name} {} da GitHub...", package.version)));
                        alert.notice();
                        match package.kind {
                            Kind::Engine => match updater::install_engine(&folder, package) {
                                Ok(version) => {
                                    let _ = tx.send(UiEvent::Updating(format!(
                                        "MDRS aggiornato alla {version}. Backup precedente conservato.")));
                                    alert.notice();
                                }
                                Err(err) => { let _ = tx.send(UiEvent::UpdateError(err)); alert.notice(); return; }
                            },
                            Kind::Gui => match updater::stage_gui(&folder, package) {
                                Ok(staged) => {
                                    let _ = tx.send(UiEvent::GuiStaged(staged, package.digest.clone()));
                                    alert.notice(); return;
                                }
                                Err(err) => { let _ = tx.send(UiEvent::UpdateError(err)); alert.notice(); return; }
                            },
                        }
                    }
                    let _ = tx.send(UiEvent::Updated("Aggiornamento riuscito.".into()));
                    alert.notice();
                });
            }
            E::OnNotice if handle == notice.handle => {
                while let Ok(message) = receiver.borrow().try_recv() {
                    match message {
                        UiEvent::Checked(available, errors) => {
                            let mut current = state.borrow_mut();
                            current.checking = false;
                            current.updates = available;
                            let is_busy = current.running || current.updating;
                            update_button.set_enabled(!is_busy && !current.updates.is_empty());
                            check_button.set_enabled(!is_busy);
                            let text = if current.updates.is_empty() && !errors.is_empty() {
                                "Verifica GitHub non riuscita. Premi Verifica.".into()
                            } else {
                                update_status(&current.updates)
                            };
                            let notify = !current.updates.is_empty() && !is_busy;
                            let notes = current.updates.first().map(|v| v.description.clone()).unwrap_or_default();
                            update_label.set_text(&text);
                            drop(current);
                            if notify {
                                nwg::modal_info_message(&handler_window.handle, "Nuovo aggiornamento",
                                    &format!("{text}\n{notes}\nPremi 'Aggiorna' per applicarlo.\nI vecchi file vengono conservati."));
                            }
                        }
                        UiEvent::Updating(text) => update_label.set_text(&text),
                        UiEvent::Updated(text) => {
                            {
                                let mut cur = state.borrow_mut();
                                cur.updating = false;
                                cur.updates.clear();
                            }
                            progress.set_marquee(false, 45); progress.set_pos(100);
                            link.set_enabled(true); folder.set_enabled(true);
                            browse.set_enabled(true); checkbox.set_enabled(true);
                            start.set_enabled(true); check_button.set_enabled(true);
                            update_button.set_enabled(false);
                            update_label.set_text(&update_status(&[]));
                            status.set_text(&text);
                        }
                        UiEvent::GuiStaged(path, hash) => {
                            let folder = updater::app_dir();
                            let result = folder.and_then(|dir| updater::start_gui_replacement(&dir, &path, &hash));
                            match result {
                                Ok(()) => {
                                    update_label.set_text("Riavvio con la versione aggiornata...");
                                    nwg::stop_thread_dispatch();
                                }
                                Err(err) => {
                                    let _ = sender.send(UiEvent::UpdateError(err));
                                    notifier.notice();
                                }
                            }
                        }
                        UiEvent::UpdateError(err) => {
                            state.borrow_mut().updating = false;
                            progress.set_marquee(false, 45); progress.set_pos(0);
                            link.set_enabled(true); folder.set_enabled(true);
                            browse.set_enabled(true); checkbox.set_enabled(true);
                            start.set_enabled(true); check_button.set_enabled(true);
                            update_button.set_enabled(!state.borrow().updates.is_empty());
                            update_label.set_text("Aggiornamento non riuscito.");
                            nwg::modal_error_message(&handler_window.handle, "Aggiornamento", &err);
                        }
                        UiEvent::Download(Update::Info(text)) => status.set_text(&text),
                        UiEvent::Download(Update::Done { folder: downloaded, zip, files, bytes }) => {
                            {
                                let mut cur = state.borrow_mut();
                                cur.running = false; cur.cancel = None;
                                cur.last_folder = Some(downloaded);
                            }
                            progress.set_marquee(false, 45); progress.set_pos(100);
                            let suffix = if zip.is_some() { " | ZIP pronto" } else { "" };
                            status.set_text(&format!("Completato: {} file, {:.1} MiB{}",
                                files, bytes as f64 / 1_048_576.0, suffix));
                            link.set_enabled(true); folder.set_enabled(true);
                            browse.set_enabled(true); checkbox.set_enabled(true);
                            start.set_enabled(true); stop.set_enabled(false); open.set_enabled(true);
                            check_button.set_enabled(!state.borrow().checking);
                            update_button.set_enabled(!state.borrow().updates.is_empty() && !state.borrow().checking);
                        }
                        UiEvent::Download(Update::Cancelled(downloaded)) => {
                            let mut cur = state.borrow_mut();
                            cur.running = false; cur.cancel = None;
                            cur.last_folder = Some(downloaded);
                            drop(cur);
                            progress.set_marquee(false, 45); progress.set_pos(0);
                            status.set_text("Annullato: i file parziali sono conservati.");
                            link.set_enabled(true); folder.set_enabled(true);
                            browse.set_enabled(true); checkbox.set_enabled(true);
                            start.set_enabled(true); stop.set_enabled(false); open.set_enabled(true);
                            check_button.set_enabled(!state.borrow().checking);
                            update_button.set_enabled(!state.borrow().updates.is_empty() && !state.borrow().checking);
                        }
                        UiEvent::Download(Update::Error(err)) => {
                            state.borrow_mut().running = false;
                            state.borrow_mut().cancel = None;
                            progress.set_marquee(false, 45); progress.set_pos(0);
                            status.set_text("Download fallito. Controlla il log.");
                            link.set_enabled(true); folder.set_enabled(true);
                            browse.set_enabled(true); checkbox.set_enabled(true);
                            start.set_enabled(true); stop.set_enabled(false);
                            check_button.set_enabled(!state.borrow().checking);
                            update_button.set_enabled(!state.borrow().updates.is_empty() && !state.borrow().checking);
                            nwg::modal_error_message(&handler_window.handle, "Errore download", &err);
                        }
                    }
                }
            }
            _ => {}
        }
    });
    nwg::dispatch_thread_events();
    nwg::unbind_event_handler(&handler);
}
