use super::*;

#[test]
fn locks_exclude_concurrent_commands_and_sessions_stay_bound_to_portal() {
    let root = tempfile::tempdir().unwrap();
    let portal = Url::parse("https://example.zendesk.com/").unwrap();
    let storage = Storage::new(Some(root.path().into()), &portal).unwrap();
    let lock = storage.lock().unwrap();
    assert!(storage.lock().is_err());
    drop(lock);
    let _lock = storage.lock().unwrap();
    storage
        .save(&Session::basic(&portal, "test@example.com".into(), "fake".into()).unwrap())
        .unwrap();
    assert!(
        storage
            .load(&Url::parse("https://other.zendesk.com/").unwrap())
            .is_err()
    );
    fs::write(storage.dir.join("session.json"), "invalid json").unwrap();
    assert!(storage.load(&portal).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_and_non_private_credentials() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let portal = Url::parse("https://example.zendesk.com/").unwrap();
    let storage = Storage::new(Some(root.path().into()), &portal).unwrap();
    let credentials = Session::basic(&portal, "test@example.com".into(), "fake".into()).unwrap();
    storage.save(&credentials).unwrap();
    let path = storage.dir.join("session.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        storage
            .load(&portal)
            .err()
            .unwrap()
            .to_string()
            .contains("0600")
    );
    fs::remove_file(&path).unwrap();
    let outside = root.path().join("outside.json");
    fs::write(&outside, "do not follow").unwrap();
    symlink(&outside, &path).unwrap();
    assert!(storage.load(&portal).is_err());
    symlink(&outside, storage.dir.join("session.lock")).unwrap();
    assert!(storage.lock().is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "do not follow");

    let other_root = tempfile::tempdir().unwrap();
    let outside_dir = root.path().join("outside-dir");
    fs::create_dir(&outside_dir).unwrap();
    fs::set_permissions(&outside_dir, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(
        &outside_dir,
        other_root.path().join("example.zendesk.com_443"),
    )
    .unwrap();
    assert!(Storage::new(Some(other_root.path().into()), &portal).is_err());
    assert_eq!(
        fs::metadata(outside_dir).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
