use std::{
    fs::{self, File},
    io::{self, Read},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::{AtomicBool, Ordering}, Arc},
    thread,
    time::Duration,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug)]
pub enum Update {
    Info(String),
    Done { folder: PathBuf, zip: Option<PathBuf>, files: usize, bytes: u64 },
    Cancelled(PathBuf),
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
    let key = fields.next().unwrap_or("").split(['?', '#']).next().unwrap_or("");
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
            if ty.is_dir() { dirs.push(entry.path()); }
            else if ty.is_file() { files.push(entry.path()); }
        }
    }
    files.sort();
    Ok(files)
}

fn stats(root: &Path) -> (usize, u64) {
    let entries = list_files(root).unwrap_or_default();
    let size = entries.iter().filter_map(|p| p.metadata().ok().map(|m| m.len())).sum();
    (entries.len(), size)
}

fn zip_folder(root: &Path, archive_path: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let partial = archive_path.with_extension("zip.part");
    let result = (|| -> Result<(), String> {
        let archive_file = File::create(&partial).map_err(|e| e.to_string())?;
        let mut archive = ZipWriter::new(archive_file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        let entries = list_files(root).map_err(|e| e.to_string())?;
        for file in &entries {
            if cancel.load(Ordering::Relaxed) { return Err("Annullato.".into()); }
            let relative = file.strip_prefix(root).map_err(|e| e.to_string())?;
            let name = relative.to_string_lossy().replace('\\', "/");
            archive.start_file(name, options).map_err(|e| e.to_string())?;
            let mut input = File::open(file).map_err(|e| e.to_string())?;
            io::copy(&mut input, &mut archive).map_err(|e| e.to_string())?;
        }
        archive.finish().map_err(|e| e.to_string())?;
        fs::rename(&partial, archive_path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&partial); }
    result
}

pub fn run(
    mdrs: &Path,
    url: &str,
    base: &Path,
    make_zip: bool,
    cancelled: Arc<AtomicBool>,
    mut inform: impl FnMut(Update),
) {
    let key = match parse_key(url) {
        Ok(key) => key,
        Err(err) => { inform(Update::Error(err)); return; }
    };
    if !mdrs.is_file() {
        inform(Update::Error("mdrs.exe non trovato accanto a MediaFireMini.exe.".into()));
        return;
    }
    if !base.is_dir() {
        inform(Update::Error("La cartella di destinazione non esiste.".into()));
        return;
    }
    let folder = new_folder(base, &key);
    if let Err(err) = fs::create_dir(&folder) {
        inform(Update::Error(format!("Impossibile creare cartella: {err}")));
        return;
    }
    let log_path = folder.with_extension("log");
    let log = match File::create(&log_path) {
        Ok(f) => f,
        Err(err) => { inform(Update::Error(err.to_string())); return; }
    };
    let stderr_log = match log.try_clone() {
        Ok(f) => f,
        Err(err) => { inform(Update::Error(err.to_string())); return; }
    };
    let mut cmd = Command::new(mdrs);
    cmd.arg(url).arg("-o").arg(&folder)
        .arg("-m").arg("3").arg("-t").arg("2")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr_log))
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => { inform(Update::Error(format!("Avvio MDRS fallito: {err}"))); return; }
    };
    inform(Update::Info("Download avviato. Attendo i file...".into()));
    let mut ticks = 0u32;
    let success = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            inform(Update::Cancelled(folder));
            return;
        }
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None) => {
                ticks += 1;
                if ticks % 2 == 0 {
                    let (files, bytes) = stats(&folder);
                    inform(Update::Info(format!(
                        "Download in corso: {} file, {:.1} MiB",
                        files, bytes as f64 / 1_048_576.0
                    )));
                }
                thread::sleep(Duration::from_millis(750));
            },
            Err(err) => { inform(Update::Error(format!("MDRS: {err}"))); return; }
        }
    };
    if !success {
        inform(Update::Error(format!(
            "MDRS ha segnalato un errore. Consulta il log: {}",
            log_path.display()
        )));
        return;
    }
    if cancelled.load(Ordering::Relaxed) {
        inform(Update::Cancelled(folder));
        return;
    }
    let (files, bytes) = stats(&folder);
    if files == 0 {
        inform(Update::Error(format!("Nessun file scaricato. Consulta: {}", log_path.display())));
        return;
    }
    let archive = if make_zip {
        inform(Update::Info(format!("Download completo ({} file). Creo lo ZIP...", files)));
        let output_zip = folder.with_extension("zip");
        if let Err(err) = zip_folder(&folder, &output_zip, &cancelled) {
            if cancelled.load(Ordering::Relaxed) {
                inform(Update::Cancelled(folder));
            } else {
                inform(Update::Error(format!("Errore creazione ZIP: {err}. I file originali sono conservati in {}", folder.display())));
            }
            return;
        }
        Some(output_zip)
    } else {
        None
    };
    inform(Update::Done { folder, zip: archive, files, bytes });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_mediafire_urls() {
        assert_eq!(parse_key("https://www.mediafire.com/folder/r1xp6x0i043wx").unwrap(), "r1xp6x0i043wx");
        assert_eq!(parse_key("https://mediafire.com/file/abc123/example.txt").unwrap(), "abc123");
        assert!(parse_key("https://example.com/folder/r1xp6x0i043wx").is_err());
        assert!(parse_key("http://www.mediafire.com/folder/abc").is_err());
        assert!(parse_key("https://www.mediafire.com/folder/../../bad").is_err());
    }
    #[test]
    fn zip_preserves_subfolders() {
        let root = std::env::temp_dir().join(format!("mediafire-mini-zip-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("v12/addon")).unwrap();
        fs::write(root.join("v12/changelog.txt"), b"123").unwrap();
        fs::write(root.join("v12/addon/file.txt"), b"hello").unwrap();
        let output = root.with_extension("zip");
        zip_folder(&root, &output, &AtomicBool::new(false)).unwrap();
        let mut z = ZipArchive::new(File::open(&output).unwrap()).unwrap();
        assert_eq!(z.len(), 2);
        let mut s = String::new();
        z.by_name("v12/addon/file.txt").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "hello");
        drop(z);
        fs::remove_file(output).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
