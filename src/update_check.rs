use ring::digest::{Context, SHA256};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

pub const EMBEDDED_ENGINE_VERSION: &str = "0.2.3";
const APP_REPO: &str = "daviidevvvv/MediaFireMini";
const MDRS_REPO: &str = "NicKoehler/mediafire_rs";
const APP_ASSET_NAME: &str = "MediaFireMini.exe";
pub const RELEASES_PAGE_URL: &str = "https://github.com/NicKoehler/mediafire_rs/releases/latest";
const MAX_ASSET_BYTES: u64 = 80 * 1024 * 1024;
const FINISH_UPDATE_ARG: &str = "--finish-self-update";

#[derive(Debug, Clone)]
pub struct AppUpdate {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub enum CheckResult {
    AppUpdateAvailable {
        current_app: String,
        update: AppUpdate,
    },
    UpstreamEngineNotice {
        installed_engine: String,
        latest_engine: String,
        url: String,
    },
    UpToDate {
        app_version: String,
        engine_version: String,
    },
    Error(String),
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: Option<String>,
    assets: Option<Vec<GitHubAsset>>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}

fn valid_version(input: &str) -> bool {
    let digits = input.trim_start_matches('v');
    !digits.is_empty()
        && digits.len() < 40
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && digits
            .split('.')
            .all(|part| !part.is_empty() && part.parse::<u64>().is_ok())
}

pub fn is_newer(remote: &str, installed: &str) -> bool {
    if !valid_version(remote) || !valid_version(installed) {
        return false;
    }
    let left_parts: Vec<u64> = remote
        .trim_start_matches('v')
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    let right_parts: Vec<u64> = installed
        .trim_start_matches('v')
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();

    for i in 0..left_parts.len().max(right_parts.len()) {
        let left = left_parts.get(i).copied().unwrap_or(0);
        let right = right_parts.get(i).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn normalized_sha256(digest: Option<&str>) -> Result<String, String> {
    let raw = digest.ok_or("Release senza digest SHA256: aggiornamento bloccato.")?;
    let hex = raw
        .strip_prefix("sha256:")
        .ok_or("Digest release non SHA256.")?;
    if hex.len() != 64 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Digest SHA256 GitHub non valido.".into());
    }
    Ok(hex.to_ascii_lowercase())
}

fn build_http_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(concat!("MediaFireMini/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Client HTTP: {e}"))
}

async fn fetch_release(client: &reqwest::Client, repo: &str) -> Result<GitHubRelease, String> {
    let api_url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Connessione GitHub: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub HTTP {}", resp.status()));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| format!("Lettura risposta: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("JSON GitHub non valido: {e}"))
}

fn parse_app_update(
    release: &GitHubRelease,
    current_app: &str,
) -> Result<Option<AppUpdate>, String> {
    if !is_newer(&release.tag_name, current_app) {
        return Ok(None);
    }
    let assets = release.assets.as_deref().unwrap_or(&[]);
    let asset = assets
        .iter()
        .find(|a| a.name == APP_ASSET_NAME)
        .ok_or_else(|| {
            format!(
                "Release {} senza asset {}",
                release.tag_name, APP_ASSET_NAME
            )
        })?;
    if asset.size < 8192 || asset.size > MAX_ASSET_BYTES {
        return Err("Dimensione asset MediaFireMini.exe inattesa.".into());
    }
    let expected_prefix = format!("https://github.com/{APP_REPO}/releases/download/");
    if !asset.browser_download_url.starts_with(&expected_prefix)
        || !asset.browser_download_url.ends_with(APP_ASSET_NAME)
    {
        return Err("URL download asset non attendibile.".into());
    }
    let sha256 = normalized_sha256(asset.digest.as_deref())?;
    Ok(Some(AppUpdate {
        version: release.tag_name.clone(),
        download_url: asset.browser_download_url.clone(),
        sha256,
        size: asset.size,
    }))
}

pub fn check_updates() -> CheckResult {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return CheckResult::Error(format!("Runtime check fallito: {e}")),
    };

    rt.block_on(async {
        let client = match build_http_client(12) {
            Ok(c) => c,
            Err(e) => return CheckResult::Error(e),
        };

        let current_app = env!("CARGO_PKG_VERSION").to_owned();
        if let Ok(app_rel) = fetch_release(&client, APP_REPO).await {
            if let Ok(Some(update)) = parse_app_update(&app_rel, &current_app) {
                return CheckResult::AppUpdateAvailable {
                    current_app,
                    update,
                };
            }
        }

        let installed_engine = EMBEDDED_ENGINE_VERSION.to_owned();
        match fetch_release(&client, MDRS_REPO).await {
            Ok(mdrs_rel) if is_newer(&mdrs_rel.tag_name, &installed_engine) => {
                let url = mdrs_rel
                    .html_url
                    .filter(|u| u.starts_with("https://github.com/NicKoehler/mediafire_rs/"))
                    .unwrap_or_else(|| RELEASES_PAGE_URL.to_owned());
                CheckResult::UpstreamEngineNotice {
                    installed_engine,
                    latest_engine: mdrs_rel.tag_name,
                    url,
                }
            }
            Ok(_) => CheckResult::UpToDate {
                app_version: current_app,
                engine_version: installed_engine,
            },
            Err(err) => CheckResult::Error(err),
        }
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut ctx = Context::new(&SHA256);
    ctx.update(bytes);
    let digest = ctx.finish();
    let mut out = String::with_capacity(64);
    for b in digest.as_ref() {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Downloads and verifies the replacement before swapping executables and restarting.
pub fn download_and_launch_self_update(update: &AppUpdate) -> Result<(), String> {
    let expected_prefix = format!("https://github.com/{APP_REPO}/releases/download/");
    if !update.download_url.starts_with(&expected_prefix)
        || !update.download_url.ends_with(APP_ASSET_NAME)
    {
        return Err("URL aggiornamento non consentito.".into());
    }

    let current_exe = std::env::current_exe().map_err(|e| format!("Percorso EXE: {e}"))?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime download: {e}"))?;

    let bytes = rt.block_on(async {
        let client = build_http_client(60)?;
        let resp = client
            .get(&update.download_url)
            .send()
            .await
            .map_err(|e| format!("Download fallito: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("Download HTTP {}", resp.status()));
        }
        if let Some(len) = resp.content_length() {
            if len != update.size || len > MAX_ASSET_BYTES {
                return Err("Dimensione dichiarata non corrispondente.".into());
            }
        }
        let data = resp
            .bytes()
            .await
            .map_err(|e| format!("Lettura aggiornamento: {e}"))?;
        if data.len() as u64 != update.size || data.len() as u64 > MAX_ASSET_BYTES {
            return Err("Dimensione scaricata non corrispondente.".into());
        }
        Ok(data)
    })?;

    if bytes.len() < 8192 || !bytes.starts_with(b"MZ") {
        return Err("Il file scaricato non è un eseguibile Windows valido.".into());
    }

    let actual_sha = sha256_hex(&bytes);
    if actual_sha != update.sha256 {
        return Err("SHA256 NON CORRISPONDE: aggiornamento bloccato per sicurezza.".into());
    }

    install_verified_update(&current_exe, &bytes, |exe| {
        Command::new(exe).arg(FINISH_UPDATE_ARG).spawn().map(|_| ())
    })
}

fn update_dir(exe: &Path) -> PathBuf {
    let mut dir = exe.as_os_str().to_owned();
    dir.push(".update");
    PathBuf::from(dir)
}

fn install_verified_update(
    exe: &Path,
    bytes: &[u8],
    restart: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), String> {
    // Use the same volume for both renames. create_dir reserves this location
    // for one updater even when several instances are open.
    let dir = update_dir(exe);
    fs::create_dir(&dir)
        .map_err(|e| format!("Preparazione aggiornamento in {}: {e}", dir.display()))?;
    let staged = dir.join("new.exe");
    let backup = dir.join("previous.exe");

    let result = (|| {
        let mut file = File::create_new(&staged)
            .map_err(|e| format!("Creazione del nuovo eseguibile: {e}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("Salvataggio del nuovo eseguibile: {e}"))?;
        drop(file);

        // Windows allows renaming the running image while keeping it mapped.
        // The old instance can therefore report errors before closing.
        fs::rename(exe, &backup)
            .map_err(|e| format!("Spostamento dell'eseguibile attuale: {e}"))?;
        if let Err(error) = fs::rename(&staged, exe).and_then(|()| restart(exe)) {
            if let Err(restore_error) = fs::rename(&backup, exe) {
                return Err(format!(
                    "Installazione fallita: {error}. Ripristino fallito: {restore_error}. Copia precedente: {}",
                    backup.display()
                ));
            }
            return Err(format!(
                "Installazione fallita: {error}. Versione precedente ripristinata."
            ));
        }
        Ok(())
    })();

    if result.is_err() && !backup.exists() {
        let _ = fs::remove_file(&staged);
        let _ = fs::remove_dir(&dir);
    }
    result
}

pub fn finish_self_update() {
    if std::env::args_os().nth(1).as_deref() != Some(FINISH_UPDATE_ARG.as_ref()) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let dir = update_dir(&exe);
    let backup = dir.join("previous.exe");
    // The new process can start just before the original image is unmapped.
    thread::spawn(move || {
        for _ in 0..100 {
            match fs::remove_file(&backup) {
                Ok(()) => {
                    let _ = fs::remove_dir(&dir);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_strictly() {
        assert!(is_newer("v0.3.2", "0.3.1"));
        assert!(is_newer("1.0.0", "0.3.1"));
        assert!(!is_newer("v0.3.1", "0.3.1"));
        assert!(!is_newer("v0.3.0", "0.3.1"));
        assert!(!is_newer("v0.3.2-rc1", "0.3.1"));
    }

    #[test]
    fn verifies_sha256_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn rejects_missing_or_invalid_digest() {
        assert!(normalized_sha256(None).is_err());
        assert!(normalized_sha256(Some("sha256:xyz")).is_err());
        assert!(normalized_sha256(Some(&format!("sha256:{}", "a".repeat(64)))).is_ok());
    }

    #[test]
    fn restores_previous_executable_when_relaunch_fails() {
        let dir =
            std::env::temp_dir().join(format!("mediafire-mini-update-test-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("MediaFire Mini & avanzamento 100%.exe");
        fs::write(&exe, b"previous executable").unwrap();

        let result = install_verified_update(&exe, b"new executable", |installed| {
            assert_eq!(fs::read(installed)?, b"new executable");
            Err(io::Error::other("simulated launch failure"))
        });
        let restored = fs::read(&exe);
        let discarded_staging = !update_dir(&exe).exists();
        fs::remove_file(exe).unwrap();
        fs::remove_dir(dir).unwrap();

        assert!(result
            .unwrap_err()
            .contains("Versione precedente ripristinata"));
        assert_eq!(restored.unwrap(), b"previous executable");
        assert!(discarded_staging);
    }
}
