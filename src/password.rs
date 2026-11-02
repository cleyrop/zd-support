use crate::session::Session;
use anyhow::{Context, Result, bail};
use std::io::{self, IsTerminal, Read, Write};
use url::Url;

pub fn login(portal: &Url, email: Option<String>, password_stdin: bool) -> Result<Session> {
    let email = match email {
        Some(email) => email,
        None => {
            if !io::stdin().is_terminal() {
                bail!("Non-interactive login requires --email and --password-stdin");
            }
            eprint!("Support email: ");
            io::stderr().flush()?;
            let mut email = String::new();
            io::stdin().read_line(&mut email)?;
            email
        }
    };
    let password = if password_stdin {
        if io::stdin().is_terminal() {
            bail!("--password-stdin expects a pipe; omit it for a hidden password prompt");
        }
        let mut value = String::new();
        io::stdin().read_to_string(&mut value)?;
        // Remove the single line ending emitted by secret managers, preserving spaces.
        if value.ends_with('\n') {
            value.pop();
            if value.ends_with('\r') {
                value.pop();
            }
        }
        value
    } else {
        rpassword::prompt_password("Support password: ").context(
            "Cannot read password; run login in a terminal or use --email with --password-stdin",
        )?
    };
    Session::basic(portal, email, password)
}
