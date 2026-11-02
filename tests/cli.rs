use std::process::Command;

#[test]
fn version_works_offline_without_valid_portal_or_session_directory() {
    let dir = tempfile::tempdir().unwrap();
    let invalid_state = dir.path().join("not-a-directory");
    std::fs::write(&invalid_state, "unchanged").unwrap();
    for args in [
        vec!["version"],
        vec!["version", "--json"],
        vec!["--version"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_zd-support"))
            .args(&args)
            .env("ZD_SUPPORT_URL", "invalid portal")
            .env("ZD_SUPPORT_STATE_DIR", &invalid_state)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        if args.contains(&"--json") {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
            assert_eq!(value["target"], env!("ZD_SUPPORT_TARGET"));
        }
    }
    assert_eq!(std::fs::read_to_string(invalid_state).unwrap(), "unchanged");
}
