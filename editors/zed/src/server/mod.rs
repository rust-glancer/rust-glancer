//! Managed Rust Glancer installation for Zed.
//!
//! The extension entry point first checks settings and the worktree environment for a server.
//! This module owns the fallback. It maps the host platform to one release asset, reuses a
//! complete cached installation, or downloads the pinned server release.
//!
//! Downloads go into a separate staging directory. The final cache path only appears after the
//! expected executable is present and ready to run, so an interrupted download is never treated
//! as an installed server.

use std::{
    fs,
    path::{Path, PathBuf},
};

use zed_extension_api as zed;

pub(crate) const SERVER_BINARY: &str = "rust-glancer";

const GITHUB_REPOSITORY: &str = "rust-glancer/rust-glancer";
const SERVER_CACHE_ROOT: &str = "servers";

// An extension build requests one exact server release instead of following "latest". Release
// automation advances this pin with the extension, so a repeated installation stays reproducible.
const MANAGED_SERVER_VERSION: &str = "0.2.0"; // x-release-please-version

const MANUAL_INSTALLATION_HINT: &str =
    "install rust-glancer manually and configure lsp.rust-glancer.binary.path to use it";

/// Reuses or installs the extension-owned server while reporting progress through Zed.
pub(crate) struct ManagedServer;

impl ManagedServer {
    /// Run managed resolution and keep Zed's installation status aligned with its result.
    ///
    /// The inner installation flow reports which concrete step failed. This boundary adds the
    /// manual-install hint once, then uses the same message for both Zed's status and the returned
    /// error.
    pub(crate) fn ensure_installed(
        language_server_id: &zed::LanguageServerId,
    ) -> zed::Result<String> {
        let result = Self::ensure_installed_inner(language_server_id);

        match result {
            Ok(binary_path) => {
                zed::set_language_server_installation_status(
                    language_server_id,
                    &zed::LanguageServerInstallationStatus::None,
                );
                Ok(binary_path)
            }
            Err(error) => {
                let error = format!("{error}; {MANUAL_INSTALLATION_HINT}");
                zed::set_language_server_installation_status(
                    language_server_id,
                    &zed::LanguageServerInstallationStatus::Failed(error.clone()),
                );
                Err(error)
            }
        }
    }

    /// Reuse a complete cache entry or install the exact release asset for this host.
    fn ensure_installed_inner(language_server_id: &zed::LanguageServerId) -> zed::Result<String> {
        let platform = ServerPlatform::current()?;
        let binary_path = platform.binary_path();

        // The executable is the marker for a complete installation. A directory by itself may be
        // left behind by an older failed attempt and must not suppress the download.
        if binary_path.is_file() {
            return Ok(binary_path.to_string_lossy().into_owned());
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        // The pin determines the GitHub tag, asset name, and cache directory together. We never
        // ask GitHub for "latest", so one extension build always resolves to the same server.
        let release_tag = format!("v{MANAGED_SERVER_VERSION}");
        let release =
            zed::github_release_by_tag_name(GITHUB_REPOSITORY, &release_tag).map_err(|error| {
                format!("failed to find Rust Glancer release {release_tag}: {error}")
            })?;
        let asset_name = platform.asset_name();
        let asset = release
            .assets
            .into_iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| {
                format!("Rust Glancer release {release_tag} does not contain asset {asset_name}")
            })?;

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::Downloading,
        );

        platform.install(&asset.download_url, &asset_name)
    }
}

/// Turns one supported Zed host platform into a Rust release target.
///
/// The target string feeds both the release asset name and the cache path. The executable name is
/// kept beside it because Windows archives contain `rust-glancer.exe`, while Unix archives contain
/// `rust-glancer`. Keeping both choices here prevents the downloaded asset and extracted path from
/// describing different platforms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ServerPlatform {
    target: &'static str,
    executable: &'static str,
}

impl ServerPlatform {
    fn current() -> zed::Result<Self> {
        let (os, architecture) = zed::current_platform();
        Self::from_zed(os, architecture)
    }

    fn from_zed(os: zed::Os, architecture: zed::Architecture) -> zed::Result<Self> {
        let (target, executable) = match (os, architecture) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => ("aarch64-apple-darwin", "rust-glancer"),
            (zed::Os::Mac, zed::Architecture::X8664) => ("x86_64-apple-darwin", "rust-glancer"),
            (zed::Os::Linux, zed::Architecture::Aarch64) => {
                ("aarch64-unknown-linux-gnu", "rust-glancer")
            }
            (zed::Os::Linux, zed::Architecture::X8664) => {
                ("x86_64-unknown-linux-gnu", "rust-glancer")
            }
            (zed::Os::Windows, zed::Architecture::X8664) => {
                ("x86_64-pc-windows-msvc", "rust-glancer.exe")
            }
            _ => {
                return Err(format!(
                    "managed Rust Glancer binaries are unavailable for {os:?}/{architecture:?}"
                ));
            }
        };

        Ok(Self { target, executable })
    }

    fn asset_name(self) -> String {
        format!(
            "rust-glancer-{MANAGED_SERVER_VERSION}-{}.tar.gz",
            self.target
        )
    }

    fn version_dir(self) -> PathBuf {
        PathBuf::from(SERVER_CACHE_ROOT).join(MANAGED_SERVER_VERSION)
    }

    fn install_dir(self) -> PathBuf {
        self.version_dir().join(self.target)
    }

    fn staging_dir(self) -> PathBuf {
        self.version_dir().join(format!("{}.partial", self.target))
    }

    fn binary_path(self) -> PathBuf {
        self.install_dir().join(self.executable)
    }

    /// Build this platform's cache entry through a separate staging directory.
    fn install(self, download_url: &str, asset_name: &str) -> zed::Result<String> {
        let version_dir = self.version_dir();
        let install_dir = self.install_dir();
        let staging_dir = self.staging_dir();

        fs::create_dir_all(&version_dir).map_err(|error| {
            format!(
                "failed to create managed server directory {}: {error}",
                version_dir.display()
            )
        })?;

        // Clear only this platform's incomplete paths. Other targets and older versions remain
        // available, which keeps cleanup local to the installation this call is about to replace.
        for stale_dir in [&staging_dir, &install_dir] {
            if stale_dir.exists() {
                fs::remove_dir_all(stale_dir).map_err(|error| {
                    format!(
                        "failed to remove incomplete managed server directory {}: {error}",
                        stale_dir.display()
                    )
                })?;
            }
        }

        // Any failure before the final rename belongs to the staging directory. Remove that
        // directory so the next attempt starts from an unambiguous state.
        let result = self.download_into(download_url, asset_name, &staging_dir, &install_dir);
        if let Err(error) = result {
            if staging_dir.exists() {
                fs::remove_dir_all(&staging_dir).map_err(|cleanup_error| {
                    format!(
                        "{error}; failed to remove incomplete download {}: {cleanup_error}",
                        staging_dir.display()
                    )
                })?;
            }
            return Err(error);
        }

        Ok(self.binary_path().to_string_lossy().into_owned())
    }

    /// Download and validate the archive, then move its directory into the cache.
    fn download_into(
        self,
        download_url: &str,
        asset_name: &str,
        staging_dir: &Path,
        install_dir: &Path,
    ) -> zed::Result<()> {
        let staging_path = staging_dir.to_string_lossy();
        zed::download_file(
            download_url,
            staging_path.as_ref(),
            zed::DownloadedFileType::GzipTar,
        )
        .map_err(|error| format!("failed to download Rust Glancer asset {asset_name}: {error}"))?;

        let downloaded_binary = staging_dir.join(self.executable);
        if !downloaded_binary.is_file() {
            return Err(format!(
                "Rust Glancer asset {asset_name} did not contain {} at its root",
                self.executable,
            ));
        }

        zed::make_file_executable(downloaded_binary.to_string_lossy().as_ref()).map_err(
            |error| {
                format!(
                    "failed to make managed server {} executable: {error}",
                    downloaded_binary.display()
                )
            },
        )?;
        fs::rename(staging_dir, install_dir).map_err(|error| {
            format!(
                "failed to finish managed server installation at {}: {error}",
                install_dir.display()
            )
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_supported_platforms_to_release_assets_and_cache_paths() {
        let test_cases = [
            (
                zed::Os::Mac,
                zed::Architecture::Aarch64,
                "aarch64-apple-darwin",
                "rust-glancer",
            ),
            (
                zed::Os::Mac,
                zed::Architecture::X8664,
                "x86_64-apple-darwin",
                "rust-glancer",
            ),
            (
                zed::Os::Linux,
                zed::Architecture::Aarch64,
                "aarch64-unknown-linux-gnu",
                "rust-glancer",
            ),
            (
                zed::Os::Linux,
                zed::Architecture::X8664,
                "x86_64-unknown-linux-gnu",
                "rust-glancer",
            ),
            (
                zed::Os::Windows,
                zed::Architecture::X8664,
                "x86_64-pc-windows-msvc",
                "rust-glancer.exe",
            ),
        ];

        for (os, architecture, expected_target, expected_executable) in test_cases {
            let platform = ServerPlatform::from_zed(os, architecture)
                .expect("test platform should be supported");

            assert_eq!(platform.target, expected_target);
            assert_eq!(
                platform.asset_name(),
                format!("rust-glancer-{MANAGED_SERVER_VERSION}-{expected_target}.tar.gz")
            );
            assert_eq!(
                platform.binary_path(),
                PathBuf::from(SERVER_CACHE_ROOT)
                    .join(MANAGED_SERVER_VERSION)
                    .join(expected_target)
                    .join(expected_executable)
            );
        }
    }
}
