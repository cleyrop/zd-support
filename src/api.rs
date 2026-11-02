use crate::{
    oauth,
    session::{Session, Storage},
};
use anyhow::{Context, Result, bail};
use reqwest::{Method, blocking::Client};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashSet};
use url::Url;

pub struct Api<'a> {
    client: Client,
    session: RefCell<Session>,
    portal: Url,
    storage: &'a Storage,
    persist: bool,
}

impl<'a> Api<'a> {
    pub fn new(storage: &'a Storage, session: Session) -> Result<Self> {
        Ok(Self {
            client: oauth::client()?,
            portal: Url::parse(&session.portal)?,
            session: RefCell::new(session),
            storage,
            persist: false,
        })
    }
    pub fn load(storage: &'a Storage, portal: &Url) -> Result<Self> {
        let mut api = Self::new(storage, storage.load(portal)?)?;
        api.persist = true;
        Ok(api)
    }
    pub fn save(&self) -> Result<()> {
        self.storage.save(&self.session.borrow())
    }
    fn token(&self, force_refresh: bool) -> Result<String> {
        let mut session = self.session.borrow_mut();
        if force_refresh
            || session
                .expires_at
                .is_some_and(|t| t <= oauth::now().saturating_add(30))
        {
            oauth::refresh(&self.client, &mut session)?;
            // Persist rotated tokens before making any resource request.
            if self.persist {
                self.storage
                    .save(&session)
                    .context("Cannot save refreshed credentials; log in again")?;
            }
        }
        Ok(session.access_token.clone())
    }

    fn url(&self, path: &str) -> Result<Url> {
        let url = self.portal.join(path)?;
        let allowed = url.path() == "/api/v2/requests.json"
            || url.path() == "/api/v2/requests"
            || url.path().starts_with("/api/v2/requests/")
            || url.path() == "/api/v2/users/me.json";
        if url.origin() != self.portal.origin()
            || !allowed
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            bail!("Refusing API URL outside this portal's Requests API");
        }
        Ok(url)
    }

    fn current_user(&self) -> Result<Value> {
        let value = self.request("GET", "/api/v2/users/me.json", None)?;
        if !value["user"]["id"].is_u64() || value["user"]["role"] == "anonymous" {
            bail!("Session expired or anonymous; run `zd-support login`");
        }
        Ok(value)
    }

    pub fn verify(&self) -> Result<Value> {
        let value = self.current_user()?;
        let user = &value["user"];
        // Do not expose the authenticity token, even in --json output.
        Ok(
            json!({"user": {"id": user["id"], "name": user["name"], "email": user["email"], "role": user["role"], "verified": user["verified"]}}),
        )
    }

    pub fn authenticate_and_save(&self) -> Result<Value> {
        let user = self.verify()?;
        // Users/Me also accepts anonymous access. Confirm that Requests is accessible.
        let requests = self.request("GET", "/api/v2/requests.json?page[size]=1", None)?;
        if !requests["requests"].is_array() {
            bail!("Unexpected Requests response; credentials were not saved");
        }
        self.save()?;
        Ok(user)
    }

    pub fn ticket(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let value = self.request(method, path, body)?;
        let expected_id = self
            .url(path)?
            .path()
            .rsplit('/')
            .next()
            .and_then(|s| s.trim_end_matches(".json").parse::<u64>().ok());
        let id = value["request"]["id"].as_u64().filter(|id| *id > 0);
        if id.is_none() || expected_id.is_some_and(|expected| id != Some(expected)) {
            let hint = if method == "GET" {
                ""
            } else {
                "; delivery is unknown. Check the ticket before retrying; no automatic retry was made"
            };
            bail!("Unexpected API response: missing or mismatched request ID{hint}");
        }
        Ok(value)
    }

    pub fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = self.url(path)?;
        let basic = self.session.borrow().basic.clone();
        // A tenant may treat disabled password access as anonymous. Never post anonymously.
        if method != "GET" && basic.is_some() {
            self.current_user()?;
        }
        let mut token = if basic.is_none() {
            self.token(false)?
        } else {
            String::new()
        };
        let mut attempts = 0;
        let response = loop {
            let mut request = self
                .client
                .request(Method::from_bytes(method.as_bytes())?, url.clone())
                .header("Accept", "application/json");
            request = if let Some(credentials) = &basic {
                request.basic_auth(&credentials.email, Some(&credentials.password))
            } else {
                request.bearer_auth(&token)
            };
            if let Some(body) = &body {
                request = request.json(body);
            }
            let response = request.send().with_context(|| if method == "GET" {
                "API request failed"
            } else {
                "Network failure: delivery is unknown. Check the ticket before retrying; no automatic retry was made"
            })?;
            // Only safe reads may be replayed after a 401. Writes are never replayed.
            if response.status().as_u16() == 401
                && basic.is_none()
                && method == "GET"
                && attempts == 0
            {
                token = self.token(true)?;
                attempts += 1;
                continue;
            }
            break response;
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unspecified")
            .to_owned();
        if status.as_u16() == 401 {
            if basic.is_some() {
                bail!(
                    "HTTP 401: support credentials rejected or password API access disabled by OUTSCALE. Check your Zendesk support email/password; this response alone does not identify the cause"
                );
            }
            bail!("HTTP 401: session expired; run `zd-support login`");
        }
        if status.as_u16() == 403 {
            if basic.is_some() {
                bail!(
                    "HTTP 403: access denied. Check email verification, account permissions and whether OUTSCALE enables password API access for end users"
                );
            }
            bail!(
                "HTTP 403: access denied (permissions, OAuth scopes or unverified email). Try login again"
            );
        }
        if status.as_u16() == 429 {
            bail!("HTTP 429: rate limited; Retry-After: {retry_after}");
        }
        if status.is_redirection() {
            bail!("HTTP {status}: redirect refused; run login again or check --portal");
        }
        let uncertain = if method != "GET" {
            "; delivery is unknown. Check the ticket before retrying; no automatic retry was made"
        } else {
            ""
        };
        let value: Value = response.json().with_context(|| format!("HTTP {status}: expected a JSON API response (possible login page or proxy challenge){uncertain}"))?;
        if !status.is_success() {
            let detail = serde_json::to_string(&value)?;
            let detail: String = detail.chars().take(1500).collect();
            let hint = if status.is_server_error() {
                uncertain
            } else {
                ""
            };
            bail!("HTTP {status}: {detail}{hint}");
        }
        Ok(value)
    }

    pub fn collection(&self, path: &str, key: &str, all: bool) -> Result<Value> {
        let mut next = Some(path.to_owned());
        let mut items = Vec::new();
        let mut users = Vec::new();
        let mut user_ids = HashSet::new();
        let mut seen = HashSet::new();
        while let Some(path) = next.take() {
            let url = self.url(&path)?;
            if !seen.insert(url.to_string()) {
                bail!("Pagination loop detected");
            }
            let page = self.request("GET", url.as_str(), None)?;
            items.extend(
                page[key]
                    .as_array()
                    .context("Unexpected collection response")?
                    .iter()
                    .cloned(),
            );
            if let Some(page_users) = page["users"].as_array() {
                for user in page_users {
                    if let Some(id) = user["id"].as_u64()
                        && user_ids.insert(id)
                    {
                        users.push(user.clone());
                    }
                }
            }
            next = next_page(&page)?;
            if !all {
                break;
            }
        }
        let mut result = json!({key: items, "next_page": next});
        if !users.is_empty() {
            result["users"] = json!(users);
        }
        Ok(result)
    }
}

fn next_page(page: &Value) -> Result<Option<String>> {
    if page["meta"]["has_more"] == false {
        return Ok(None);
    }
    let next = page["links"]["next"]
        .as_str()
        .or_else(|| page["next_page"].as_str());
    if page["meta"]["has_more"] == true && next.is_none() {
        bail!("API reports more results without a next-page URL");
    }
    Ok(next.map(str::to_owned))
}

#[cfg(test)]
mod tests;
