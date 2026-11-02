use super::*;

fn fixture_asset(name: &str, bytes: &[u8]) -> Asset {
    Asset {
        name: name.into(),
        size: bytes.len() as u64,
        digest: format!("sha256:{}", sha256(bytes).unwrap()),
        browser_download_url: format!(
            "https://github.com/cleyrop/zd-support/releases/download/v9.0.0/{name}"
        ),
    }
}

#[test]
fn metadata_rejects_mutable_prerelease_duplicate_and_external_assets() {
    let name = "zd-support-v9.0.0-aarch64-apple-darwin.zip";
    let mut release = Release {
        tag_name: "v9.0.0".into(),
        draft: false,
        prerelease: false,
        immutable: true,
        assets: vec![fixture_asset(name, b"archive")],
    };
    assert_eq!(release.version().unwrap(), Version::new(9, 0, 0));
    assert!(release.asset(name, MAX_ARCHIVE).is_ok());
    release.immutable = false;
    assert!(release.version().is_err());
    release.immutable = true;
    release.tag_name = "v9.0.0-rc.1".into();
    assert!(release.version().is_err());
    release.tag_name = "v9.0.0".into();
    release.assets[0].browser_download_url = "https://example.com/archive".into();
    assert!(release.asset(name, MAX_ARCHIVE).is_err());
    release.assets = vec![
        fixture_asset(name, b"archive"),
        fixture_asset(name, b"archive"),
    ];
    assert!(release.asset(name, MAX_ARCHIVE).is_err());
    for url in [
        "http://github.com/file",
        "https://github.com.evil.test/file",
        "https://user@github.com/file",
        "https://github.com:444/file",
        "https://127.0.0.1/file",
    ] {
        assert!(
            validate_download_url(&Url::parse(url).unwrap()).is_err(),
            "{url}"
        );
    }
    assert!(
        validate_download_url(
            &Url::parse("https://release-assets.githubusercontent.com/file?token=signed").unwrap()
        )
        .is_ok()
    );
}

#[test]
fn checksum_validation_rejects_tampering_mismatch_and_duplicate_entries() {
    let archive = fixture_asset("archive.zip", b"archive bytes");
    let digest = parse_digest(&archive.digest).unwrap();
    let manifest = format!("{digest}  archive.zip\n");
    let asset = fixture_asset("SHA256SUMS", manifest.as_bytes());
    assert_eq!(
        verify_checksum(manifest.as_bytes(), &asset, &archive).unwrap(),
        digest
    );
    assert!(verify_checksum(b"tampered", &asset, &archive).is_err());
    for text in [
        format!("{digest}  other.zip\n"),
        manifest.repeat(2),
        format!("{}  archive.zip\n", "0".repeat(64)),
    ] {
        let asset = fixture_asset("SHA256SUMS", text.as_bytes());
        assert!(verify_checksum(text.as_bytes(), &asset, &archive).is_err());
    }
}

#[cfg(target_os = "macos")]
fn write_archive(path: &Path, name: &str, bytes: &[u8], symlink: bool) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    if symlink {
        zip.add_symlink(name, "outside", options).unwrap();
    } else {
        zip.start_file(name, options.unix_permissions(0o755))
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[cfg(target_os = "linux")]
fn write_archive(path: &Path, name: &str, bytes: &[u8], symlink: bool) {
    let gzip =
        flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o755);
    header.set_size(if symlink { 0 } else { bytes.len() as u64 });
    if symlink {
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name("outside").unwrap();
    }
    header.set_cksum();
    tar.append_data(&mut header, name, if symlink { &[] } else { bytes })
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
}

#[test]
fn installation_preserves_old_binary_on_bad_hash_missing_member_or_failed_validation() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive");
    let destination = dir.path().join("zd-support");
    fs::write(&destination, b"old binary").unwrap();
    let member = "zd-support-v9.0.0-target/zd-support";
    write_archive(&archive, member, b"new binary", false);
    let digest = sha256(File::open(&archive).unwrap()).unwrap();
    assert!(install(&archive, "bad hash", member, &destination, |_| Ok(())).is_err());
    assert!(install(&archive, &digest, "absent", &destination, |_| Ok(())).is_err());
    assert!(
        install(&archive, &digest, member, &destination, |_| bail!(
            "wrong signature/version"
        ))
        .is_err()
    );
    assert_eq!(fs::read(&destination).unwrap(), b"old binary");
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        2,
        "no staging files left after failure"
    );
    install(&archive, &digest, member, &destination, |path| {
        assert_eq!(fs::read(path)?, b"new binary");
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"new binary");
}

#[test]
fn installation_rejects_symlinks_and_concurrent_changes() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive");
    let destination = dir.path().join("zd-support");
    fs::write(&destination, b"old").unwrap();
    let member = "release/zd-support";
    write_archive(&archive, member, b"", true);
    let digest = sha256(File::open(&archive).unwrap()).unwrap();
    assert!(install(&archive, &digest, member, &destination, |_| Ok(())).is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"old");
    write_archive(&archive, member, b"new", false);
    let digest = sha256(File::open(&archive).unwrap()).unwrap();
    assert!(
        install(&archive, &digest, member, &destination, |_| {
            fs::write(&destination, b"concurrent")?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(fs::read(&destination).unwrap(), b"concurrent");
    let lock = lock_update(&destination).unwrap();
    assert!(lock_update(&destination).is_err());
    drop(lock);
    assert!(lock_update(&destination).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn staged_executable_is_closed_before_execution() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive");
    let destination = dir.path().join("zd-support");
    fs::write(&destination, b"old").unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
    write_archive(
        &archive,
        "release/zd-support",
        b"#!/bin/sh\nprintf 'zd-support 9.0.0\\n'\n",
        false,
    );
    let digest = sha256(File::open(&archive).unwrap()).unwrap();
    install(
        &archive,
        &digest,
        "release/zd-support",
        &destination,
        |path| validate_binary(path, &Version::new(9, 0, 0)),
    )
    .unwrap();
}
