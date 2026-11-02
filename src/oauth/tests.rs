use super::*;
use crate::{
    session::Storage,
    test_support::{Response, Server},
};
use std::sync::{Arc, Mutex};

#[test]
fn authorization_code_roundtrip_checks_pkce_and_saves_private_tokens() {
    let challenge = Arc::new(Mutex::new(String::new()));
    let expected = challenge.clone();
    let server = Server::new(move |request, _| {
        assert_eq!(request.path, "/oauth/tokens");
        assert_eq!(request.method, "POST");
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["grant_type"], "authorization_code");
        assert_eq!(body["code"], "test-code");
        assert_eq!(body["client_id"], "public-client");
        assert!(body.get("client_secret").is_none());
        let verifier = body["code_verifier"].as_str().unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            *expected.lock().unwrap()
        );
        Response::json(
            json!({"access_token":"access-demo", "refresh_token":"refresh-demo", "token_type":"bearer", "expires_in":1800, "refresh_token_expires_in":604800}),
        )
    });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let callback_thread = Arc::new(Mutex::new(None));
    let worker = callback_thread.clone();
    let session = login_with_opener(
        &server.url,
        &server.url,
        "public-client",
        &redirect,
        "read write",
        Duration::from_secs(10),
        move |url| {
            assert_eq!(url.path(), "/oauth/authorizations/new");
            let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(params["code_challenge_method"], "S256");
            assert_eq!(params["scope"], "read write");
            *challenge.lock().unwrap() = params["code_challenge"].clone();
            let mut callback = Url::parse(&params["redirect_uri"]).unwrap();
            callback
                .query_pairs_mut()
                .append_pair("state", &params["state"])
                .append_pair("code", "test-code");
            *worker.lock().unwrap() = Some(thread::spawn(move || {
                let client = Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap();
                assert!(client.get(callback).send().unwrap().status().is_success());
            }));
            Ok(())
        },
    )
    .unwrap();
    callback_thread
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(session.access_token, "access-demo");
    assert_eq!(session.refresh_token.as_deref(), Some("refresh-demo"));
    assert!(session.expires_at.unwrap() > now());
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    storage.save(&session).unwrap();
    assert_eq!(
        storage.load(&server.url).unwrap().access_token,
        "access-demo"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = temp.path().join(format!(
            "127.0.0.1_{}/session.json",
            server.url.port().unwrap()
        ));
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn callback_rejects_forgery_and_ambiguous_parameters() {
    let redirect = Url::parse("http://127.0.0.1:8765/callback").unwrap();
    let request = |target: &str| format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n");
    for target in [
        "/callback?state=wrong&code=x",
        "/callback?state=ok&state=ok&code=x",
        "/callback?state=ok&code=x&code=y",
        "/wrong?state=ok&code=x",
        "//evil.example/callback?state=ok&code=x",
        "/callback?state=ok&code=x&error=denied",
    ] {
        assert!(callback(&request(target), &redirect, "ok").is_err());
    }
    assert!(
        callback(
            &request("/callback?state=ok&code=x").replace("127.0.0.1:8765", "evil.example"),
            &redirect,
            "ok"
        )
        .is_err()
    );
    assert_eq!(
        callback(&request("/callback?state=ok&code=x"), &redirect, "ok").unwrap(),
        Some("x".into())
    );
    assert_eq!(
        callback(
            &request("/callback?state=ok&error=access_denied"),
            &redirect,
            "ok"
        )
        .unwrap(),
        None
    );
}

#[test]
fn loopback_and_core_domain_are_required() {
    for value in [
        "http://0.0.0.0:8765/callback",
        "https://evil.example/callback",
        "http://127.0.0.1:0/callback",
        "http://user@127.0.0.1:8765/callback",
    ] {
        assert!(redirect_url(value).is_err());
    }
    let portal = Url::parse("https://support.outscale.com").unwrap();
    assert!(origin(&portal, None).is_err());
    assert!(origin(&portal, Some("evil.example/path")).is_err());
    assert_eq!(
        origin(&portal, Some("example")).unwrap().as_str(),
        "https://example.zendesk.com/"
    );
}
