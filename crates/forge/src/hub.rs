//! The [`Hub`]: what the shell and the commands hold. It detects the workspace remote's forge with the settings,
//! builds signed-in [`Forge`]s from the credential store, answers reads from the cache at once while a refresh runs
//! on a background thread (stale-while-refreshing, deduplicated per key, dropped when the repository's generation
//! moved on), keeps the local pending reviews, and runs the device flows.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::auth::{self, DeviceStart};
use crate::cache::{Cache, Cached};
use crate::client::{Client, Limits};
use crate::credentials::{Credential, Credentials, StoreKind};
use crate::detect::{self, HostEntry};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::forge::Forge;
use crate::http::{Cancel, Transport};
use crate::model::*;
use crate::util::{now_secs, rfc3339};

/// The settings the hub reads (`forge.*`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HubConfig {
    pub hosts: Vec<HostEntry>,
    pub github_client_id: Option<String>,
    pub gitlab_application_id: Option<String>,
    pub azure_application_id: Option<String>,
}

/// The client ids Eludite is built with (`ELUDITE_GITHUB_CLIENT_ID` and the others at build time); the settings
/// override them. Empty when the build names none: the device flow is then unavailable.
pub const BUILT_GITHUB_CLIENT_ID: Option<&str> = option_env!("ELUDITE_GITHUB_CLIENT_ID");
pub const BUILT_GITLAB_APPLICATION_ID: Option<&str> = option_env!("ELUDITE_GITLAB_APPLICATION_ID");
pub const BUILT_AZURE_APPLICATION_ID: Option<&str> = option_env!("ELUDITE_AZURE_APPLICATION_ID");

/// Something a background refresh finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    /// `key`'s answer was refreshed (the windows read it again from the cache).
    Refreshed { scope: String, key: String },
    /// `key`'s refresh failed; the cache is kept.
    RefreshFailed {
        scope: String,
        key: String,
        error: ForgeError,
    },
    /// A device flow completed (or failed) for `host`.
    SignedIn {
        host: String,
        result: std::result::Result<Account, String>,
    },
}

/// The root url a version probe asks for a host.
pub type ProbeBase = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// A listener for [`HubEvent`]s (the shell wakes its windows on the UI thread from here).
pub type Listener = Arc<dyn Fn(&HubEvent) + Send + Sync>;

/// A read's answer: the value and where it came from (`forge-*.output.json`'s `stale`, `fetched_at`,
/// `age_seconds`, `refresh_error`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer<T> {
    #[serde(flatten)]
    pub value: T,
    pub stale: bool,
    pub fetched_at: String,
    pub age_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<ForgeError>,
}

impl<T> Answer<T> {
    fn fresh(value: T) -> Self {
        Self {
            value,
            stale: false,
            fetched_at: rfc3339(now_secs()),
            age_seconds: 0,
            refresh_error: None,
        }
    }

    fn cached(c: Cached<T>, error: Option<ForgeError>) -> Self {
        let age = c.age();
        Self {
            value: c.value,
            stale: true,
            fetched_at: rfc3339(c.fetched_at),
            age_seconds: age,
            refresh_error: error,
        }
    }

    /// Answer `f` of the value with the same provenance.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Answer<U> {
        Answer {
            value: f(self.value),
            stale: self.stale,
            fetched_at: self.fetched_at,
            age_seconds: self.age_seconds,
            refresh_error: self.refresh_error,
        }
    }
}

/// A device flow in progress for a host.
#[derive(Debug, Clone)]
pub struct DeviceFlow {
    pub start: DeviceStart,
    pub cancel: Cancel,
    pub family: Family,
}

/// The local pending review of one pull request (GitHub, Forgejo, Gitea).
#[derive(Debug, Clone, Default)]
pub struct Pending {
    pub comments: Vec<DraftComment>,
    /// Drafts kept on the forge (GitLab) added through Eludite this session.
    pub server_drafts: u64,
}

/// See the module docs.
pub struct Hub {
    transport: Arc<dyn Transport>,
    credentials: Credentials,
    config: RwLock<HubConfig>,
    cache: RwLock<Option<Cache>>,
    limits: Limits,
    probes: Mutex<HashMap<String, detect::Probed>>,
    /// The root url a probe asks (`https://<host>`; tests point it at the fixture server).
    probe_base: RwLock<Option<ProbeBase>>,
    generation: AtomicU64,
    in_flight: Mutex<HashSet<String>>,
    last_errors: Mutex<HashMap<String, ForgeError>>,
    pending: Mutex<HashMap<String, Pending>>,
    flows: Mutex<HashMap<String, DeviceFlow>>,
    listeners: Mutex<Vec<Listener>>,
    /// Background refreshes may run (tests turn them off to make a stale read deterministic).
    background: std::sync::atomic::AtomicBool,
    /// The `gh` and `glab` paths tests give (else PATH).
    cli: Mutex<HashMap<Family, std::path::PathBuf>>,
    /// How long a cached answer counts as fresh: read without a refresh, and how long after a refresh (done or
    /// failed) another background refresh of the same key waits. A window that redraws on each refresh would
    /// otherwise refresh forever.
    fresh_for: RwLock<std::time::Duration>,
    /// When each key's last background refresh ended.
    attempted: Mutex<HashMap<String, std::time::Instant>>,
}

/// [`Hub`]'s default freshness window.
pub const FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(30);

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

impl Hub {
    pub fn new(transport: Arc<dyn Transport>, credentials: Credentials) -> Arc<Self> {
        Arc::new(Self {
            transport,
            credentials,
            config: RwLock::new(HubConfig::default()),
            cache: RwLock::new(None),
            limits: Limits::default(),
            probes: Mutex::new(HashMap::new()),
            probe_base: RwLock::new(None),
            generation: AtomicU64::new(1),
            in_flight: Mutex::new(HashSet::new()),
            last_errors: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            flows: Mutex::new(HashMap::new()),
            listeners: Mutex::new(Vec::new()),
            background: std::sync::atomic::AtomicBool::new(true),
            cli: Mutex::new(HashMap::new()),
            fresh_for: RwLock::new(FRESH_FOR),
            attempted: Mutex::new(HashMap::new()),
        })
    }

    /// Set the freshness window (tests set zero: every cached answer is stale and refreshes).
    pub fn set_fresh_for(&self, d: std::time::Duration) {
        *self.fresh_for.write().unwrap_or_else(|e| e.into_inner()) = d;
    }

    fn fresh_for(&self) -> std::time::Duration {
        *self.fresh_for.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    pub fn set_config(&self, config: HubConfig) {
        let mut c = self.config.write().unwrap_or_else(|e| e.into_inner());
        if *c != config {
            *c = config;
            self.probes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        }
    }

    pub fn config(&self) -> HubConfig {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The workspace's cache (`None`: no workspace, nothing cached). A new workspace starts a new generation.
    pub fn set_cache(&self, cache: Option<Cache>) {
        *self.cache.write().unwrap_or_else(|e| e.into_inner()) = cache;
        self.bump();
    }

    pub fn cache(&self) -> Option<Cache> {
        self.cache.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_probe_base(&self, f: Option<ProbeBase>) {
        *self.probe_base.write().unwrap_or_else(|e| e.into_inner()) = f;
    }

    /// Run `program` instead of `gh` or `glab` for `family`'s CLI sign-in (tests).
    pub fn set_cli(&self, family: Family, program: std::path::PathBuf) {
        self.cli
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(family, program);
    }

    pub fn cli_override(&self, family: Family) -> Option<std::path::PathBuf> {
        self.cli
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&family)
            .cloned()
    }

    /// Forget every cached `checks` answer of `repo` (a rerun changed them).
    pub fn forget_prefix_checks(&self, repo: &Repository) {
        if let Some(c) = self.cache() {
            c.remove_kind(&repo.cache_key(), "checks");
        }
    }

    /// Let (or stop) reads refresh in the background.
    pub fn set_background(&self, on: bool) {
        self.background.store(on, Ordering::SeqCst);
    }

    /// The generation: it grows when the workspace, the repository or the account changes; a background answer of
    /// an older generation is dropped.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn bump(&self) -> u64 {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn add_listener(&self, l: Listener) {
        self.listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(l);
    }

    fn emit(&self, e: &HubEvent) {
        let ls = self
            .listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for l in ls {
            l(e);
        }
    }

    /// Detect the forge of `url`, probing an unknown host once per host when `probe`.
    pub fn detect(&self, url: &str, probe: bool) -> Repository {
        let config = self.config();
        let prober = |host: &str| -> detect::Probed {
            if let Some(hit) = self
                .probes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(host)
            {
                return hit.clone();
            }
            let base = match &*self.probe_base.read().unwrap_or_else(|e| e.into_inner()) {
                Some(f) => f(host),
                None => format!("https://{host}"),
            };
            let found = detect::probe(&*self.transport, &base);
            self.probes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(host.to_owned(), found.clone());
            found
        };
        if probe {
            detect::detect(url, &config.hosts, Some(&prober))
        } else {
            detect::detect(url, &config.hosts, None)
        }
    }

    /// The stored credential of `host`.
    pub fn credential(&self, host: &str) -> Option<(Credential, StoreKind)> {
        self.credentials.get(host)
    }

    /// The signed-in account of `host`, from the store (no request).
    pub fn account(&self, host: &str) -> Option<Account> {
        self.credential(host).and_then(|(c, _)| c.account)
    }

    /// A client for `repo`'s API, signed in when a token is stored.
    pub fn client(&self, repo: &Repository, cancel: Cancel) -> Client {
        let mut c = Client::new(self.transport.clone(), repo.api.clone(), repo.host.clone())
            .with_cache(self.cache())
            .with_limits(self.limits.clone())
            .with_cancel(cancel);
        if let Some((cred, _)) = self.credential(&repo.host) {
            c = auth::authorize(c, repo.family, &cred);
        }
        c
    }

    /// The forge of `repo`.
    pub fn forge(&self, repo: &Repository, cancel: Cancel) -> Result<Box<dyn Forge>> {
        let client = self.client(repo, cancel);
        let cred = self.credential(&repo.host).map(|(c, _)| c);
        make_forge(repo.clone(), client, cred)
    }

    /// The forge of `repo`, refusing with `sign_in_required` when no token is stored (writes need one).
    pub fn signed_in_forge(&self, repo: &Repository, cancel: Cancel) -> Result<Box<dyn Forge>> {
        if self.credential(&repo.host).is_none() {
            return Err(ForgeError::sign_in_required(
                &repo.host,
                "no token is stored for this host.",
            ));
        }
        self.forge(repo, cancel)
    }

    /// A read through the cache: answer the cached value at once (`stale: true`) and refresh in the background, or,
    /// with `wait` or nothing cached, fetch now (a failure falls back to the cache with the reason).
    pub fn read<T>(
        self: &Arc<Self>,
        repo: &Repository,
        key: &str,
        wait: bool,
        fetch: impl Fn(&dyn Forge) -> Result<T> + Send + Sync + 'static,
    ) -> Result<Answer<T>>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
    {
        let scope = repo.cache_key();
        let cache = self.cache();
        let cached: Option<Cached<T>> = cache.as_ref().and_then(|c| c.get(&scope, key));
        let flight = format!("{scope}|{key}");
        if !wait && let Some(c) = cached {
            let error = self
                .last_errors
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&flight)
                .cloned();
            let fresh_for = self.fresh_for();
            if error.is_none() && std::time::Duration::from_secs(c.age()) < fresh_for {
                // Fresh enough: no refresh.
                let mut a = Answer::cached(c, None);
                a.stale = false;
                return Ok(a);
            }
            let recent = self
                .attempted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&flight)
                .is_some_and(|t| t.elapsed() < fresh_for);
            if self.background.load(Ordering::SeqCst) && !recent {
                self.refresh_in_background(repo.clone(), key.to_owned(), fetch);
            }
            return Ok(Answer::cached(c, error));
        }
        let forge = self.forge(repo, Cancel::new())?;
        match fetch(&*forge) {
            Ok(v) => {
                if let Some(c) = &cache {
                    c.put(&scope, key, &v);
                }
                self.last_errors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&flight);
                Ok(Answer::fresh(v))
            }
            Err(e) => {
                self.last_errors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(flight, e.clone());
                match cached {
                    Some(c) if e.kind != ErrorKind::Canceled => Ok(Answer::cached(c, Some(e))),
                    _ => Err(e),
                }
            }
        }
    }

    /// The cached answer of `key` alone, never a request (the windows' first paint).
    pub fn cached<T: DeserializeOwned>(&self, repo: &Repository, key: &str) -> Option<Answer<T>> {
        let scope = repo.cache_key();
        let c: Cached<T> = self.cache()?.get(&scope, key)?;
        let error = self
            .last_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&format!("{scope}|{key}"))
            .cloned();
        Some(Answer::cached(c, error))
    }

    /// Store `value` as `key`'s answer (after a write changed it).
    pub fn store<T: Serialize>(&self, repo: &Repository, key: &str, value: &T) {
        if let Some(c) = self.cache() {
            c.put(&repo.cache_key(), key, value);
        }
    }

    /// Forget `key`'s answer (a write made it wrong).
    pub fn forget(&self, repo: &Repository, key: &str) {
        if let Some(c) = self.cache() {
            c.remove(&repo.cache_key(), key);
        }
    }

    fn refresh_in_background<T>(
        self: &Arc<Self>,
        repo: Repository,
        key: String,
        fetch: impl Fn(&dyn Forge) -> Result<T> + Send + Sync + 'static,
    ) where
        T: Serialize + Send + 'static,
    {
        let scope = repo.cache_key();
        let flight = format!("{scope}|{key}");
        if !self
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(flight.clone())
        {
            return;
        }
        let flight_key = flight.clone();
        let hub = self.clone();
        let generation = self.generation();
        let cache = self.cache();
        let spawned = std::thread::Builder::new()
            .name("forge-refresh".into())
            .spawn(move || {
                let result = hub.forge(&repo, Cancel::new()).and_then(|f| fetch(&*f));
                hub.attempted
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(flight.clone(), std::time::Instant::now());
                hub.in_flight
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&flight);
                if hub.generation() != generation {
                    return; // the workspace or the account changed: a stale answer is dropped
                }
                match result {
                    Ok(v) => {
                        if let Some(c) = &cache {
                            c.put(&scope, &key, &v);
                        }
                        hub.last_errors
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&flight);
                        hub.emit(&HubEvent::Refreshed { scope, key });
                    }
                    Err(error) => {
                        hub.last_errors
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(flight, error.clone());
                        hub.emit(&HubEvent::RefreshFailed { scope, key, error });
                    }
                }
            });
        if spawned.is_err() {
            self.in_flight
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&flight_key);
        }
    }

    /// Whether a background refresh of `key` is running.
    pub fn refreshing(&self, repo: &Repository, key: &str) -> bool {
        self.in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&format!("{}|{key}", repo.cache_key()))
    }

    /// The last refresh failure of `key`, if its last refresh failed.
    pub fn last_error(&self, repo: &Repository, key: &str) -> Option<ForgeError> {
        self.last_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&format!("{}|{key}", repo.cache_key()))
            .cloned()
    }

    // ----- Pending reviews -----

    fn pending_key(repo: &Repository, item: &ItemRef) -> String {
        format!("{}#{}", repo.cache_key(), item.id())
    }

    pub fn pending(&self, repo: &Repository, item: &ItemRef) -> Option<Pending> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&Self::pending_key(repo, item))
            .cloned()
    }

    pub fn start_pending(&self, repo: &Repository, item: &ItemRef) -> Pending {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(Self::pending_key(repo, item))
            .or_default()
            .clone()
    }

    pub fn update_pending(
        &self,
        repo: &Repository,
        item: &ItemRef,
        f: impl FnOnce(&mut Pending),
    ) -> Pending {
        let mut m = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let p = m.entry(Self::pending_key(repo, item)).or_default();
        f(p);
        p.clone()
    }

    pub fn clear_pending(&self, repo: &Repository, item: &ItemRef) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&Self::pending_key(repo, item));
    }

    // ----- Sign-in -----

    /// Sign in to `host` (of `family`) with a token the person pasted (or the CLI printed): checked against the
    /// forge, then stored. Answers the account and the store used.
    pub fn sign_in_token(
        &self,
        repo: &Repository,
        method: crate::credentials::SignInMethod,
        token: crate::credentials::Secret,
        user: Option<&str>,
        allow_file: bool,
    ) -> Result<(Account, StoreKind)> {
        let cred = auth::credential_from_token(&self.transport, repo, method, token, user)?;
        let account = cred.account.clone().unwrap_or_default();
        let kind = self.credentials.set(&repo.host, &cred, allow_file)?;
        self.bump();
        Ok((account, kind))
    }

    pub fn sign_out(&self, host: &str) -> Result<()> {
        self.cancel_flow(host);
        self.credentials.delete(host)?;
        self.bump();
        Ok(())
    }

    /// Start the device flow for `repo`'s host: answers the code at once, and polls off-thread until the person
    /// enters it (then stores the token and emits [`HubEvent::SignedIn`]), it expires, or it is canceled.
    pub fn start_device_flow(
        self: &Arc<Self>,
        repo: &Repository,
        allow_file: bool,
    ) -> Result<DeviceStart> {
        let config = self.config();
        let start = auth::device_start(&*self.transport, repo, &config)?;
        let cancel = Cancel::new();
        let flow = DeviceFlow {
            start: start.clone(),
            cancel: cancel.clone(),
            family: repo.family,
        };
        if let Some(old) = self
            .flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(repo.host.clone(), flow)
        {
            old.cancel.cancel();
        }
        let answer = start.clone();
        let hub = self.clone();
        let repo = repo.clone();
        std::thread::Builder::new()
            .name("forge-device-flow".into())
            .spawn(move || {
                let result = auth::device_poll(&hub.transport, &repo, &config, &start, &cancel)
                    .and_then(|cred| {
                        let account = cred.account.clone().unwrap_or_default();
                        hub.credentials.set(&repo.host, &cred, allow_file)?;
                        Ok(account)
                    });
                hub.flows
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&repo.host);
                if cancel.is_canceled() {
                    return;
                }
                hub.bump();
                hub.emit(&HubEvent::SignedIn {
                    host: repo.host.clone(),
                    result: result.map_err(|e| e.message),
                });
            })
            .map_err(|e| {
                ForgeError::other(format!("could not start the device flow's thread: {e}"))
            })?;
        Ok(answer)
    }

    /// The device flow waiting for `host`, if any.
    pub fn flow(&self, host: &str) -> Option<DeviceFlow> {
        self.flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(host)
            .cloned()
    }

    pub fn cancel_flow(&self, host: &str) {
        if let Some(f) = self
            .flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(host)
        {
            f.cancel.cancel();
        }
    }
}

/// Build the family's [`Forge`].
pub fn make_forge(
    repo: Repository,
    client: Client,
    cred: Option<Credential>,
) -> Result<Box<dyn Forge>> {
    Ok(match repo.family {
        Family::GitHub => Box::new(crate::github::GitHub::new(repo, client)),
        Family::GitLab => Box::new(crate::gitlab::GitLab::new(repo, client)),
        Family::AzureDevOps => Box::new(crate::azure::AzureDevOps::new(repo, client)),
        Family::Forgejo | Family::Gitea => Box::new(crate::forgejo::Forgejo::new(repo, client)),
        Family::Tangled => Box::new(crate::tangled::Tangled::new(repo, client, cred)),
        Family::None => {
            return Err(ForgeError::new(
                ErrorKind::Unsupported,
                format!(
                    "No supported forge for {}",
                    repo.remote_url
                        .as_deref()
                        .unwrap_or("this repository's remote")
                ),
            ));
        }
    })
}
