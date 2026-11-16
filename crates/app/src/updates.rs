//! GitHub Releases update checks, verified downloads, and installer handoff.

use std::{
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use semver::Version;
use sha2::{Digest as _, Sha256};
use tracing::{info, warn};
use ureq::Agent;

use crate::services::{AppServices, WindowCommand};

const RELEASES_LATEST_URL: &str = "https://api.github.com/repos/iFence/steward/releases/latest";
const RELEASE_DOWNLOAD_PREFIX: &str = "/iFence/steward/releases/download/";
const CHECKSUM_ASSET_NAME: &str = "SHA256SUMS-msi.txt";
const USER_AGENT: &str = concat!("Steward/", env!("CARGO_PKG_VERSION"));
const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_INSTALLER_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateAsset {
    name: String,
    url: String,
    size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum UpdateState {
    #[default]
    Idle,
    Checking,
    Available {
        latest_version: Version,
        release_notes: String,
        asset: UpdateAsset,
        checksum_url: String,
    },
    Downloading {
        latest_version: Version,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Installing,
    UpToDate,
    Error(String),
}

impl UpdateState {
    pub(crate) fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing
        )
    }

    pub(crate) fn has_update(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Shared state between the settings surface and the update worker threads.
#[derive(Clone, Default)]
pub(crate) struct UpdatePort {
    state: Arc<RwLock<UpdateState>>,
    revision: Arc<AtomicU64>,
}

impl UpdatePort {
    pub(crate) fn state(&self) -> UpdateState {
        self.state
            .read()
            .map(|state| state.clone())
            .unwrap_or_else(|_| UpdateState::Error("update state is unavailable".into()))
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Check GitHub Releases in the background. Startup checks stay silent on
    /// failure; checks initiated from settings surface the error.
    pub(crate) fn check_for_updates(&self, manual: bool) {
        if !self.begin(UpdateState::Checking) {
            return;
        }

        let port = self.clone();
        std::thread::spawn(move || {
            let result = check_latest_release(manual);
            match result {
                Ok(Some(update)) => port.set(UpdateState::Available {
                    latest_version: update.latest_version,
                    release_notes: update.release_notes,
                    asset: update.asset,
                    checksum_url: update.checksum_url,
                }),
                Ok(None) => {
                    port.set(UpdateState::UpToDate);
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    if manual {
                        warn!(error, "update check failed");
                        port.set(UpdateState::Error(error));
                    } else {
                        warn!(error, "startup update check failed");
                        port.set(UpdateState::Idle);
                    }
                }
            }
        });
    }

    /// Download and verify the available MSI, start it through the Windows
    /// shell, then ask the application event loop to quit before installation.
    pub(crate) fn download_and_install(&self, services: Arc<AppServices>) {
        let (latest_version, asset, checksum_url) = {
            let state = self.state();
            match state {
                UpdateState::Available {
                    latest_version,
                    asset,
                    checksum_url,
                    ..
                } => (latest_version, asset, checksum_url),
                _ => return,
            }
        };

        let total_bytes = (asset.size > 0).then_some(asset.size);
        if !self.begin(UpdateState::Downloading {
            latest_version: latest_version.clone(),
            downloaded_bytes: 0,
            total_bytes,
        }) {
            return;
        }

        let port = self.clone();
        std::thread::spawn(move || {
            let destination = update_destination(&asset.name);
            if let Err(error) =
                install_update(&port, &latest_version, &asset, &checksum_url, &destination)
            {
                let _ = std::fs::remove_file(&destination);
                warn!(error = %format!("{error:#}"), "update download or installation failed");
                port.set(UpdateState::Error(format!("{error:#}")));
                return;
            }

            port.set(UpdateState::Installing);
            if let Err(error) = crate::launch::open_file(&destination) {
                let _ = std::fs::remove_file(&destination);
                warn!(error = %format!("{error:#}"), "launch update installer");
                port.set(UpdateState::Error(format!(
                    "failed to start the update installer: {error:#}"
                )));
                return;
            }
            services.send_window_command(WindowCommand::Quit);
        });
    }

    fn begin(&self, state: UpdateState) -> bool {
        let Ok(mut current) = self.state.write() else {
            return false;
        };
        if current.is_busy() {
            return false;
        }
        *current = state;
        drop(current);
        self.revision.fetch_add(1, Ordering::Release);
        true
    }

    fn set(&self, state: UpdateState) {
        let Ok(mut current) = self.state.write() else {
            return;
        };
        *current = state;
        drop(current);
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn update_download_progress(
        &self,
        latest_version: &Version,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    ) {
        let Ok(mut current) = self.state.write() else {
            return;
        };
        let UpdateState::Downloading {
            latest_version: current_version,
            downloaded_bytes: current_downloaded,
            total_bytes: current_total,
        } = &mut *current
        else {
            return;
        };
        if current_version != latest_version {
            return;
        }
        *current_downloaded = downloaded_bytes;
        *current_total = total_bytes;
        drop(current);
        self.revision.fetch_add(1, Ordering::Release);
    }
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

struct UpdateInfo {
    latest_version: Version,
    release_notes: String,
    asset: UpdateAsset,
    checksum_url: String,
}

fn check_latest_release(manual: bool) -> Result<Option<UpdateInfo>> {
    let agent = http_agent();
    let release_json = fetch_text(&agent, RELEASES_LATEST_URL, MAX_METADATA_BYTES)
        .context("fetch latest GitHub release")?;
    let release: GithubRelease =
        serde_json::from_str(&release_json).context("parse GitHub release metadata")?;
    if release.draft || release.prerelease {
        return Ok(None);
    }

    let tag = release.tag_name.trim();
    let version_text = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    let latest_version =
        Version::parse(version_text).with_context(|| format!("parse release tag {tag:?}"))?;
    if !latest_version.pre.is_empty() {
        return Ok(None);
    }
    let current_version =
        Version::parse(env!("CARGO_PKG_VERSION")).context("parse current Steward version")?;
    if latest_version <= current_version {
        return Ok(None);
    }

    let installer = select_installer_asset(&release.assets, &latest_version)
        .context("release has no installer matching this Windows architecture")?;
    if installer.size == 0 {
        anyhow::bail!("installer asset has no size");
    }
    if installer.size > MAX_INSTALLER_BYTES {
        anyhow::bail!("installer is unexpectedly large: {} bytes", installer.size);
    }
    validate_release_asset_url(&installer.browser_download_url, tag)
        .context("validate installer download URL")?;

    let checksum = release
        .assets
        .iter()
        .find(|asset| asset.name == CHECKSUM_ASSET_NAME)
        .context("release has no MSI checksum manifest")?;
    validate_release_asset_url(&checksum.browser_download_url, tag)
        .context("validate checksum download URL")?;

    let release_notes = release
        .body
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty())
        .unwrap_or_default()
        .to_string();
    if manual {
        info!(
            version = %latest_version,
            asset = %installer.name,
            "Steward update available"
        );
    }

    Ok(Some(UpdateInfo {
        latest_version,
        release_notes,
        asset: UpdateAsset {
            name: installer.name.clone(),
            url: installer.browser_download_url.clone(),
            size: installer.size,
        },
        checksum_url: checksum.browser_download_url.clone(),
    }))
}

fn http_agent() -> Agent {
    Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .into()
}

fn fetch_text(agent: &Agent, url: &str, limit: u64) -> Result<String> {
    let mut response = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("request {url}"))?;
    let mut body = String::new();
    response
        .body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_string(&mut body)
        .with_context(|| format!("read response from {url}"))?;
    if body.len() as u64 > limit {
        anyhow::bail!("response from {url} exceeded the size limit");
    }
    Ok(body)
}

fn select_installer_asset<'a>(
    assets: &'a [GithubAsset],
    version: &Version,
) -> Option<&'a GithubAsset> {
    let prefix = format!("steward-app-{version}-");
    let suffix = format!("-{}.msi", std::env::consts::ARCH);
    assets.iter().find(|asset| {
        is_safe_update_asset_name(&asset.name)
            && asset.name.starts_with(&prefix)
            && asset.name.ends_with(&suffix)
    })
}

fn is_safe_update_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn validate_release_asset_url(value: &str, tag: &str) -> Result<()> {
    let url = url::Url::parse(value).context("parse release asset URL")?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        anyhow::bail!("release asset must use https://github.com");
    }
    let expected_prefix = format!("{RELEASE_DOWNLOAD_PREFIX}{tag}/");
    if !url.path().starts_with(&expected_prefix) {
        anyhow::bail!("release asset is outside the Steward release for {tag}");
    }
    Ok(())
}

fn install_update(
    port: &UpdatePort,
    latest_version: &Version,
    asset: &UpdateAsset,
    checksum_url: &str,
    destination: &Path,
) -> Result<()> {
    let agent = http_agent();
    let checksums = fetch_text(&agent, checksum_url, MAX_METADATA_BYTES)
        .context("download MSI checksum manifest")?;
    let expected_sha256 = checksum_for_asset(&checksums, &asset.name)
        .context("checksum manifest has no MSI entry")?;

    let mut response = agent
        .get(&asset.url)
        .header("Accept", "application/octet-stream")
        .header("User-Agent", USER_AGENT)
        .call()
        .context("download Steward installer")?;
    let mut source = response.body_mut().as_reader();
    let mut file = std::fs::File::create(destination).context("create update download file")?;
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    let mut last_progress = Instant::now();
    loop {
        let read = source
            .read(&mut buffer)
            .context("read installer download")?;
        if read == 0 {
            break;
        }
        downloaded = downloaded.saturating_add(read as u64);
        if downloaded > MAX_INSTALLER_BYTES {
            anyhow::bail!("installer exceeded the download size limit");
        }
        file.write_all(&buffer[..read])
            .context("write installer download")?;
        if last_progress.elapsed() >= Duration::from_millis(100) {
            last_progress = Instant::now();
            port.update_download_progress(
                latest_version,
                downloaded,
                (asset.size > 0).then_some(asset.size),
            );
        }
    }
    file.sync_all().context("flush installer download")?;
    if downloaded != asset.size {
        anyhow::bail!(
            "installer download was incomplete: downloaded {downloaded} of {} bytes",
            asset.size
        );
    }

    let actual_sha256 = sha256_file(destination).context("hash installer download")?;
    if !actual_sha256.eq_ignore_ascii_case(&expected_sha256) {
        anyhow::bail!(
            "installer checksum mismatch: expected {expected_sha256}, got {actual_sha256}"
        );
    }
    port.update_download_progress(
        latest_version,
        downloaded,
        (asset.size > 0).then_some(asset.size),
    );
    Ok(())
}

fn checksum_for_asset(contents: &str, asset_name: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        (is_sha256(hash) && name == asset_name).then(|| hash.to_ascii_lowercase())
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).context("open downloaded installer")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .context("read downloaded installer")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn update_destination(asset_name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "steward-update-{}-{nonce}-{asset_name}",
        std::process::id(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: u64, minor: u64, patch: u64) -> Version {
        Version::new(major, minor, patch)
    }

    fn asset(name: &str, size: u64) -> GithubAsset {
        GithubAsset {
            name: name.into(),
            browser_download_url: format!(
                "https://github.com/iFence/steward/releases/download/v0.3.2/{name}"
            ),
            size,
        }
    }

    #[test]
    fn selects_the_current_architecture_installer_for_the_release() {
        let installer = format!("steward-app-0.3.2-{}.msi", std::env::consts::ARCH);
        let assets = vec![
            asset("steward-app-0.3.1-x86_64.msi", 10),
            asset(&installer, 20),
            asset("steward-plugin-calendar-0.2.0.zip", 30),
        ];

        let selected = select_installer_asset(&assets, &version(0, 3, 2)).unwrap();
        assert_eq!(selected.name, installer);
        assert_eq!(selected.size, 20);
    }

    #[test]
    fn rejects_installers_from_another_version() {
        let installer = format!("steward-app-0.3.1-{}.msi", std::env::consts::ARCH);
        let assets = vec![asset(&installer, 20)];
        assert!(select_installer_asset(&assets, &version(0, 3, 2)).is_none());
    }

    #[test]
    fn rejects_installer_names_that_are_not_plain_file_names() {
        let unsafe_name = "steward-app-0.3.2-../x86_64.msi".to_string();
        let assets = vec![asset(&unsafe_name, 20)];
        assert!(select_installer_asset(&assets, &version(0, 3, 2)).is_none());
    }

    #[test]
    fn checksum_manifest_matches_exact_asset_names() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest = format!("{hash}  steward-app-0.3.2-x86_64.msi\n");
        assert_eq!(
            checksum_for_asset(&manifest, "steward-app-0.3.2-x86_64.msi").as_deref(),
            Some(hash)
        );
        assert!(checksum_for_asset(&manifest, "steward-app-0.3.2-x86_64.msi.exe").is_none());
    }

    #[test]
    fn checksum_manifest_rejects_invalid_hashes() {
        let manifest = "not-a-hash  steward-app-0.3.2-x86_64.msi\n";
        assert!(checksum_for_asset(manifest, "steward-app-0.3.2-x86_64.msi").is_none());
    }

    #[test]
    fn release_urls_must_stay_inside_the_expected_repository_release() {
        assert!(validate_release_asset_url(
            "https://github.com/iFence/steward/releases/download/v0.3.2/steward-app-0.3.2-x86_64.msi",
            "v0.3.2"
        )
        .is_ok());
        assert!(validate_release_asset_url(
            "https://example.com/steward-app-0.3.2-x86_64.msi",
            "v0.3.2"
        )
        .is_err());
        assert!(validate_release_asset_url(
            "https://github.com/other/repo/releases/download/v0.3.2/steward-app-0.3.2-x86_64.msi",
            "v0.3.2"
        )
        .is_err());
    }

    #[test]
    fn update_state_busy_and_available_helpers() {
        assert!(!UpdateState::Idle.is_busy());
        assert!(UpdateState::Checking.is_busy());
        assert!(!UpdateState::UpToDate.has_update());
        assert!(UpdateState::Available {
            latest_version: version(0, 3, 2),
            release_notes: String::new(),
            asset: UpdateAsset {
                name: "steward-app-0.3.2-x86_64.msi".into(),
                url: "https://github.com/iFence/steward/releases/download/v0.3.2/steward-app-0.3.2-x86_64.msi".into(),
                size: 20,
            },
            checksum_url: "https://github.com/iFence/steward/releases/download/v0.3.2/SHA256SUMS-msi.txt".into(),
        }
        .has_update());
    }
}
