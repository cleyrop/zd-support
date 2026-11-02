use super::*;
use crate::test_support::{Response, Server};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn session(url: &Url) -> Session {
    Session {
        version: 2,
        portal: url.to_string(),
        basic: None,
        oauth_origin: url.to_string(),
        client_id: "public-client".into(),
        access_token: "initial".into(),
        refresh_token: Some("refresh-initial".into()),
        expires_at: Some(oauth::now() + 3600),
        refresh_expires_at: None,
    }
}

#[test]
fn read_401_refreshes_and_persists_rotated_tokens() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let server = Server::new(
        move |request, _| match calls.fetch_add(1, Ordering::Relaxed) {
            0 => {
                assert_eq!(request.headers["authorization"], "Bearer initial");
                Response {
                    status: 401,
                    headers: vec![],
                    body: "{}".into(),
                }
            }
            1 => {
                assert_eq!(request.path, "/oauth/tokens");
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(body["grant_type"], "refresh_token");
                assert_eq!(body["refresh_token"], "refresh-initial");
                assert_eq!(body["client_id"], "public-client");
                assert!(body.get("client_secret").is_none());
                Response::json(
                    json!({"access_token":"rotated", "refresh_token":"refresh-rotated", "token_type":"bearer", "expires_in":1800, "refresh_token_expires_in":604800}),
                )
            }
            2 => {
                assert_eq!(request.method, "GET");
                assert_eq!(request.headers["authorization"], "Bearer rotated");
                Response::json(json!({"request":{"id":42}}))
            }
            _ => panic!("Unexpected retry"),
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    storage.save(&session(&server.url)).unwrap();
    let api = Api::load(&storage, &server.url).unwrap();
    assert_eq!(
        api.request("GET", "/api/v2/requests/42.json", None)
            .unwrap()["request"]["id"],
        42
    );
    let saved = storage.load(&server.url).unwrap();
    assert_eq!(saved.access_token, "rotated");
    assert_eq!(saved.refresh_token.as_deref(), Some("refresh-rotated"));
    assert_eq!(count.load(Ordering::Relaxed), 3);
}

#[test]
fn writes_use_bearer_auth_and_are_not_replayed_after_401() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let server = Server::new(move |request, _| {
        calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/v2/requests.json");
        assert_eq!(request.headers["authorization"], "Bearer initial");
        assert!(!request.headers.contains_key("cookie"));
        assert!(!request.headers.contains_key("x-csrf-token"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            body["request"]["comment"]["body"],
            "Diagnostic\n\"quoted\" é"
        );
        Response {
            status: 401,
            headers: vec![],
            body: "{}".into(),
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let api = Api::new(&storage, session(&server.url)).unwrap();
    assert!(
        api.request(
            "POST",
            "/api/v2/requests.json",
            Some(json!({"request":{"comment":{"body":"Diagnostic\n\"quoted\" é"}}}))
        )
        .unwrap_err()
        .to_string()
        .contains("401")
    );
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[test]
fn follows_cursor_pagination_and_rejects_external_links() {
    let server = Server::new(|request, base| {
        if request.path.contains("page=2") {
            Response::json(
                json!({"requests": [{"id": 2}], "users": [{"id": 10, "name": "Alice"}, {"id": 20, "name": "Support", "agent": true}], "meta": {"has_more": false}, "links": {"next": null}}),
            )
        } else {
            Response::json(
                json!({"requests": [{"id": 1}], "users": [{"id": 10, "name": "Alice"}], "meta": {"has_more": true}, "links": {"next": base.join("api/v2/requests.json?page=2").unwrap().as_str()}}),
            )
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let api = Api::new(&storage, session(&server.url)).unwrap();
    let value = api
        .collection("/api/v2/requests.json", "requests", true)
        .unwrap();
    assert_eq!(value["requests"].as_array().unwrap().len(), 2);
    // Authors from later pages must remain available, without duplicates.
    assert_eq!(
        value["users"],
        json!([{"id": 10, "name": "Alice"}, {"id": 20, "name": "Support", "agent": true}])
    );
    assert!(value["next_page"].is_null());
    for url in [
        "https://evil.example/api/v2/requests.json",
        "/api/v2/tickets.json",
        "/api/v2/requests/../../users.json",
        "//evil.example/api/v2/requests.json",
    ] {
        assert!(api.url(url).is_err(), "{url}");
    }
}

#[test]
fn redirect_is_not_followed_and_status_output_has_no_csrf_token() {
    let server = Server::new(|request, _| {
        if request.path == "/api/v2/users/me.json" {
            Response::json(
                json!({"user": {"id": 1, "role": "end-user", "email": "test@example.com", "authenticity_token": "never-print-this"}}),
            )
        } else {
            Response {
                status: 302,
                headers: vec![("Location".into(), "https://evil.example/".into())],
                body: String::new(),
            }
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let api = Api::new(&storage, session(&server.url)).unwrap();
    assert!(
        !api.verify()
            .unwrap()
            .to_string()
            .contains("never-print-this")
    );
    assert!(
        api.request("GET", "/api/v2/requests.json", None)
            .unwrap_err()
            .to_string()
            .contains("redirect refused")
    );
}

#[test]
fn reports_rate_limits_without_retrying() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let server = Server::new(move |_, _| {
        calls.fetch_add(1, Ordering::Relaxed);
        Response {
            status: 429,
            headers: vec![("Retry-After".into(), "60".into())],
            body: "{}".into(),
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let api = Api::new(&storage, session(&server.url)).unwrap();
    assert!(
        api.request("GET", "/api/v2/requests.json", None)
            .unwrap_err()
            .to_string()
            .contains("Retry-After: 60")
    );
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[test]
fn password_login_verifies_requests_then_reuses_credentials_for_writes() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let server = Server::new(move |request, _| {
        assert_eq!(
            request.headers["authorization"],
            format!(
                "Basic {}",
                STANDARD.encode("user@example.com: fake:pássword ")
            )
        );
        assert!(!request.headers.contains_key("x-csrf-token"));
        match calls.fetch_add(1, Ordering::Relaxed) {
            0 => {
                assert_eq!(request.path, "/api/v2/users/me.json");
                Response::json(
                    json!({"user":{"id":42,"email":"user@example.com","role":"end-user"}}),
                )
            }
            1 => {
                assert_eq!(request.method, "GET");
                assert!(request.path.starts_with("/api/v2/requests.json?"));
                Response::json(json!({"requests":[]}))
            }
            2 => {
                assert_eq!(request.path, "/api/v2/users/me.json");
                Response::json(json!({"user":{"id":42,"role":"end-user"}}))
            }
            3 => {
                assert_eq!(request.method, "PUT");
                assert_eq!(request.path, "/api/v2/requests/42.json");
                Response::json(json!({"request":{"id":42}}))
            }
            _ => panic!("Unexpected call"),
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let credentials = Session::basic(
        &server.url,
        "user@example.com".into(),
        " fake:pássword ".into(),
    )
    .unwrap();
    let api = Api::new(&storage, credentials).unwrap();
    let result = api.authenticate_and_save().unwrap();
    assert_eq!(result["user"]["id"], 42);
    assert!(!result.to_string().contains("pássword"));
    let reloaded = Api::load(&storage, &server.url).unwrap();
    reloaded
        .request(
            "PUT",
            "/api/v2/requests/42.json",
            Some(json!({"request":{"comment":{"body":"Test"}}})),
        )
        .unwrap();
    assert_eq!(count.load(Ordering::Relaxed), 4);
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
    storage.remove().unwrap();
    assert!(storage.load(&server.url).is_err());
}

#[test]
fn password_login_failure_preserves_existing_session_and_does_not_retry() {
    for status in [401, 403] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let server = Server::new(move |request, _| {
            calls.fetch_add(1, Ordering::Relaxed);
            if status == 403 && request.path == "/api/v2/users/me.json" {
                Response::json(json!({"user":{"id":42,"role":"end-user"}}))
            } else {
                Response {
                    status,
                    headers: vec![],
                    body: "{}".into(),
                }
            }
        });
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
        storage.save(&session(&server.url)).unwrap();
        let credentials = Session::basic(
            &server.url,
            "user@example.com".into(),
            "rejected-password".into(),
        )
        .unwrap();
        let api = Api::new(&storage, credentials).unwrap();
        let error = api.authenticate_and_save().unwrap_err().to_string();
        assert!(error.contains(&status.to_string()));
        assert!(!error.contains("rejected-password"));
        assert_eq!(storage.load(&server.url).unwrap().access_token, "initial");
        assert_eq!(
            count.load(Ordering::Relaxed),
            if status == 401 { 1 } else { 2 }
        );
    }
}

#[test]
fn password_write_is_blocked_when_server_treats_credentials_as_anonymous() {
    let server = Server::new(|request, _| {
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/v2/users/me.json");
        Response::json(json!({"user":{"id":null,"role":"anonymous"}}))
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let session = Session::basic(
        &server.url,
        "user@example.com".into(),
        "fake-password".into(),
    )
    .unwrap();
    let api = Api::new(&storage, session).unwrap();
    assert!(
        api.request(
            "POST",
            "/api/v2/requests.json",
            Some(json!({"request":{"subject":"Must not be sent"}}))
        )
        .unwrap_err()
        .to_string()
        .contains("anonymous")
    );
}

#[test]
fn server_error_preserves_http_status_and_write_delivery_warning() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let server = Server::new(move |_, _| {
        calls.fetch_add(1, Ordering::Relaxed);
        Response {
            status: 502,
            headers: vec![],
            body: "<html>Bad gateway</html>".into(),
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let api = Api::new(&storage, session(&server.url)).unwrap();
    let error = format!(
        "{:#}",
        api.request(
            "POST",
            "/api/v2/requests.json",
            Some(json!({"request": {"subject": "Test"}}))
        )
        .unwrap_err()
    );
    assert!(error.contains("502"), "{error}");
    assert!(error.contains("delivery is unknown"), "{error}");
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[test]
fn rejects_false_success_and_mismatched_ticket_responses() {
    let server = Server::new(|request, _| {
        if request.method == "POST" {
            Response::json(json!({"error": "Unexpected proxy response"}))
        } else {
            Response::json(json!({"request": {"id": 99}}))
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
    let api = Api::new(&storage, session(&server.url)).unwrap();
    let error = api
        .ticket(
            "POST",
            "/api/v2/requests.json",
            Some(json!({"request": {}})),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("delivery is unknown"));
    assert!(api.ticket("GET", "/api/v2/requests/42.json", None).is_err());
}

#[test]
fn refuses_pagination_cycles_and_cross_origin_without_following_them() {
    for external in [false, true] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let server = Server::new(move |_, base| {
            calls.fetch_add(1, Ordering::Relaxed);
            let next = if external {
                "https://example.invalid/api/v2/requests.json".into()
            } else {
                base.join("api/v2/requests.json").unwrap().to_string()
            };
            Response::json(json!({"requests": [], "next_page": next}))
        });
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(Some(temp.path().into()), &server.url).unwrap();
        let api = Api::new(&storage, session(&server.url)).unwrap();
        assert!(
            api.collection("/api/v2/requests.json", "requests", true)
                .is_err()
        );
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
