//! Autonomous updates: HTTPS metadata, GitHub asset digests, SHA256SUMS, atomic install.
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    process::Command,
};
use tempfile::NamedTempFile;
use url::Url;

const REPOSITORY: &str = "cleyrop/zd-support";
const LATEST: &str = "https://api.github.com/repos/cleyrop/zd-support/releases/latest";
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
const MAX_BINARY: u64 = 32 * 1024 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    immutable: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    digest: String,
    browser_download_url: String,
}

fn archive_name(tag: &str, target: &str) -> Result<String> {
    let suffix = match target {
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => "zip",
        "aarch64-unknown-linux-gnu" | "x86_64-unknown-linux-gnu" => "tar.gz",
        _ => bail!("Self-update is not supported for {target}; install from source"),
    };
    Ok(format!("zd-support-{tag}-{target}.{suffix}"))
}

impl Release {
    fn version(&self) -> Result<Version> {
        ensure!(
            !self.draft && !self.prerelease && self.immutable,
            "Expected an immutable stable release"
        );
        let version = Version::parse(
            self.tag_name
                .strip_prefix('v')
                .context("Invalid release tag")?,
        )?;
        ensure!(
            version.pre.is_empty() && version.build.is_empty(),
            "Expected a stable release tag"
        );
        ensure!(
            self.tag_name == format!("v{version}"),
            "Non-canonical release tag"
        );
        Ok(version)
    }

    fn asset(&self, name: &str, limit: u64) -> Result<&Asset> {
        let matches: Vec<_> = self.assets.iter().filter(|a| a.name == name).collect();
        ensure!(
            matches.len() == 1,
            "Missing or duplicate release asset: {name}"
        );
        let asset = matches[0];
        ensure!(
            asset.size > 0 && asset.size <= limit,
            "Invalid asset size: {name}"
        );
        parse_digest(&asset.digest)?;
        let expected = format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/{name}",
            self.tag_name
        );
        ensure!(
            asset.browser_download_url == expected,
            "Unexpected release asset URL"
        );
        Ok(asset)
    }
}

fn parse_digest(input: &str) -> Result<&str> {
    let digest = input
        .strip_prefix("sha256:")
        .context("Missing GitHub SHA-256 digest")?;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Invalid SHA-256 digest"
    );
    Ok(digest)
}

fn validate_download_url(url: &Url) -> Result<()> {
    ensure!(
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && matches!(
                url.host_str(),
                Some(
                    "github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                )
            ),
        "Refusing download outside GitHub's HTTPS release hosts"
    );
    Ok(())
}

fn download(client: &Client, asset: &Asset, out: &mut impl Write) -> Result<()> {
    let mut url = Url::parse(&asset.browser_download_url)?;
    for _ in 0..=5 {
        validate_download_url(&url)?;
        // This client never receives support credentials or a GitHub token.
        let response = client
            .get(url.clone())
            .send()
            .context("Release download failed")?;
        if response.status().is_redirection() {
            url = url.join(
                response
                    .headers()
                    .get("Location")
                    .context("Redirect without Location")?
                    .to_str()?,
            )?;
            continue;
        }
        ensure!(
            response.status().is_success(),
            "Download returned HTTP {}",
            response.status()
        );
        let mut reader = response.take(asset.size + 1);
        let count = io::copy(&mut reader, out)?;
        ensure!(
            count == asset.size,
            "Downloaded asset size does not match GitHub metadata"
        );
        return Ok(());
    }
    bail!("Too many release download redirects")
}

fn sha256(mut reader: impl Read) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_checksum(
    manifest: &[u8],
    manifest_asset: &Asset,
    archive_asset: &Asset,
) -> Result<String> {
    ensure!(
        sha256(manifest)? == parse_digest(&manifest_asset.digest)?,
        "SHA256SUMS does not match its GitHub digest"
    );
    let text = std::str::from_utf8(manifest).context("Invalid SHA256SUMS encoding")?;
    let mut found = None;
    for line in text.lines() {
        let Some((digest, name)) = line.split_once("  ") else {
            bail!("Invalid SHA256SUMS line")
        };
        if name == archive_asset.name {
            ensure!(found.is_none(), "Duplicate archive checksum");
            ensure!(
                digest == parse_digest(&archive_asset.digest)?,
                "Archive digests disagree"
            );
            found = Some(digest.to_owned());
        }
    }
    found.context("Archive absent from SHA256SUMS")
}

fn copy_binary(reader: impl Read, size: u64, out: &mut File) -> Result<()> {
    ensure!(size > 0 && size <= MAX_BINARY, "Invalid executable size");
    let count = io::copy(&mut reader.take(MAX_BINARY + 1), out)?;
    ensure!(count == size, "Truncated or oversized executable");
    Ok(())
}

#[cfg(target_os = "macos")]
fn extract_binary(archive: &Path, name: &str, out: &mut File) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(archive)?)?;
    ensure!(archive.len() <= 10_000, "Too many ZIP entries");
    let mut found = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.name()? != name {
            continue;
        }
        ensure!(!found, "Duplicate executable in archive");
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        ensure!(
            entry.is_file() && (kind == 0 || kind == 0o100000),
            "Executable must be a regular file"
        );
        let size = entry.size();
        copy_binary(&mut entry, size, out)?;
        found = true;
    }
    ensure!(found, "Executable absent from ZIP archive");
    Ok(())
}

#[cfg(target_os = "linux")]
fn extract_binary(archive: &Path, name: &str, out: &mut File) -> Result<()> {
    let reader = flate2::read::GzDecoder::new(File::open(archive)?).take(256 * 1024 * 1024);
    let mut archive = tar::Archive::new(reader);
    let mut found = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path_bytes().as_ref() != name.as_bytes() {
            continue;
        }
        ensure!(
            !found && entry.header().entry_type().is_file(),
            "Duplicate or non-regular executable in archive"
        );
        let size = entry.size();
        copy_binary(&mut entry, size, out)?;
        found = true;
    }
    ensure!(found, "Executable absent from tar archive");
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn extract_binary(_: &Path, _: &str, _: &mut File) -> Result<()> {
    bail!("Self-update is unsupported on this OS")
}

fn validate_binary(path: &Path, version: &Version) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let signature = Command::new("/usr/bin/codesign")
            .args([
                "--verify",
                "--strict",
                "-R=anchor apple generic and certificate leaf[subject.OU] = \"4SKW2Z97A2\"",
            ])
            .arg(path)
            .output()?;
        ensure!(
            signature.status.success(),
            "Apple Developer ID signature verification failed"
        );
    }
    let output = Command::new(path)
        .arg("--version")
        .output()
        .context("Cannot execute downloaded binary on this system")?;
    ensure!(
        output.status.success() && output.stdout == format!("zd-support {version}\n").as_bytes(),
        "Downloaded binary has an unexpected version"
    );
    Ok(())
}

fn install(
    archive: &Path,
    expected_digest: &str,
    member: &str,
    destination: &Path,
    validate: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    ensure!(
        sha256(File::open(archive)?)? == expected_digest,
        "Downloaded archive SHA-256 mismatch; installed binary unchanged"
    );
    let parent = destination
        .parent()
        .context("Executable has no parent directory")?;
    let original = fs::symlink_metadata(destination)?;
    ensure!(
        original.is_file(),
        "Installed executable must be a regular file"
    );
    let original_digest = sha256(File::open(destination)?)?;
    let mut staged = NamedTempFile::new_in(parent)
        .context("Cannot write beside the executable; use a writable installation directory")?;
    extract_binary(archive, member, staged.as_file_mut())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Preserve ordinary execute/read bits, never setuid or setgid bits.
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(
                original.permissions().mode() & 0o777,
            ))?;
    }
    staged.as_file().sync_all()?;
    // Close the writable descriptor before executing (Linux otherwise returns ETXTBSY).
    let staged = staged.into_temp_path();
    validate(&staged)?;
    ensure!(
        fs::symlink_metadata(destination)?.is_file()
            && sha256(File::open(destination)?)? == original_digest,
        "Installed executable changed during update; retry"
    );
    // Same-directory rename is atomic: every failure above leaves the old file intact.
    staged
        .persist(destination)
        .context("Cannot replace installed executable")?;
    Ok(())
}

fn lock_update(destination: &Path) -> Result<File> {
    let parent = destination
        .parent()
        .context("Executable has no parent directory")?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options
        .open(parent.join(".zd-support-update.lock"))
        .context("Cannot lock installation directory; check write permissions")?;
    ensure!(
        lock.metadata()?.is_file(),
        "Update lock is not a regular file"
    );
    lock.try_lock_exclusive()
        .context("Another self-update is running")?;
    Ok(lock)
}

pub fn run(check: bool, as_json: bool) -> Result<()> {
    let target = env!("ZD_SUPPORT_TARGET");
    archive_name("v0.0.0", target)?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let client = crate::oauth::client()?;
    let response = client
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()?;
    ensure!(
        response.status().is_success(),
        "GitHub returned HTTP {}; retry later if rate limited",
        response.status()
    );
    let release: Release = serde_json::from_reader(response.take(1024 * 1024))?;
    let latest = release.version()?;
    let available = latest > current;
    let name = archive_name(&release.tag_name, target)?;
    let archive_asset = release.asset(&name, MAX_ARCHIVE)?;
    let manifest_asset = release.asset("SHA256SUMS", 64 * 1024)?;
    let mut updated = false;
    if available && !check {
        let destination = std::env::current_exe()?.canonicalize()?;
        ensure!(
            !destination.components().any(|p| p.as_os_str() == "Cellar")
                && !destination.starts_with("/nix/store")
                && !destination.starts_with("/snap"),
            "This installation is managed by a package manager; update it with that package manager"
        );
        let _lock = lock_update(&destination)?;
        let mut manifest = Vec::new();
        download(&client, manifest_asset, &mut manifest)?;
        let digest = verify_checksum(&manifest, manifest_asset, archive_asset)?;
        let mut archive = NamedTempFile::new()?;
        download(&client, archive_asset, archive.as_file_mut())?;
        let member = format!("zd-support-{}-{target}/zd-support", release.tag_name);
        install(archive.path(), &digest, &member, &destination, |path| {
            validate_binary(path, &latest)
        })?;
        updated = true;
    }
    if as_json {
        writeln!(
            io::stdout(),
            "{}",
            json!({"current_version":current.to_string(), "latest_version":latest.to_string(), "target":target, "update_available":available, "updated":updated})
        )?;
    } else if updated {
        writeln!(io::stdout(), "Updated zd-support {current} → {latest}")?;
    } else if available {
        writeln!(
            io::stdout(),
            "Update available: {current} → {latest}. Run `zd-support self-update`."
        )?;
    } else {
        writeln!(
            io::stdout(),
            "zd-support {current} is up to date (latest release: {latest})."
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
