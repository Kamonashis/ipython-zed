use zed_extension_api as zed;

const SERVER_ID: &str = "ipynb-lsp";
const SERVER_BINARY: &str = "ipynb-lsp";
const GITHUB_REPO: &str = "kamonashis/ipython-notebook-zed";
const RELEASE_TAG: &str = "v0.1.0";

struct IpynbExtension {}

impl zed::Extension for IpynbExtension {
    fn new() -> Self {
        IpynbExtension {}
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        if language_server_id.as_ref() != SERVER_ID {
            return Err(format!("unknown language server: {language_server_id}"));
        }

        let binary_path = self.resolve_server_binary(language_server_id, worktree)?;

        Ok(zed::Command {
            command: binary_path,
            args: Vec::new(),
            env: vec![("RUST_LOG".to_string(), "warn".to_string())],
        })
    }
}

impl IpynbExtension {
    fn resolve_server_binary(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<String> {
        if let Some(path) = std::env::var("IPYNB_LSP_BIN").ok().filter(|p| !p.is_empty()) {
            return Ok(path);
        }

        if let Some(path) = worktree.which(SERVER_BINARY) {
            return Ok(path);
        }

        self.download_server_binary(language_server_id)
    }

    fn download_server_binary(
        &mut self,
        language_server_id: &zed::LanguageServerId,
    ) -> zed::Result<String> {
        let (os, arch) = zed::current_platform();
        let (os_name, exe_suffix) = match os {
            zed::Os::Linux => ("linux", ""),
            zed::Os::Mac => ("macos", ""),
            zed::Os::Windows => ("windows", ".exe"),
        };
        let arch_name = match arch {
            zed::Architecture::X8664 => "x86_64",
            zed::Architecture::Aarch64 => "aarch64",
            zed::Architecture::X86 => return Err("x86 (32-bit) is not supported".to_string()),
        };

        let dir_name = format!("language_servers/{SERVER_BINARY}-{os_name}-{arch_name}");
        let binary_rel_path = format!("{dir_name}/{SERVER_BINARY}{exe_suffix}");

        if std::fs::metadata(&binary_rel_path).map(|m| m.is_file()).unwrap_or(false) {
            return Ok(absolute_path(&binary_rel_path)?);
        }

        let archive_name = format!("{SERVER_BINARY}-{os_name}-{arch_name}");
        let archive_file_type = match os {
            zed::Os::Windows => zed::DownloadedFileType::Zip,
            _ => zed::DownloadedFileType::GzipTar,
        };

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::Downloading,
        );

        let release = zed::github_release_by_tag_name(GITHUB_REPO, RELEASE_TAG)
            .map_err(|e| format!("failed to look up release {RELEASE_TAG}: {e}"))?;

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == archive_name)
            .ok_or_else(|| {
                format!(
                    "no asset named {archive_name} in release {RELEASE_TAG}; \
                     either build the companion binary locally and set IPYNB_LSP_BIN, \
                     or publish release artifacts (see .github/workflows/release.yml)"
                )
            })?;

        zed::download_file(&asset.download_url, &dir_name, archive_file_type)
            .map_err(|e| format!("failed to download {}: {e}", asset.download_url))?;

        if exe_suffix.is_empty() {
            zed::make_file_executable(&binary_rel_path)
                .map_err(|e| format!("failed to make {binary_rel_path} executable: {e}"))?;
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );

        Ok(absolute_path(&binary_rel_path)?)
    }
}

/// Extensions run with their working directory set to the extension's
/// installation directory, but the host forbids `chdir`, so resolve relative
/// paths against the `PWD` env var (set by the host) as a first choice.
fn absolute_path(relative: &str) -> zed::Result<String> {
    let base = std::env::var("PWD")
        .ok()
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "cannot determine extension working directory".to_string())?;

    Ok(base.join(relative).to_string_lossy().into_owned())
}

zed::register_extension!(IpynbExtension);
