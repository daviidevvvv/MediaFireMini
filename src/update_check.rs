use serde::Deserialize;
use std::time::Duration;

pub const EMBEDDED_ENGINE_VERSION: &str = "0.2.3";
const MDRS_REPO: &str = "NicKoehler/mediafire_rs";
pub const RELEASES_PAGE_URL: &str = "https://github.com/NicKoehler/mediafire_rs/releases/latest";

#[derive(Debug, Clone)]
pub enum EngineCheckResult {
    UpToDate {
        installed: String,
    },
    UpdateAvailable {
        installed: String,
        latest: String,
        url: String,
    },
    Error(String),
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: Option<String>,
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

/// Read-only check against GitHub Releases for `NicKoehler/mediafire_rs`.
/// Never downloads or writes files to disk.
pub fn check_latest_engine() -> EngineCheckResult {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return EngineCheckResult::Error(format!("Runtime check fallito: {e}")),
    };

    rt.block_on(async {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("MediaFireMini/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(c) => c,
            Err(e) => return EngineCheckResult::Error(format!("Client HTTP: {e}")),
        };

        let api_url = format!("https://api.github.com/repos/{MDRS_REPO}/releases/latest");
        let resp = match client
            .get(&api_url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return EngineCheckResult::Error(format!("Connessione GitHub: {e}")),
        };

        if !resp.status().is_success() {
            return EngineCheckResult::Error(format!("GitHub HTTP {}", resp.status()));
        }

        let body = match resp.text().await {
            Ok(t) => t,
            Err(e) => return EngineCheckResult::Error(format!("Lettura risposta: {e}")),
        };

        let release: GitHubRelease = match serde_json::from_str(&body) {
            Ok(r) => r,
            Err(e) => return EngineCheckResult::Error(format!("JSON GitHub non valido: {e}")),
        };

        let installed = EMBEDDED_ENGINE_VERSION.to_owned();
        if is_newer(&release.tag_name, &installed) {
            let url = release
                .html_url
                .filter(|u| u.starts_with("https://github.com/NicKoehler/mediafire_rs/"))
                .unwrap_or_else(|| RELEASES_PAGE_URL.to_owned());
            EngineCheckResult::UpdateAvailable {
                installed,
                latest: release.tag_name,
                url,
            }
        } else {
            EngineCheckResult::UpToDate { installed }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_strictly() {
        assert!(is_newer("v0.2.4", "0.2.3"));
        assert!(is_newer("1.0.0", "0.2.3"));
        assert!(!is_newer("v0.2.3", "0.2.3"));
        assert!(!is_newer("v0.2.2", "0.2.3"));
        assert!(!is_newer("v0.2.4-rc1", "0.2.3"));
    }
}
