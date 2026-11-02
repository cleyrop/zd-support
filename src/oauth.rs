use crate::session::Session;
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("zd-support/", env!("CARGO_PKG_VERSION")))
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(45))
        .build()?)
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn origin(portal: &Url, subdomain: Option<&str>) -> Result<Url> {
    let subdomain = subdomain.or_else(|| portal.host_str()?.strip_suffix(".zendesk.com"))
        .context("For a custom portal, supply --zendesk-subdomain (the core Zendesk subdomain provided by OUTSCALE)")?;
    if subdomain.is_empty()
        || !subdomain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        bail!("Invalid Zendesk subdomain; provide only the name before .zendesk.com");
    }
    Ok(Url::parse(&format!("https://{subdomain}.zendesk.com/"))?)
}

fn random_secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("System random source unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn login(
    portal: &Url,
    origin: &Url,
    client_id: &str,
    redirect: &str,
    scope: &str,
    timeout: Duration,
) -> Result<Session> {
    login_with_opener(
        portal,
        origin,
        client_id,
        redirect,
        scope,
        timeout,
        open_default,
    )
}

fn open_default(url: &Url) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(url.as_str()).status();
    #[cfg(target_os = "windows")]
    let status = Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url.as_str())
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let status = Command::new("xdg-open").arg(url.as_str()).status();
    if !status.context("Cannot open default browser")?.success() {
        bail!("Could not open default browser");
    }
    Ok(())
}

fn redirect_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("Invalid redirect URI")?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none_or(|p| p == 0)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Redirect URI must be http://127.0.0.1:PORT/path, exactly as registered in Zendesk");
    }
    Ok(url)
}

fn login_with_opener(
    portal: &Url,
    origin: &Url,
    client_id: &str,
    redirect: &str,
    scope: &str,
    timeout: Duration,
    open: impl FnOnce(&Url) -> Result<()>,
) -> Result<Session> {
    if client_id.trim().is_empty() || scope.trim().is_empty() {
        bail!("Client ID and scope must not be empty");
    }
    let redirect = redirect_url(redirect)?;
    let listener = TcpListener::bind(("127.0.0.1", redirect.port().unwrap()))
        .context("Cannot bind OAuth callback port; choose a free port registered with Zendesk")?;
    listener.set_nonblocking(true)?;
    let verifier = random_secret()?;
    let state = random_secret()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut authorization = origin.join("oauth/authorizations/new")?;
    authorization.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect.as_str()),
        ("scope", scope),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    eprintln!("Opening your default browser. Approve access to continue. Callback: {redirect}");
    open(&authorization)?;
    let code = wait_for_callback(&listener, &redirect, &state, timeout)?;
    let token = token_request(
        &client()?,
        origin,
        json!({
            "grant_type": "authorization_code", "client_id": client_id, "redirect_uri": redirect.as_str(),
            "code": code, "code_verifier": verifier, "expires_in": 1800
        }),
    )?;
    let mut session = Session {
        version: 2,
        portal: portal.to_string(),
        basic: None,
        oauth_origin: origin.to_string(),
        client_id: client_id.into(),
        access_token: String::new(),
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
    };
    apply_tokens(&mut session, &token)?;
    Ok(session)
}

fn wait_for_callback(
    listener: &TcpListener,
    redirect: &Url,
    state: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                let remaining = deadline.saturating_duration_since(Instant::now());
                stream.set_read_timeout(Some(
                    remaining
                        .min(Duration::from_secs(2))
                        .max(Duration::from_millis(1)),
                ))?;
                stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                let request = read_headers(
                    &mut stream,
                    deadline.min(Instant::now() + Duration::from_secs(2)),
                );
                match request.and_then(|r| callback(&r, redirect, state)) {
                    Ok(Some(code)) => {
                        respond(
                            &mut stream,
                            200,
                            "Authorization received. Return to the terminal to check the connection.",
                        );
                        return Ok(code);
                    }
                    Ok(None) => {
                        respond(&mut stream, 400, "Authorization was denied.");
                        bail!("OAuth authorization denied; no session saved");
                    }
                    Err(_) => respond(
                        &mut stream,
                        400,
                        "Invalid callback. Continue the sign-in from the CLI.",
                    ),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25))
            }
            Err(e) => return Err(e.into()),
        }
    }
    bail!("OAuth login timed out; run `zd-support login-oauth` again");
}

fn read_headers(stream: &mut TcpStream, deadline: Instant) -> Result<String> {
    let mut data = Vec::new();
    let mut byte = [0u8; 1];
    while data.len() < 16_384 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("Callback timed out");
        }
        stream.set_read_timeout(Some(remaining))?;
        if stream.read(&mut byte)? == 0 {
            bail!("Incomplete callback");
        }
        data.push(byte[0]);
        if data.ends_with(b"\r\n\r\n") {
            return Ok(String::from_utf8(data)?);
        }
    }
    bail!("Callback headers too large")
}

fn callback(request: &str, redirect: &Url, state: &str) -> Result<Option<String>> {
    let mut lines = request.lines();
    let parts: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if parts.len() != 3
        || parts[0] != "GET"
        || !parts[1].starts_with('/')
        || parts[1].starts_with("//")
    {
        bail!("Invalid callback method");
    }
    let hosts: Vec<_> = lines
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
        .collect();
    let expected_host = format!("127.0.0.1:{}", redirect.port().unwrap());
    if hosts.len() != 1 || hosts[0].1.trim() != expected_host {
        bail!("Invalid callback host");
    }
    let url = redirect.join(parts[1])?;
    if url.origin() != redirect.origin()
        || url.path() != redirect.path()
        || url.fragment().is_some()
    {
        bail!("Invalid callback path");
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let values = |key: &str| {
        pairs
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_ref())
            .collect::<Vec<_>>()
    };
    if values("state") != [state] {
        bail!("OAuth state mismatch");
    }
    let errors = values("error");
    let codes = values("code");
    if errors.len() == 1 && codes.is_empty() {
        return Ok(None);
    }
    if !errors.is_empty() || codes.len() != 1 || codes[0].is_empty() {
        bail!("Missing or ambiguous authorization code");
    }
    Ok(Some(codes[0].into()))
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Result\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn token_request(client: &Client, origin: &Url, payload: Value) -> Result<Value> {
    let response = client
        .post(origin.join("oauth/tokens")?)
        .json(&payload)
        .send()
        .context("OAuth token request failed; retry login if token delivery is uncertain")?;
    if !response.status().is_success() {
        bail!(
            "OAuth token endpoint returned HTTP {}; check the public client configuration or log in again",
            response.status()
        );
    }
    let value: Value = response.json().context("Invalid OAuth token response")?;
    Ok(value)
}

pub fn refresh(client: &Client, session: &mut Session) -> Result<()> {
    if session.refresh_expires_at.is_some_and(|t| t <= now()) {
        bail!("Refresh token expired; run `zd-support login-oauth`");
    }
    let refresh_token = session
        .refresh_token
        .as_deref()
        .context("No refresh token; run `zd-support login-oauth`")?;
    let value = token_request(
        client,
        &Url::parse(&session.oauth_origin)?,
        json!({"grant_type": "refresh_token", "refresh_token": refresh_token, "client_id": session.client_id}),
    )?;
    apply_tokens(session, &value)
}

fn apply_tokens(session: &mut Session, value: &Value) -> Result<()> {
    let access = value["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("Missing access token")?;
    if !value["token_type"]
        .as_str()
        .is_some_and(|t| t.eq_ignore_ascii_case("bearer"))
    {
        bail!("Unsupported OAuth token type");
    }
    session.access_token = access.into();
    session.expires_at = value["expires_in"]
        .as_u64()
        .map(|t| now().saturating_add(t));
    if let Some(refresh) = value["refresh_token"].as_str().filter(|s| !s.is_empty()) {
        session.refresh_token = Some(refresh.into());
        session.refresh_expires_at = value["refresh_token_expires_in"]
            .as_u64()
            .map(|t| now().saturating_add(t));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
