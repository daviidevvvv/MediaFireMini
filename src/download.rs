use md_downloader::types::{DownloadError, DownloadJob, DownloadProgress};
use md_downloader::MediafireDownloader;
use std::{
    collections::VecDeque,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const CONCURRENT_DOWNLOADS: usize = 3;
const MAX_RETRIES: u64 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(2);
const PROGRESS_THROTTLE: Duration = Duration::from_millis(120);

#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            self.notify.notified().await;
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub job: DownloadJob,
    pub rel_path: String,
}

#[derive(Debug, Clone)]
pub enum Update {
    Scanning(String),
    ScanReady {
        key: String,
        files: Vec<ScannedFile>,
    },
    Downloading {
        completed_files: usize,
        total_files: usize,
        downloaded_bytes: u64,
        total_bytes: u64,
        current_file: String,
    },
    Zipping {
        current: usize,
        total: usize,
        current_file: String,
    },
    Done {
        folder: PathBuf,
        zip: Option<PathBuf>,
        files: usize,
        bytes: u64,
    },
    Cancelled {
        folder: Option<PathBuf>,
        has_files: bool,
    },
    Error(String),
}

pub fn parse_key(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    let rest = url
        .strip_prefix("https://")
        .ok_or("Usa un link MediaFire HTTPS valido.")?;
    let (host, path) = rest.split_once('/').ok_or("Link MediaFire incompleto.")?;
    if host != "www.mediafire.com" && host != "mediafire.com" {
        return Err("Sono accettati solo link mediafire.com.".into());
    }
    let mut fields = path.split('/');
    let kind = fields.next().unwrap_or("");
    if kind != "folder" && kind != "file" {
        return Err("Inserisci un link MediaFire a una cartella o a un file.".into());
    }
    let key = fields
        .next()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if key.is_empty() || key.len() > 80 || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Identificativo MediaFire non valido.".into());
    }
    Ok(key.to_owned())
}

pub fn new_folder(base: &Path, key: &str) -> PathBuf {
    let stem = format!("MediaFire-{}", key);
    let preferred = base.join(&stem);
    if !preferred.exists() && !preferred.with_extension("zip").exists() {
        return preferred;
    }
    for index in 2..10_000 {
        let candidate = base.join(format!("{}-{}", stem, index));
        if !candidate.exists() && !candidate.with_extension("zip").exists() {
            return candidate;
        }
    }
    base.join(format!("{}-new", stem))
}

pub fn list_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                dirs.push(entry.path());
            } else if ty.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn stats(root: &Path) -> (usize, u64) {
    let entries = list_files(root).unwrap_or_default();
    let size = entries
        .iter()
        .filter_map(|p| p.metadata().ok().map(|m| m.len()))
        .sum();
    (entries.len(), size)
}

fn zip_folder(
    root: &Path,
    archive_path: &Path,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(usize, usize, &str),
) -> Result<(), String> {
    let partial = archive_path.with_extension("zip.part");
    let result = (|| -> Result<(), String> {
        let archive_file = File::create(&partial).map_err(|e| e.to_string())?;
        let mut archive = ZipWriter::new(archive_file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        let entries = list_files(root).map_err(|e| e.to_string())?;
        let total = entries.len();
        for (idx, file) in entries.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err("Annullato.".into());
            }
            let relative = file.strip_prefix(root).map_err(|e| e.to_string())?;
            let name = relative.to_string_lossy().replace('\\', "/");
            on_progress(idx + 1, total, &name);
            archive
                .start_file(name, options)
                .map_err(|e| e.to_string())?;
            let mut input = File::open(file).map_err(|e| e.to_string())?;
            io::copy(&mut input, &mut archive).map_err(|e| e.to_string())?;
        }
        archive.finish().map_err(|e| e.to_string())?;
        fs::rename(&partial, archive_path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn build_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .map_err(|err| format!("Errore inizializzazione runtime: {err}"))
}

/// Scans a MediaFire folder or file URL without creating folders or downloading files.
pub fn scan(url: &str, cancel: CancelToken, mut inform: impl FnMut(Update) + Send + 'static) {
    let key = match parse_key(url) {
        Ok(key) => key,
        Err(err) => {
            inform(Update::Error(err));
            return;
        }
    };
    let rt = match build_runtime() {
        Ok(rt) => rt,
        Err(err) => {
            inform(Update::Error(err));
            return;
        }
    };
    let inform = Arc::new(Mutex::new(inform));
    let res = rt.block_on(scan_async(url.to_owned(), cancel.clone(), inform.clone()));

    if cancel.is_cancelled() {
        if let Ok(mut cb) = inform.lock() {
            cb(Update::Cancelled {
                folder: None,
                has_files: false,
            });
        };
        return;
    }

    match res {
        Ok(files) => {
            if let Ok(mut cb) = inform.lock() {
                cb(Update::ScanReady { key, files });
            };
        }
        Err(err) => {
            if let Ok(mut cb) = inform.lock() {
                cb(Update::Error(err));
            };
        }
    }
}

/// Downloads all files from a MediaFire URL directly (scans first, then downloads).
pub fn run(
    url: &str,
    base: &Path,
    make_zip: bool,
    cancel: CancelToken,
    mut inform: impl FnMut(Update) + Send + 'static,
) {
    let key = match parse_key(url) {
        Ok(key) => key,
        Err(err) => {
            inform(Update::Error(err));
            return;
        }
    };
    if !base.is_dir() {
        inform(Update::Error(
            "La cartella di destinazione non esiste.".into(),
        ));
        return;
    }

    let rt = match build_runtime() {
        Ok(rt) => rt,
        Err(err) => {
            inform(Update::Error(err));
            return;
        }
    };

    let inform_arc = Arc::new(Mutex::new(inform));
    let scanned = rt.block_on(scan_async(
        url.to_owned(),
        cancel.clone(),
        inform_arc.clone(),
    ));

    if cancel.is_cancelled() {
        if let Ok(mut cb) = inform_arc.lock() {
            cb(Update::Cancelled {
                folder: None,
                has_files: false,
            });
        };
        return;
    }

    let jobs: Vec<DownloadJob> = match scanned {
        Ok(items) => items.into_iter().map(|item| item.job).collect(),
        Err(err) => {
            if let Ok(mut cb) = inform_arc.lock() {
                cb(Update::Error(err));
            };
            return;
        }
    };

    execute_jobs(&rt, &key, jobs, base, make_zip, cancel, inform_arc);
}

/// Downloads a user-selected subset of pre-scanned jobs into a new destination folder.
pub fn run_selected(
    key: &str,
    jobs: Vec<DownloadJob>,
    base: &Path,
    make_zip: bool,
    cancel: CancelToken,
    mut inform: impl FnMut(Update) + Send + 'static,
) {
    if jobs.is_empty() {
        inform(Update::Error(
            "Seleziona almeno un file da scaricare.".into(),
        ));
        return;
    }
    if !base.is_dir() {
        inform(Update::Error(
            "La cartella di destinazione non esiste.".into(),
        ));
        return;
    }
    let rt = match build_runtime() {
        Ok(rt) => rt,
        Err(err) => {
            inform(Update::Error(err));
            return;
        }
    };
    let inform_arc = Arc::new(Mutex::new(inform));
    execute_jobs(&rt, key, jobs, base, make_zip, cancel, inform_arc);
}

fn execute_jobs<F>(
    rt: &tokio::runtime::Runtime,
    key: &str,
    mut jobs: Vec<DownloadJob>,
    base: &Path,
    make_zip: bool,
    cancel: CancelToken,
    inform: Arc<Mutex<F>>,
) where
    F: FnMut(Update) + Send + 'static,
{
    let folder = new_folder(base, key);
    if let Err(err) = fs::create_dir(&folder) {
        if let Ok(mut cb) = inform.lock() {
            cb(Update::Error(format!("Impossibile creare cartella: {err}")));
        };
        return;
    }

    // Re-root relative job paths into the freshly created destination folder
    // and prioritize larger files first (matching mdrs default ordering).
    for job in &mut jobs {
        job.path = folder.join(&job.path);
    }
    jobs.sort_by_key(|a| std::cmp::Reverse(a.size));

    let download_result = rt.block_on(download_jobs_async(jobs, cancel.clone(), inform.clone()));

    if cancel.is_cancelled() {
        let (files, _) = stats(&folder);
        if files == 0 {
            let _ = fs::remove_dir_all(&folder);
        }
        if let Ok(mut cb) = inform.lock() {
            cb(Update::Cancelled {
                folder: (files > 0).then_some(folder),
                has_files: files > 0,
            });
        };
        return;
    }

    if let Err(err) = download_result {
        let (files, _) = stats(&folder);
        if files == 0 {
            let _ = fs::remove_dir_all(&folder);
        }
        if let Ok(mut cb) = inform.lock() {
            cb(Update::Error(err));
        };
        return;
    }

    let (files, bytes) = stats(&folder);
    if files == 0 {
        let _ = fs::remove_dir_all(&folder);
        if let Ok(mut cb) = inform.lock() {
            cb(Update::Error(
                "Nessun file scaricato dal link indicato.".into(),
            ));
        };
        return;
    }

    let archive = if make_zip {
        let output_zip = folder.with_extension("zip");
        let zip_res = zip_folder(&folder, &output_zip, &cancel, |current, total, name| {
            if let Ok(mut cb) = inform.lock() {
                cb(Update::Zipping {
                    current,
                    total,
                    current_file: name.to_owned(),
                });
            }
        });
        if let Err(err) = zip_res {
            if cancel.is_cancelled() {
                if let Ok(mut cb) = inform.lock() {
                    cb(Update::Cancelled {
                        folder: Some(folder),
                        has_files: true,
                    });
                };
            } else if let Ok(mut cb) = inform.lock() {
                cb(Update::Error(format!(
                    "Errore creazione ZIP: {err}. I file originali sono conservati in {}",
                    folder.display()
                )));
            }
            return;
        }
        Some(output_zip)
    } else {
        None
    };

    if let Ok(mut cb) = inform.lock() {
        cb(Update::Done {
            folder,
            zip: archive,
            files,
            bytes,
        });
    };
}

async fn scan_async<F>(
    url: String,
    cancel: CancelToken,
    inform: Arc<Mutex<F>>,
) -> Result<Vec<ScannedFile>, String>
where
    F: FnMut(Update) + Send + 'static,
{
    if let Ok(mut cb) = inform.lock() {
        cb(Update::Scanning("Connessione a MediaFire...".into()));
    }

    let downloader = MediafireDownloader::new(MAX_RETRIES)
        .map_err(|e| format!("Inizializzazione downloader fallita: {e}"))?;

    let urls = vec![url];
    let scan_inform = inform.clone();
    let mut heap = tokio::select! {
        _ = cancel.cancelled() => return Ok(Vec::new()),
        res = downloader.get_download_jobs_with_progress(&urls, PathBuf::new(), |name| {
            if let Ok(mut cb) = scan_inform.lock() {
                cb(Update::Scanning(format!("Scansione cartella: {name}")));
            }
        }) => res.map_err(|e| format!("Errore scansione MediaFire: {e}"))?,
    };

    if cancel.is_cancelled() {
        return Ok(Vec::new());
    }

    let mut files = Vec::with_capacity(heap.len());
    while let Some(job) = heap.pop() {
        let rel_path = job.path.to_string_lossy().replace('\\', "/");
        files.push(ScannedFile { job, rel_path });
    }

    if files.is_empty() {
        return Err("La cartella o il file MediaFire non contiene elementi scaricabili.".into());
    }

    files.sort_by_key(|a| a.rel_path.to_lowercase());
    Ok(files)
}

async fn download_jobs_async<F>(
    jobs: Vec<DownloadJob>,
    cancel: CancelToken,
    inform: Arc<Mutex<F>>,
) -> Result<(), String>
where
    F: FnMut(Update) + Send + 'static,
{
    let total_files = jobs.len();
    let total_bytes: u64 = jobs.iter().map(|j| j.size).fold(0u64, u64::saturating_add);
    let ordered_jobs: VecDeque<(usize, DownloadJob)> = jobs.into_iter().enumerate().collect();

    let per_job_bytes: Arc<Vec<AtomicU64>> =
        Arc::new((0..total_files).map(|_| AtomicU64::new(0)).collect());
    let completed_files = Arc::new(AtomicUsize::new(0));
    let last_emit = Arc::new(Mutex::new(Instant::now() - PROGRESS_THROTTLE));
    let queue = Arc::new(tokio::sync::Mutex::new(ordered_jobs));
    let failures: Arc<tokio::sync::Mutex<Vec<(String, DownloadError)>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    if let Ok(mut cb) = inform.lock() {
        cb(Update::Downloading {
            completed_files: 0,
            total_files,
            downloaded_bytes: 0,
            total_bytes,
            current_file: "Avvio download...".into(),
        });
    }

    let worker_count = CONCURRENT_DOWNLOADS.min(total_files);
    let mut handles = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let queue = queue.clone();
        let per_job_bytes = per_job_bytes.clone();
        let completed_files = completed_files.clone();
        let last_emit = last_emit.clone();
        let failures = failures.clone();
        let inform = inform.clone();
        let cancel = cancel.clone();

        handles.push(tokio::spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let next = {
                    let mut q = queue.lock().await;
                    q.pop_front()
                };
                let Some((job_idx, job)) = next else {
                    break;
                };

                let res = download_job_with_retries(
                    job_idx,
                    &job,
                    total_files,
                    total_bytes,
                    &per_job_bytes,
                    &completed_files,
                    &last_emit,
                    &inform,
                    &cancel,
                )
                .await;

                if cancel.is_cancelled() {
                    break;
                }

                match res {
                    Ok(()) => {
                        per_job_bytes[job_idx].store(job.size, Ordering::Relaxed);
                        let done = completed_files.fetch_add(1, Ordering::Relaxed) + 1;
                        emit_progress(
                            done,
                            total_files,
                            total_bytes,
                            &per_job_bytes,
                            &job.filename,
                            &last_emit,
                            &inform,
                            true,
                        );
                    }
                    Err(err) => {
                        failures.lock().await.push((job.filename.clone(), err));
                    }
                }
            }
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    if cancel.is_cancelled() {
        return Ok(());
    }

    let failed = failures.lock().await;
    if !failed.is_empty() {
        let preview: Vec<String> = failed
            .iter()
            .take(3)
            .map(|(name, err)| format!("{name}: {err}"))
            .collect();
        let extra = if failed.len() > 3 {
            format!(" (+{} altri)", failed.len() - 3)
        } else {
            String::new()
        };
        return Err(format!(
            "Errore nel download di {} file: {}{}",
            failed.len(),
            preview.join("; "),
            extra
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_job_with_retries<F>(
    job_idx: usize,
    job: &DownloadJob,
    total_files: usize,
    total_bytes: u64,
    per_job_bytes: &Arc<Vec<AtomicU64>>,
    completed_files: &Arc<AtomicUsize>,
    last_emit: &Arc<Mutex<Instant>>,
    inform: &Arc<Mutex<F>>,
    cancel: &CancelToken,
) -> Result<(), DownloadError>
where
    F: FnMut(Update) + Send + 'static,
{
    let mut tries = MAX_RETRIES;
    let mut last_err = None;

    while tries > 0 {
        if cancel.is_cancelled() {
            return Ok(());
        }

        let filename = job.filename.clone();
        let job_size = job.size;
        let result = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            res = job.download_with_progress(|status| {
                match status {
                    DownloadProgress::Downloading(partial, _) => {
                        per_job_bytes[job_idx].store(partial.min(job_size), Ordering::Relaxed);
                        let done = completed_files.load(Ordering::Relaxed);
                        emit_progress(
                            done,
                            total_files,
                            total_bytes,
                            per_job_bytes,
                            &filename,
                            last_emit,
                            inform,
                            false,
                        );
                    }
                    DownloadProgress::GettingLink | DownloadProgress::TryResuming | DownloadProgress::CheckingHash => {
                        let done = completed_files.load(Ordering::Relaxed);
                        emit_progress(
                            done,
                            total_files,
                            total_bytes,
                            per_job_bytes,
                            &filename,
                            last_emit,
                            inform,
                            false,
                        );
                    }
                    DownloadProgress::Done => {
                        per_job_bytes[job_idx].store(job_size, Ordering::Relaxed);
                    }
                }
            }) => res,
        };

        match result {
            Ok(()) => return Ok(()),
            Err(err) => {
                tries -= 1;
                last_err = Some(err);
                per_job_bytes[job_idx].store(0, Ordering::Relaxed);
                if tries > 0 {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(RETRY_DELAY) => {}
                    }
                }
            }
        }
    }

    Err(last_err.unwrap_or(DownloadError::LinkExtractionError))
}

#[allow(clippy::too_many_arguments)]
fn emit_progress<F>(
    completed_files: usize,
    total_files: usize,
    total_bytes: u64,
    per_job_bytes: &[AtomicU64],
    current_file: &str,
    last_emit: &Mutex<Instant>,
    inform: &Mutex<F>,
    force: bool,
) where
    F: FnMut(Update) + Send + 'static,
{
    if !force {
        if let Ok(mut last) = last_emit.lock() {
            if last.elapsed() < PROGRESS_THROTTLE {
                return;
            }
            *last = Instant::now();
        }
    } else if let Ok(mut last) = last_emit.lock() {
        *last = Instant::now();
    }

    let downloaded_bytes: u64 = per_job_bytes
        .iter()
        .map(|a| a.load(Ordering::Relaxed))
        .sum();

    if let Ok(mut cb) = inform.lock() {
        cb(Update::Downloading {
            completed_files,
            total_files,
            downloaded_bytes,
            total_bytes,
            current_file: current_file.to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    #[test]
    fn only_mediafire_urls() {
        assert_eq!(
            parse_key("https://www.mediafire.com/folder/r1xp6x0i043wx").unwrap(),
            "r1xp6x0i043wx"
        );
        assert_eq!(
            parse_key("https://mediafire.com/file/abc123/example.txt").unwrap(),
            "abc123"
        );
        assert!(parse_key("https://example.com/folder/r1xp6x0i043wx").is_err());
        assert!(parse_key("http://www.mediafire.com/folder/abc").is_err());
        assert!(parse_key("https://www.mediafire.com/folder/../../bad").is_err());
    }

    #[test]
    fn zip_preserves_subfolders() {
        let root =
            std::env::temp_dir().join(format!("mediafire-mini-zip-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("v12/addon")).unwrap();
        fs::write(root.join("v12/changelog.txt"), b"123").unwrap();
        fs::write(root.join("v12/addon/file.txt"), b"hello").unwrap();
        let output = root.with_extension("zip");
        zip_folder(&root, &output, &CancelToken::new(), |_, _, _| {}).unwrap();
        let mut z = ZipArchive::new(File::open(&output).unwrap()).unwrap();
        assert_eq!(z.len(), 2);
        let mut s = String::new();
        z.by_name("v12/addon/file.txt")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "hello");
        drop(z);
        fs::remove_file(output).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
