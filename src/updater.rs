use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use zip::ZipArchive;

const MDRS_REPO: &str = "NicKoehler/mediafire_rs";
const ENGINE_ASSET: &str = "mdrs-windows-x64.zip";
const GUI_ASSET: &str = "MediaFireMini.exe";
const MAX_ASSET_BYTES: u64 = 60 * 1024 * 1024;
const MAX_ENGINE_BYTES: u64 = 30 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Engine, Gui }

#[derive(Debug, Clone)]
pub struct Available {
    pub kind: Kind,
    pub version: String,
    pub url: String,
    pub digest: String,
    pub size: u64,
    pub description: String,
    pub repo: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}

pub fn app_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe.parent().ok_or("Percorso app non valido")?.to_path_buf())
}

pub fn engine_version(folder: &Path) -> Result<String, String> {
    let engine = folder.join("mdrs.exe");
    let out = Command::new(engine)
        .arg("--version")
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("MDRS non disponibile: {e}"))?;
    if !out.status.success() { return Err("MDRS --version non riuscito".into()); }
    let s = String::from_utf8_lossy(&out.stdout);
    let version = s.split_whitespace().last().ok_or("Versione MDRS sconosciuta")?;
    if !valid_version(version) { return Err("Formato versione MDRS inatteso".into()); }
    Ok(version.to_owned())
}

use std::os::windows::process::CommandExt;

fn valid_version(input: &str) -> bool {
    let digits = input.trim_start_matches('v');
    !digits.is_empty() && digits.len() < 40 &&
        digits.chars().all(|c| c.is_ascii_digit() || c == '.') &&
        digits.split('.').all(|part| !part.is_empty() && part.parse::<u64>().is_ok())
}

pub fn is_newer(remote: &str, installed: &str) -> bool {
    if !valid_version(remote) || !valid_version(installed) { return false; }
    let a: Vec<u64> = remote.trim_start_matches('v').split('.').filter_map(|p| p.parse().ok()).collect();
    let b: Vec<u64> = installed.trim_start_matches('v').split('.').filter_map(|p| p.parse().ok()).collect();
    for i in 0..a.len().max(b.len()) {
        let left = a.get(i).copied().unwrap_or(0);
        let right = b.get(i).copied().unwrap_or(0);
        if left != right { return left > right; }
    }
    false
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, name].iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && p.len() <= 100 &&
        p.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)))
}

fn github_release(repo: &str) -> Result<Release, String> {
    if !valid_repo(repo) { return Err("Repository aggiornamenti non valido".into()); }
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let response = ureq::get(&url)
        .set("User-Agent", "MediaFireMini/0.2.0")
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(12))
        .call().map_err(|e| format!("GitHub: {e}"))?;
    response.into_json().map_err(|e| format!("GitHub JSON: {e}"))
}

fn normalized_digest(asset: &Asset) -> Result<String, String> {
    let digest = asset.digest.as_deref().ok_or("Release senza SHA256: aggiornamento bloccato")?;
    let sha = digest.strip_prefix("sha256:").ok_or("Digest release non SHA256")?;
    if sha.len() != 64 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Digest GitHub non valido".into());
    }
    Ok(sha.to_ascii_lowercase())
}

fn release_update(repo: &str, installed: &str, kind: Kind) -> Result<Option<Available>, String> {
    let release = github_release(repo)?;
    if !is_newer(&release.tag_name, installed) { return Ok(None); }
    let target = match kind { Kind::Engine => ENGINE_ASSET, Kind::Gui => GUI_ASSET };
    let asset = release.assets.iter().find(|a| a.name == target)
        .ok_or_else(|| format!("Release {} senza asset {}", release.tag_name, target))?;
    if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
        return Err("Dimensione dell'asset inattesa".into());
    }
    let expected_url = format!("https://github.com/{repo}/releases/download/");
    if !asset.browser_download_url.starts_with(&expected_url) ||
        !asset.browser_download_url.ends_with(target) {
        return Err("URL asset non attendibile".into());
    }
    let digest = normalized_digest(asset)?;
    Ok(Some(Available {
        kind,
        version: release.tag_name,
        url: asset.browser_download_url.clone(),
        digest,
        size: asset.size,
        description: release.body.unwrap_or_default().chars().take(300).collect(),
        repo: repo.to_owned(),
    }))
}

pub fn check(folder: &Path) -> (Vec<Available>, Vec<String>) {
    let mut available = Vec::new();
    let mut errors = Vec::new();
    match engine_version(folder).and_then(|installed| release_update(MDRS_REPO, &installed, Kind::Engine)) {
        Ok(Some(new)) => available.push(new),
        Ok(None) => {},
        Err(e) => errors.push(format!("MDRS: {e}")),
    }
    let settings = folder.join("gui-update-repo.txt");
    if let Ok(repo_text) = fs::read_to_string(settings) {
        let repo = repo_text.lines().next().unwrap_or("").trim();
        if !repo.is_empty() && !repo.starts_with('#') {
            match release_update(repo, env!("CARGO_PKG_VERSION"), Kind::Gui) {
                Ok(Some(new)) => available.push(new),
                Ok(None) => {},
                Err(e) => errors.push(format!("MediaFire Mini: {e}")),
            }
        }
    }
    (available, errors)
}

fn download_release(release: &Available) -> Result<Vec<u8>, String> {
    if !valid_repo(&release.repo) {
        return Err("Repository non valido".into());
    }
    let asset_name = match release.kind { Kind::Engine => ENGINE_ASSET, Kind::Gui => GUI_ASSET };
    let prefix = format!("https://github.com/{}/releases/download/", release.repo);
    if !release.url.starts_with(&prefix) || !release.url.ends_with(asset_name) {
        return Err("URL release non consentito".into());
    }
    let response = ureq::get(&release.url)
        .set("User-Agent", "MediaFireMini/0.2.0")
        .timeout(Duration::from_secs(30))
        .call().map_err(|e| format!("Download aggiornamento: {e}"))?;
    let mut buffer = Vec::new();
    response.into_reader().take(MAX_ASSET_BYTES + 1).read_to_end(&mut buffer)
        .map_err(|e| format!("Lettura download: {e}"))?;
    if buffer.len() as u64 != release.size || buffer.len() as u64 > MAX_ASSET_BYTES {
        return Err("Dimensione aggiornamento non corrispondente".into());
    }
    let result = format!("{:x}", Sha256::digest(&buffer));
    if result != release.digest {
        return Err("SHA256 NON CORRISPONDE: aggiornamento bloccato.".into());
    }
    Ok(buffer)
}

fn next_backup(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(format!("{name}.bak"));
    if !first.exists() { return first; }
    for n in 2..10_000 {
        let p = folder.join(format!("{name}.bak-{n}"));
        if !p.exists() { return p; }
    }
    folder.join(format!("{name}.bak-last"))
}

fn verify_exe(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 8192 || !bytes.starts_with(b"MZ") {
        return Err("L'asset non contiene un eseguibile Windows valido.".into());
    }
    Ok(())
}

pub fn install_engine(folder: &Path, release: &Available) -> Result<String, String> {
    if release.kind != Kind::Engine { return Err("Componente sbagliato".into()); }
    let downloaded = download_release(release)?;
    let mut archive = ZipArchive::new(Cursor::new(downloaded))
        .map_err(|e| format!("Archivio GitHub non valido: {e}"))?;
    let mut candidate = None;
    for i in 0..archive.len() {
        let item = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = item.name().replace('\\', "/");
        if (name == "mdrs.exe" || name.ends_with("/mdrs.exe")) && item.is_file() {
            if candidate.is_some() { return Err("Archivio contiene più copie di mdrs.exe".into()); }
            candidate = Some(i);
        }
    }
    let index = candidate.ok_or("mdrs.exe assente dall'asset ufficiale")?;
    let mut item = archive.by_index(index).map_err(|e| e.to_string())?;
    if item.size() == 0 || item.size() > MAX_ENGINE_BYTES {
        return Err("Dimensione mdrs.exe non plausibile".into());
    }
    let mut exe = Vec::with_capacity(item.size() as usize);
    item.read_to_end(&mut exe).map_err(|e| e.to_string())?;
    verify_exe(&exe)?;

    let destination = folder.join("mdrs.exe");
    let staging = folder.join("mdrs.new");
    let backup = next_backup(folder, "mdrs.exe");
    if staging.exists() { return Err("File mdrs.new preesistente; non sovrascritto".into()); }
    {
        let mut out = File::create(&staging).map_err(|e| e.to_string())?;
        out.write_all(&exe).map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())?;
    }
    let old_version = engine_version(folder)?;
    if let Err(e) = fs::rename(&destination, &backup) {
        let _ = fs::remove_file(&staging);
        return Err(format!("Impossibile salvare MDRS esistente: {e}"));
    }
    if let Err(e) = fs::rename(&staging, &destination) {
        let _ = fs::rename(&backup, &destination);
        let _ = fs::remove_file(&staging);
        return Err(format!("Installazione fallita, ripristino tentato: {e}"));
    }
    match engine_version(folder) {
        Ok(current) if current.trim_start_matches('v') == release.version.trim_start_matches('v') && is_newer(&current, &old_version) => Ok(current),
        other => {
            // Failure: move the candidate away and restore original from backup.
            let failed = folder.join("mdrs.failed");
            let _ = fs::rename(&destination, &failed);
            let rollback = fs::rename(&backup, &destination);
            let _ = fs::remove_file(&failed);
            Err(format!("Nuovo motore non valido ({other:?}); rollback: {rollback:?}"))
        }
    }
}

pub fn stage_gui(folder: &Path, release: &Available) -> Result<PathBuf, String> {
    if release.kind != Kind::Gui { return Err("Componente sbagliato".into()); }
    let bytes = download_release(release)?;
    verify_exe(&bytes)?;
    let path = folder.join("MediaFireMini.new.exe");
    if path.exists() {
        return Err("Esiste gia MediaFireMini.new.exe: non sovrascrivo.".into());
    }
    let mut file = File::create(&path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn start_gui_replacement(folder: &Path, staged: &Path, expected_sha: &str) -> Result<(), String> {
    let helper = folder.join("MediaFireMiniUpdater.exe");
    if !helper.is_file() {
        return Err("MediaFireMiniUpdater.exe mancante. Aggiornamento GUI bloccato.".into());
    }
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(helper)
        .arg(std::process::id().to_string())
        .arg(current)
        .arg(staged)
        .arg(expected_sha)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Impossibile avviare l'updater: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn semantic_versions() {
        assert!(is_newer("v0.2.4", "0.2.3"));
        assert!(is_newer("1.0.0", "0.99.9"));
        assert!(!is_newer("v0.2.3", "0.2.3"));
        assert!(!is_newer("v0.2.3-rc1", "0.2.2"));
        assert!(!is_newer("v0.2.2", "0.2.3"));
    }
    #[test] fn strict_repo() {
        assert!(valid_repo("NicKoehler/mediafire_rs"));
        assert!(!valid_repo("github.com/a/b"));
        assert!(!valid_repo("../something"));
        assert!(!valid_repo("a/b/extra"));
    }
    #[test] #[ignore] fn github_live_check() {
        let release = github_release(MDRS_REPO).expect("GitHub API unavailable");
        println!("GitHub MDRS latest: {}", release.tag_name);
        assert!(release.assets.iter().any(|a| a.name == ENGINE_ASSET));
    }
    #[test]
    #[ignore]
    fn real_engine_update_in_isolated_folder() {
        let old_url = format!("https://api.github.com/repos/{MDRS_REPO}/releases/tags/v0.2.2");
        let old: Release = ureq::get(&old_url)
            .set("User-Agent", "MediaFireMini/0.2.0")
            .timeout(Duration::from_secs(20)).call().unwrap().into_json().unwrap();
        let asset = old.assets.iter().find(|a| a.name == ENGINE_ASSET).unwrap();
        let pkg = Available {
            kind: Kind::Engine,
            version: old.tag_name.clone(),
            url: asset.browser_download_url.clone(),
            digest: normalized_digest(asset).unwrap(),
            size: asset.size,
            description: String::new(),
            repo: MDRS_REPO.to_owned(),
        };
        let bytes = download_release(&pkg).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let temp = std::env::temp_dir()
            .join(format!("mdrs-updater-test-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()));
        fs::create_dir(&temp).unwrap();
        let mut extracted = File::create(temp.join("mdrs.exe")).unwrap();
        for i in 0..zip.len() {
            let mut z = zip.by_index(i).unwrap();
            if z.name().replace('\\', "/").ends_with("/mdrs.exe") {
                std::io::copy(&mut z, &mut extracted).unwrap();
                break;
            }
        }
        drop(extracted);
        assert_eq!(engine_version(&temp).unwrap(), "0.2.2");
        let latest = release_update(MDRS_REPO, "0.2.2", Kind::Engine).unwrap().unwrap();
        let installed = install_engine(&temp, &latest).unwrap();
        assert_eq!(installed, latest.version.trim_start_matches('v'));
        assert!(temp.join("mdrs.exe.bak").is_file());
        assert_eq!(engine_version(&temp).unwrap(), installed);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test] fn reject_invalid_hash() {
        let asset = Asset { name: "x".into(), browser_download_url: "https://github.com".into(),
            digest: Some("sha256:xyz".into()), size: 100 };
        assert!(normalized_digest(&asset).is_err());
    }
}
