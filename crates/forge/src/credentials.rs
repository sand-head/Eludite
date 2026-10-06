//! Where tokens live: the operating system's credential store through `keyring` (Secret Service on Linux, Keychain
//! on macOS, Credential Manager on Windows), or, only when that store is unavailable and the person agreed in the
//! sign-in dialog, a file only they can read (mode 0600) under Eludite's config directory. Nothing else ever holds
//! a token: not the cache, not a log, not an audit entry, not a command's output. [`MemoryStore`] is the fake the
//! tests use.
//!
//! The key a credential is kept under is any string, not only a forge's host: brief 0060 keeps an OpenAI-compatible
//! server's API key under `provider:<name>` (`eludite_acp::settings::provider_credential_key`), as a [`Credential`]
//! with [`Family::None`] and [`SignInMethod::Token`], through the same stores and the same consent rule for the file.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorKind, ForgeError, Result};
use crate::model::{Account, Family};

/// A token. Its `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The token, for the one place that sends it: the request's `Authorization` header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<hidden>)")
    }
}

/// How the person signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInMethod {
    Device,
    Token,
    AppPassword,
    Cli,
}

impl SignInMethod {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "device" => SignInMethod::Device,
            "token" => SignInMethod::Token,
            "app_password" => SignInMethod::AppPassword,
            "cli" => SignInMethod::Cli,
            _ => return None,
        })
    }
}

/// What a host's stored credential is: the token and what goes with it (the account it belongs to, the method;
/// Tangled's session and personal data server; a refresh token).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub family: Family,
    pub token: Secret,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<Secret>,
    pub method: SignInMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    /// Tangled: the account's DID and personal data server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub did: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pds: Option<String>,
    /// Azure DevOps: a personal access token goes as Basic, an OAuth token as Bearer.
    #[serde(default)]
    pub basic: bool,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("family", &self.family)
            .field("method", &self.method)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// Which store keeps a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    Keyring,
    File,
    Memory,
}

impl StoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreKind::Keyring => "keyring",
            StoreKind::File => "file",
            StoreKind::Memory => "memory",
        }
    }
}

/// A place credentials live, by host.
pub trait CredentialStore: Send + Sync {
    fn kind(&self) -> StoreKind;
    /// Whether the store can be used at all (the Secret Service answers; the file's folder exists or can).
    fn available(&self) -> bool;
    fn get(&self, host: &str) -> Result<Option<Credential>>;
    fn set(&self, host: &str, credential: &Credential) -> Result<()>;
    fn delete(&self, host: &str) -> Result<()>;
    /// The hosts with a credential, where the store can list them.
    fn hosts(&self) -> Vec<String>;
}

/// The service name entries are kept under.
pub const SERVICE: &str = "eludite-forge";

/// The operating system's credential store.
#[derive(Debug, Default)]
pub struct KeyringStore {
    /// Hosts set in this process (the Secret Service cannot list ours without a search).
    known: Mutex<Vec<String>>,
}

fn store_error(e: impl std::fmt::Display) -> ForgeError {
    ForgeError::new(
        ErrorKind::Other,
        format!("the credential store answered: {e}"),
    )
}

impl CredentialStore for KeyringStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Keyring
    }

    fn available(&self) -> bool {
        keyring::Entry::store_status().is_ok()
    }

    fn get(&self, host: &str) -> Result<Option<Credential>> {
        let entry = keyring::Entry::new(SERVICE, host).map_err(store_error)?;
        match entry.get_password() {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|_| store_error("an entry Eludite cannot read")),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error(e)),
        }
    }

    fn set(&self, host: &str, credential: &Credential) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, host).map_err(store_error)?;
        let text = serde_json::to_string(credential).expect("serializes");
        entry.set_password(&text).map_err(store_error)?;
        let mut k = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if !k.iter().any(|h| h == host) {
            k.push(host.to_owned());
        }
        Ok(())
    }

    fn delete(&self, host: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, host).map_err(store_error)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(store_error(e)),
        }
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|h| h != host);
        Ok(())
    }

    fn hosts(&self) -> Vec<String> {
        self.known.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The fallback: `forge-credentials.json` under Eludite's config directory, mode 0600, used only after the person
/// agreed (the sign-in dialog's check box) because the credential store was unavailable.
#[derive(Debug)]
pub struct FileStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    /// `<config dir>/eludite/forge-credentials.json`.
    pub fn in_config_dir() -> Option<Self> {
        config_dir().map(|d| Self::new(d.join("eludite").join("forge-credentials.json")))
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn read(&self) -> BTreeMap<String, Credential> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write(&self, map: &BTreeMap<String, Credential>) -> Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(store_error)?;
        }
        let bytes = serde_json::to_vec_pretty(map).expect("serializes");
        let tmp = self.path.with_extension("tmp");
        write_private(&tmp, &bytes).map_err(store_error)?;
        std::fs::rename(&tmp, &self.path).map_err(store_error)
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    // The file lives in the user's own profile (`%APPDATA%`), which only they can read by default.
    std::fs::write(path, bytes)
}

/// Eludite's config directory's parent, as the settings store finds it.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(x));
    }
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(PathBuf::from);
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        Some(home.join("Library").join("Application Support"))
    } else {
        Some(home.join(".config"))
    }
}

impl CredentialStore for FileStore {
    fn kind(&self) -> StoreKind {
        StoreKind::File
    }

    fn available(&self) -> bool {
        true
    }

    fn get(&self, host: &str) -> Result<Option<Credential>> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read().remove(host))
    }

    fn set(&self, host: &str, credential: &Credential) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = self.read();
        m.insert(host.to_owned(), credential.clone());
        self.write(&m)
    }

    fn delete(&self, host: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = self.read();
        if m.remove(host).is_some() {
            if m.is_empty() {
                let _ = std::fs::remove_file(&self.path);
                return Ok(());
            }
            self.write(&m)?;
        }
        Ok(())
    }

    fn hosts(&self) -> Vec<String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read().into_keys().collect()
    }
}

/// Credentials in memory: the tests' fake store (and a store that is "unavailable" on demand).
#[derive(Debug, Default)]
pub struct MemoryStore {
    map: Mutex<BTreeMap<String, Credential>>,
    unavailable: std::sync::atomic::AtomicBool,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the store answer unavailable (to test the file fallback).
    pub fn set_unavailable(&self, v: bool) {
        self.unavailable
            .store(v, std::sync::atomic::Ordering::SeqCst);
    }
}

impl CredentialStore for MemoryStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Memory
    }

    fn available(&self) -> bool {
        !self.unavailable.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn get(&self, host: &str) -> Result<Option<Credential>> {
        Ok(self
            .map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(host)
            .cloned())
    }

    fn set(&self, host: &str, credential: &Credential) -> Result<()> {
        if !self.available() {
            return Err(store_error("unavailable"));
        }
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(host.to_owned(), credential.clone());
        Ok(())
    }

    fn delete(&self, host: &str) -> Result<()> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(host);
        Ok(())
    }

    fn hosts(&self) -> Vec<String> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

/// The primary store and the file fallback, as the hub sees them: a credential is read from the primary, then the
/// file; it is written to the primary, or to the file only with the person's consent when the primary is
/// unavailable.
pub struct Credentials {
    pub primary: Box<dyn CredentialStore>,
    pub fallback: Option<Box<dyn CredentialStore>>,
}

impl Credentials {
    pub fn new(
        primary: Box<dyn CredentialStore>,
        fallback: Option<Box<dyn CredentialStore>>,
    ) -> Self {
        Self { primary, fallback }
    }

    /// The real stores: the operating system's and the 0600 file.
    pub fn system() -> Self {
        Self::new(
            Box::new(KeyringStore::default()),
            FileStore::in_config_dir().map(|f| Box::new(f) as Box<dyn CredentialStore>),
        )
    }

    pub fn get(&self, host: &str) -> Option<(Credential, StoreKind)> {
        if self.primary.available()
            && let Ok(Some(c)) = self.primary.get(host)
        {
            return Some((c, self.primary.kind()));
        }
        let f = self.fallback.as_ref()?;
        f.get(host).ok().flatten().map(|c| (c, f.kind()))
    }

    /// Store `c` for `host`. Without the primary store, the file is used only when `allow_file` (the person agreed);
    /// else the answer says the store is unavailable and the dialog asks.
    pub fn set(&self, host: &str, c: &Credential, allow_file: bool) -> Result<StoreKind> {
        if self.primary.available() && self.primary.set(host, c).is_ok() {
            if let Some(f) = &self.fallback {
                let _ = f.delete(host);
            }
            return Ok(self.primary.kind());
        }
        match &self.fallback {
            Some(f) if allow_file => {
                f.set(host, c)?;
                Ok(f.kind())
            }
            _ => Err(ForgeError::new(
                ErrorKind::Other,
                format!(
                    "store_unavailable: the operating system's credential store is unavailable; keep the token for \
                     {host} in a file only you can read (mode 0600) instead?"
                ),
            )
            .with_host(host)),
        }
    }

    pub fn delete(&self, host: &str) -> Result<()> {
        if self.primary.available() {
            self.primary.delete(host)?;
        }
        if let Some(f) = &self.fallback {
            f.delete(host)?;
        }
        Ok(())
    }

    pub fn hosts(&self) -> Vec<(String, StoreKind)> {
        let mut out: Vec<(String, StoreKind)> = Vec::new();
        if self.primary.available() {
            out.extend(
                self.primary
                    .hosts()
                    .into_iter()
                    .map(|h| (h, self.primary.kind())),
            );
        }
        if let Some(f) = &self.fallback {
            for h in f.hosts() {
                if !out.iter().any(|(x, _)| *x == h) {
                    out.push((h, f.kind()));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(token: &str) -> Credential {
        Credential {
            family: Family::GitHub,
            token: Secret::new(token),
            refresh: None,
            method: SignInMethod::Token,
            account: Some(Account {
                login: "octocat".into(),
                ..Default::default()
            }),
            did: None,
            pds: None,
            basic: false,
        }
    }

    #[test]
    fn secrets_never_show() {
        let c = cred("ghp_very_secret");
        assert!(!format!("{c:?}").contains("very_secret"));
        assert!(!format!("{:?}", c.token).contains("very_secret"));
    }

    #[test]
    fn the_file_is_used_only_with_consent_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let primary = MemoryStore::new();
        primary.set_unavailable(true);
        let file = FileStore::new(dir.path().join("eludite/forge-credentials.json"));
        let creds = Credentials::new(Box::new(primary), Some(Box::new(file)));
        let e = creds.set("github.com", &cred("t1"), false).unwrap_err();
        assert!(e.message.starts_with("store_unavailable"), "{e}");
        assert!(!dir.path().join("eludite/forge-credentials.json").exists());
        assert_eq!(
            creds.set("github.com", &cred("t1"), true).unwrap(),
            StoreKind::File
        );
        let path = dir.path().join("eludite/forge-credentials.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let (c, kind) = creds.get("github.com").unwrap();
        assert_eq!(kind, StoreKind::File);
        assert_eq!(c.token.expose(), "t1");
        assert_eq!(
            creds.hosts(),
            vec![("github.com".to_owned(), StoreKind::File)]
        );
        creds.delete("github.com").unwrap();
        assert!(creds.get("github.com").is_none());
        assert!(
            !path.exists(),
            "the last credential removed removes the file"
        );
    }

    #[test]
    fn a_key_that_is_not_a_host_round_trips() {
        // Brief 0060: an OpenAI-compatible server's key under `provider:<name>`.
        let dir = tempfile::tempdir().unwrap();
        let primary = MemoryStore::new();
        let file = FileStore::new(dir.path().join("f.json"));
        let creds = Credentials::new(Box::new(primary), Some(Box::new(file)));
        let key = Credential {
            family: Family::None,
            account: None,
            ..cred("sk-local")
        };
        for name in ["provider:llama.cpp", "provider:My server / 2"] {
            assert_eq!(creds.set(name, &key, false).unwrap(), StoreKind::Memory);
            let (c, kind) = creds.get(name).unwrap();
            assert_eq!((c.token.expose(), kind), ("sk-local", StoreKind::Memory));
            assert_eq!(c.family, Family::None);
        }
        assert!(creds.get("provider:other").is_none());
        creds.delete("provider:llama.cpp").unwrap();
        assert!(creds.get("provider:llama.cpp").is_none());
        assert!(creds.get("provider:My server / 2").is_some());
        // The file fallback keeps it the same way, with consent.
        let unavailable = MemoryStore::new();
        unavailable.set_unavailable(true);
        let creds = Credentials::new(
            Box::new(unavailable),
            Some(Box::new(FileStore::new(dir.path().join("g.json")))),
        );
        assert!(creds.set("provider:x", &key, false).is_err());
        assert_eq!(
            creds.set("provider:x", &key, true).unwrap(),
            StoreKind::File
        );
        assert_eq!(
            creds.get("provider:x").unwrap().0.token.expose(),
            "sk-local"
        );
    }

    #[test]
    fn the_primary_store_wins_and_clears_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileStore::new(dir.path().join("f.json"));
        file.set("gitlab.com", &cred("old")).unwrap();
        let creds = Credentials::new(Box::new(MemoryStore::new()), Some(Box::new(file)));
        assert_eq!(creds.get("gitlab.com").unwrap().1, StoreKind::File);
        assert_eq!(
            creds.set("gitlab.com", &cred("new"), false).unwrap(),
            StoreKind::Memory
        );
        let (c, kind) = creds.get("gitlab.com").unwrap();
        assert_eq!((c.token.expose(), kind), ("new", StoreKind::Memory));
        assert!(!dir.path().join("f.json").exists());
    }
}
