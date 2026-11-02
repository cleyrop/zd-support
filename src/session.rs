use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
};
use url::Url;

#[derive(Default, Serialize, Deserialize)]
pub struct Session {
    pub version: u8,
    pub portal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<BasicCredentials>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub oauth_origin: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub refresh_expires_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BasicCredentials {
    pub email: String,
    pub password: String,
}

impl Session {
    pub fn basic(portal: &Url, email: String, password: String) -> Result<Self> {
        let email = email.trim().to_owned();
        if !email.contains('@') || email.chars().any(|c| c.is_control() || c == ':') {
            bail!("Enter the email address of your Zendesk support account");
        }
        if password.is_empty() {
            bail!("Password must not be empty");
        }
        Ok(Self {
            version: 3,
            portal: portal.to_string(),
            basic: Some(BasicCredentials { email, password }),
            ..Default::default()
        })
    }
}

pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub fn new(root: Option<PathBuf>, portal: &Url) -> Result<Self> {
        let root = root.map(Ok).unwrap_or_else(|| {
            ProjectDirs::from("", "", "zd-support")
                .map(|d| d.data_local_dir().to_owned())
                .context("Cannot find application data directory; use --state-dir")
        })?;
        let name = format!(
            "{}_{}",
            portal.host_str().context("Missing host")?.replace(':', "_"),
            portal.port_or_known_default().unwrap_or(443)
        );
        let dir = root.join(name);
        fs::create_dir_all(&dir)?;
        if !fs::symlink_metadata(&dir)?.file_type().is_dir() {
            bail!("Session directory must be a real directory, not a symbolic link");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir })
    }

    pub fn lock(&self) -> Result<File> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(self.dir.join("session.lock"))?;
        private_file(&file)?;
        file.try_lock_exclusive()
            .context("Another command is using this portal session; retry when it finishes")?;
        Ok(file)
    }

    pub fn load(&self, portal: &Url) -> Result<Session> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(self.dir.join("session.json"))
            .context("No saved session; run `zd-support login`")?;
        private_file(&file)?;
        let session: Session =
            serde_json::from_reader(file).context("Invalid session file; run login again")?;
        if !matches!(session.version, 2 | 3) || session.portal != portal.as_str() {
            bail!("Incompatible session or portal mismatch; run login again");
        }
        Ok(session)
    }

    pub fn save(&self, session: &Session) -> Result<()> {
        // NamedTempFile is created with 0600 permissions on Unix; replace atomically.
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer(&mut file, session)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.dir.join("session.json"))
            .context("Cannot persist session")?;
        Ok(())
    }

    pub fn remove(&self) -> Result<()> {
        match fs::remove_file(self.dir.join("session.json")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn private_file(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("Session and lock files must be regular files");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "Session or lock file is accessible to other users; restrict its permissions to 0600 before retrying"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
