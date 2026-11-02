mod api;
mod conversation;
mod oauth;
mod password;
mod session;
#[cfg(test)]
mod test_support;

use anyhow::{Context, Result, bail};
use clap::{Args, ColorChoice, Parser, Subcommand, ValueEnum};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    thread,
    time::Duration,
};
use url::Url;

#[derive(Parser)]
#[command(version, about = "Zendesk support tickets via the Requests API")]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "ZD_SUPPORT_URL",
        default_value = "https://support.outscale.com"
    )]
    portal: String,
    /// Machine-readable JSON on stdout; diagnostics stay on stderr.
    #[arg(long, global = true)]
    json: bool,
    /// Color conversation output (auto respects NO_COLOR and redirected stdout).
    #[arg(long, global = true, value_enum, default_value = "auto")]
    color: ColorChoice,
    /// Private directory containing the session (default: OS application data directory).
    #[arg(long, global = true, env = "ZD_SUPPORT_STATE_DIR")]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in with your support email and password (no OAuth client required).
    Login {
        #[arg(long, env = "ZD_SUPPORT_EMAIL")]
        email: Option<String>,
        /// Read the password from a pipe instead of prompting (requires --email).
        #[arg(long, requires = "email")]
        password_stdin: bool,
    },
    /// Sign in with OAuth PKCE in your default browser (public Zendesk OAuth client).
    LoginOauth {
        /// OAuth client unique identifier supplied by your Zendesk administrator.
        #[arg(long, env = "ZD_SUPPORT_CLIENT_ID")]
        client_id: String,
        /// Core Zendesk subdomain, required when --portal is a custom domain.
        #[arg(long, env = "ZD_SUPPORT_SUBDOMAIN")]
        zendesk_subdomain: Option<String>,
        #[arg(
            long,
            env = "ZD_SUPPORT_REDIRECT_URI",
            default_value = "http://127.0.0.1:8765/callback"
        )]
        redirect_uri: String,
        #[arg(long, env = "ZD_SUPPORT_SCOPE", default_value = "read write")]
        scope: String,
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(10..=3600))]
        timeout: u64,
    },
    /// Check the session and show the current user (no secrets).
    Whoami,
    /// Delete this CLI's local session. Does not sign out other browsers.
    Logout,
    /// List requests in a table, excluding closed tickets by default.
    List {
        /// Filter to one status, overriding the default exclusion of closed tickets.
        #[arg(long, value_enum)]
        status: Option<Status>,
        /// Include closed tickets as well as all other statuses.
        #[arg(long, conflicts_with = "status")]
        include_closed: bool,
        /// Retrieve every page instead of the first 50 requests.
        #[arg(long)]
        all: bool,
    },
    /// Read a request and its current status.
    Show { id: u64 },
    /// Read all public comments on a request.
    Comments { id: u64 },
    /// Open a new request (sends it immediately).
    Create {
        #[arg(long)]
        subject: String,
        #[command(flatten)]
        message: Message,
    },
    /// Add a public reply (sends it immediately).
    Reply {
        id: u64,
        #[command(flatten)]
        message: Message,
    },
    /// Mark your request as solved, when Zendesk permits it.
    Solve { id: u64 },
    /// Poll a request; emit its state initially and whenever it changes. Ctrl-C to stop.
    Watch {
        id: u64,
        #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(10..))]
        interval: u64,
    },
}

#[derive(Clone, ValueEnum)]
enum Status {
    New,
    Open,
    Pending,
    Hold,
    Solved,
    Closed,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct Message {
    #[arg(long)]
    body: Option<String>,
    /// Read UTF-8 text from a file, or from stdin with '-'.
    #[arg(long)]
    body_file: Option<PathBuf>,
}

impl Message {
    fn read(&self) -> Result<String> {
        let body = match (&self.body, &self.body_file) {
            (Some(body), None) => body.clone(),
            (None, Some(path)) if path.as_os_str() == "-" => read_message(io::stdin().lock())?,
            (None, Some(path)) => read_message(
                std::fs::File::open(path)
                    .with_context(|| format!("Cannot read {}", path.display()))?,
            )?,
            _ => bail!("Specify exactly one of --body or --body-file"),
        };
        if body.trim().is_empty() {
            bail!("Message must not be empty");
        }
        if body.len() > MAX_MESSAGE_BYTES {
            bail!(
                "Message exceeds {MAX_MESSAGE_BYTES} UTF-8 bytes; split it into smaller replies to avoid Zendesk truncation. Nothing was sent"
            );
        }
        Ok(body)
    }
}

// Conservative limit below Zendesk's 64 kB comment limit (including UTF-8).
const MAX_MESSAGE_BYTES: usize = 64_000;

fn read_message(reader: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        bail!(
            "Message exceeds {MAX_MESSAGE_BYTES} UTF-8 bytes; split it into smaller replies to avoid Zendesk truncation. Nothing was sent"
        );
    }
    String::from_utf8(bytes).context("Message must be valid UTF-8")
}

fn portal_url(input: &str) -> Result<Url> {
    let mut url = Url::parse(input).context("Invalid portal URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        bail!("--portal must be an HTTPS origin, e.g. https://support.outscale.com");
    }
    url.set_path("/");
    Ok(url)
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        // A consumer such as `head` may intentionally stop reading early.
        if error
            .downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
        {
            return;
        }
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let portal = portal_url(&cli.portal)?;
    run_with_portal(cli, portal)
}

fn run_with_portal(cli: Cli, portal: Url) -> Result<()> {
    let is_write = matches!(
        &cli.command,
        Command::Create { .. } | Command::Reply { .. } | Command::Solve { .. }
    );
    let storage = session::Storage::new(cli.state_dir, &portal)?;
    if let Command::Watch { id, interval } = &cli.command {
        let mut previous = Value::Null;
        loop {
            // Release the lock between polls so another command can reply.
            let value = {
                let _lock = storage.lock()?;
                let api = api::Api::load(&storage, &portal)?;
                api.ticket("GET", &format!("/api/v2/requests/{id}.json"), None)?
            };
            if value != previous {
                if cli.json {
                    writeln!(io::stdout(), "{}", serde_json::to_string(&value)?)?;
                } else {
                    display(&value, false, cli.color)?;
                }
                io::stdout().flush()?;
                previous = value;
            }
            thread::sleep(Duration::from_secs(*interval));
        }
    }
    let _lock = storage.lock()?;
    let value = match cli.command {
        Command::Login {
            email,
            password_stdin,
        } => {
            let session = password::login(&portal, email, password_stdin)?;
            api::Api::new(&storage, session)?.authenticate_and_save()?
        }
        Command::LoginOauth {
            client_id,
            zendesk_subdomain,
            redirect_uri,
            scope,
            timeout,
        } => {
            let origin = oauth::origin(&portal, zendesk_subdomain.as_deref())?;
            let session = oauth::login(
                &portal,
                &origin,
                &client_id,
                &redirect_uri,
                &scope,
                Duration::from_secs(timeout),
            )?;
            // Verify the token before replacing a valid session.
            let api = api::Api::new(&storage, session)?;
            api.authenticate_and_save()?
        }
        Command::Logout => {
            storage.remove()?;
            json!({"logged_out": true})
        }
        command => {
            let api = api::Api::load(&storage, &portal)?;
            match command {
                Command::Whoami => api.verify()?,
                Command::List {
                    status,
                    include_closed,
                    all,
                } => {
                    let mut url = portal.join("api/v2/requests.json")?;
                    // Requests ignores sort_by/sort_order with cursor pagination.
                    url.query_pairs_mut()
                        .append_pair("per_page", "50")
                        .append_pair("sort_by", "updated_at")
                        .append_pair("sort_order", "desc");
                    if let Some(status) = status {
                        url.query_pairs_mut()
                            .append_pair("status", status.to_possible_value().unwrap().get_name());
                    } else if !include_closed {
                        // Filter server-side, before pagination, so closed tickets cannot fill the first page.
                        url.query_pairs_mut()
                            .append_pair("status", "new,open,pending,hold,solved");
                    }
                    api.collection(url.as_str(), "requests", all)?
                }
                Command::Show { id } => {
                    api.ticket("GET", &format!("/api/v2/requests/{id}.json"), None)?
                }
                Command::Comments { id } => {
                    let mut path = format!("/api/v2/requests/{id}/comments.json");
                    if !cli.json {
                        path.push_str("?include=users");
                    }
                    let mut result = api.collection(&path, "comments", true)?;
                    if !cli.json {
                        result["request_id"] = json!(id);
                        match api.verify() {
                            Ok(me) => result["current_user"] = me["user"].clone(),
                            Err(_) => eprintln!(
                                "Impossible d’identifier le compte courant ; affichage des auteurs disponibles."
                            ),
                        }
                    }
                    result
                }
                Command::Create { subject, message } => {
                    if subject.trim().is_empty() {
                        bail!("Subject must not be empty");
                    }
                    let body = message.read()?;
                    api.ticket(
                        "POST",
                        "/api/v2/requests.json",
                        Some(json!({"request": {"subject": subject, "comment": {"body": body}}})),
                    )?
                }
                Command::Reply { id, message } => {
                    let body = message.read()?;
                    api.ticket(
                        "PUT",
                        &format!("/api/v2/requests/{id}.json"),
                        Some(json!({"request": {"comment": {"body": body}}})),
                    )?
                }
                Command::Solve { id } => {
                    let path = format!("/api/v2/requests/{id}.json");
                    let current = api.ticket("GET", &path, None)?;
                    if current["request"]["can_be_solved_by_me"] != true {
                        bail!("Zendesk does not allow you to solve this request");
                    }
                    api.ticket("PUT", &path, Some(json!({"request": {"solved": true}})))?
                }
                _ => unreachable!(),
            }
        }
    };
    let result = display(&value, cli.json, cli.color);
    if is_write {
        result.context("Zendesk accepted the write but output failed; check the ticket and do not resend the message")
    } else {
        result
    }
}

// Strip terminal control characters from untrusted ticket text.
fn text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

fn display(value: &Value, as_json: bool, color: ColorChoice) -> Result<()> {
    let mut out = io::stdout();
    if as_json {
        writeln!(out, "{}", serde_json::to_string_pretty(value)?)?;
    } else if let Some(requests) = value["requests"].as_array() {
        let mut table = Table::new();
        let width = terminal_size::terminal_size()
            .map(|(w, _)| w.0)
            .unwrap_or(120);
        table
            .load_preset(UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(width)
            .set_header(["ID", "STATUT", "MISE À JOUR (UTC)", "SUJET"]);
        for r in requests {
            table.add_row([
                table_text(&r["id"]),
                table_text(&r["status"]),
                table_text(&r["updated_at"])
                    .replace('T', " ")
                    .trim_end_matches('Z')
                    .to_owned(),
                table_text(&r["subject"]),
            ]);
        }
        writeln!(out, "{table}")?;
        if requests.is_empty() {
            writeln!(out, "Aucun ticket.")?;
        }
        if !value["next_page"].is_null() {
            eprintln!("More requests available; use --all.");
        }
    } else if value["comments"].is_array() {
        conversation::display(value, color)?;
    } else if let Some(r) = value.get("request") {
        conversation::display_request(r, color)?;
    } else if let Some(user) = value.get("user") {
        writeln!(
            out,
            "Connected: {} <{}> (id {})",
            text(&user["name"]),
            text(&user["email"]),
            user["id"]
        )?;
    } else {
        writeln!(out, "{}", serde_json::to_string_pretty(value)?)?;
    }
    Ok(())
}

fn table_text(value: &Value) -> String {
    text(value).chars()
        .filter(|c| !matches!(*c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_messages_that_zendesk_could_silently_truncate() {
        for body in ["x".repeat(65_536), "é".repeat(32_768)] {
            let message = Message {
                body: Some(body.clone()),
                body_file: None,
            };
            assert!(message.read().is_err());
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.write_all(body.as_bytes()).unwrap();
            let message = Message {
                body: None,
                body_file: Some(file.path().into()),
            };
            assert!(message.read().is_err());
        }
        assert_eq!(
            read_message("é".repeat(32_000).as_bytes()).unwrap().len(),
            MAX_MESSAGE_BYTES
        );
        assert!(read_message("é".repeat(32_001).as_bytes()).is_err());
    }

    #[test]
    fn cli_create_reply_and_solve_send_the_expected_requests_once() {
        use crate::test_support::{Response, Server};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let server = Server::new(move |request, _| {
            let step = calls.fetch_add(1, Ordering::Relaxed);
            if matches!(step, 0 | 2 | 5) {
                assert_eq!(request.path, "/api/v2/users/me.json");
                assert_eq!(request.method, "GET");
                return Response::json(json!({"user": {"id": 7, "role": "end-user"}}));
            }
            match step {
                1 => {
                    assert_eq!(request.method, "POST");
                    assert_eq!(request.path, "/api/v2/requests.json");
                    assert_eq!(
                        serde_json::from_slice::<Value>(&request.body).unwrap(),
                        json!({"request": {"subject": "Incident", "comment": {"body": "Diagnostic\né"}}})
                    );
                }
                3 => {
                    assert_eq!(request.method, "PUT");
                    assert_eq!(request.path, "/api/v2/requests/42.json");
                    assert_eq!(
                        serde_json::from_slice::<Value>(&request.body).unwrap(),
                        json!({"request": {"comment": {"body": "Réponse"}}})
                    );
                }
                4 => {
                    assert_eq!(request.method, "GET");
                    assert_eq!(request.path, "/api/v2/requests/42.json");
                }
                6 => {
                    assert_eq!(request.method, "PUT");
                    assert_eq!(request.path, "/api/v2/requests/42.json");
                    assert_eq!(
                        serde_json::from_slice::<Value>(&request.body).unwrap(),
                        json!({"request": {"solved": true}})
                    );
                }
                _ => panic!("Unexpected extra API call"),
            }
            Response::json(
                json!({"request": {"id": 42, "status": "open", "can_be_solved_by_me": true}}),
            )
        });
        let temp = tempfile::tempdir().unwrap();
        let storage = session::Storage::new(Some(temp.path().into()), &server.url).unwrap();
        storage
            .save(
                &session::Session::basic(&server.url, "test@example.com".into(), "fake".into())
                    .unwrap(),
            )
            .unwrap();
        let reply = temp.path().join("reply.txt");
        std::fs::write(&reply, "Réponse").unwrap();
        for args in [
            vec!["create", "--subject", "Incident", "--body", "Diagnostic\né"],
            vec!["reply", "42", "--body-file", reply.to_str().unwrap()],
            vec!["solve", "42"],
        ] {
            let cli = Cli::try_parse_from(
                [
                    "zd-support",
                    "--json",
                    "--state-dir",
                    temp.path().to_str().unwrap(),
                ]
                .into_iter()
                .chain(args),
            )
            .unwrap();
            run_with_portal(cli, server.url.clone()).unwrap();
        }
        assert_eq!(count.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn cli_solve_refuses_when_zendesk_disallows_it() {
        use crate::test_support::{Response, Server};
        let server = Server::new(|request, _| {
            assert_eq!(request.method, "GET");
            assert_eq!(request.path, "/api/v2/requests/42.json");
            Response::json(json!({"request": {"id": 42, "can_be_solved_by_me": false}}))
        });
        let temp = tempfile::tempdir().unwrap();
        let storage = session::Storage::new(Some(temp.path().into()), &server.url).unwrap();
        storage
            .save(
                &session::Session::basic(&server.url, "test@example.com".into(), "fake".into())
                    .unwrap(),
            )
            .unwrap();
        let cli = Cli::try_parse_from([
            "zd-support",
            "--state-dir",
            temp.path().to_str().unwrap(),
            "solve",
            "42",
        ])
        .unwrap();
        assert!(
            run_with_portal(cli, server.url.clone())
                .unwrap_err()
                .to_string()
                .contains("does not allow")
        );
    }
    #[test]
    fn rejects_unsafe_portals() {
        for url in [
            "http://support.outscale.com",
            "https://user:pw@support.outscale.com",
            "https://support.outscale.com/hc",
            "https://support.outscale.com/?token=x",
        ] {
            assert!(portal_url(url).is_err());
        }
        assert!(portal_url("https://support.outscale.com").is_ok());
    }
    #[test]
    fn requires_exactly_one_message_source() {
        assert!(Cli::try_parse_from(["zd-support", "reply", "1"]).is_err());
        assert!(
            Cli::try_parse_from([
                "zd-support",
                "reply",
                "1",
                "--body",
                "x",
                "--body-file",
                "x"
            ])
            .is_err()
        );
    }
}
