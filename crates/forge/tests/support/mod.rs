//! Shared by the fixture tests, the real-service tests and the recorder: a fake [`GitSide`], hubs over the fixture
//! server or the real transport, the schema checker, and the scenarios.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eludite_forge::cache::Cache;
use eludite_forge::credentials::{Credential, Credentials, MemoryStore, Secret, SignInMethod};
use eludite_forge::detect::HostEntry;
use eludite_forge::http::{Transport, UreqTransport};
use eludite_forge::hub::{Hub, HubConfig};
use eludite_forge::ops::{self, GitSide};
use eludite_forge::replay::{FixtureServer, Fixtures};
use eludite_forge::{Account, Family};
use serde_json::{Value, json};

/// The real transport, refusing anything but loopback: a fixture test never reaches a real service.
pub struct LoopbackOnly(pub UreqTransport);

impl Transport for LoopbackOnly {
    fn send(
        &self,
        request: &eludite_forge::http::Request,
        cancel: &eludite_forge::http::Cancel,
    ) -> eludite_forge::Result<eludite_forge::http::Response> {
        assert!(
            eludite_forge::http::is_loopback(&request.url),
            "a fixture test tried to reach {}",
            request.url
        );
        self.0.send(request, cancel)
    }
}

/// A repository as the commands see it, without git.
#[derive(Default)]
pub struct FakeGit {
    pub url: String,
    pub branch: Mutex<Option<String>>,
    pub head: Mutex<String>,
    pub commits: Vec<(String, String)>,
    pub default_branch: Option<String>,
    pub checkouts: Mutex<Vec<(Option<String>, String, String)>>,
    pub branches: Mutex<Vec<(String, Option<String>, bool)>>,
    pub dirty: Mutex<Option<String>>,
}

impl FakeGit {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            branch: Mutex::new(Some("feature/login".into())),
            head: Mutex::new("1111111111111111111111111111111111111111".into()),
            commits: vec![(
                "2222222222222222222222222222222222222222".into(),
                "Add the login form\n\nIt posts to /session.".into(),
            )],
            default_branch: Some("main".into()),
            ..Default::default()
        }
    }
}

impl GitSide for FakeGit {
    fn remote(&self, remote: Option<&str>) -> Option<(String, String)> {
        match remote {
            None | Some("origin") => Some(("origin".into(), self.url.clone())),
            Some(_) => None,
        }
    }
    fn current_branch(&self) -> Option<String> {
        self.branch.lock().unwrap().clone()
    }
    fn resolve(&self, revision: Option<&str>) -> Option<String> {
        match revision {
            None | Some("HEAD") => Some(self.head.lock().unwrap().clone()),
            Some(r) if r.len() == 40 => Some(r.to_owned()),
            Some(_) => Some(self.head.lock().unwrap().clone()),
        }
    }
    fn commits_between(&self, _base: &str, _head: &str) -> Vec<(String, String)> {
        self.commits.clone()
    }
    fn default_branch(&self, _remote: &str) -> Option<String> {
        self.default_branch.clone()
    }
    fn fetch_and_checkout(
        &self,
        _remote: &str,
        url: Option<&str>,
        refspec: &str,
        branch: &str,
    ) -> Result<(String, u64), String> {
        if let Some(d) = self.dirty.lock().unwrap().clone() {
            return Err(format!(
                "checkout refused: your local changes to {d} would be overwritten"
            ));
        }
        self.checkouts.lock().unwrap().push((
            url.map(str::to_owned),
            refspec.to_owned(),
            branch.to_owned(),
        ));
        *self.branch.lock().unwrap() = Some(branch.to_owned());
        Ok(("3333333333333333333333333333333333333333".into(), 7))
    }
    fn create_branch(
        &self,
        name: &str,
        base: Option<&str>,
        checkout: bool,
    ) -> Result<String, String> {
        self.branches
            .lock()
            .unwrap()
            .push((name.to_owned(), base.map(str::to_owned), checkout));
        if checkout {
            *self.branch.lock().unwrap() = Some(name.to_owned());
        }
        Ok(self.head.lock().unwrap().clone())
    }
}

/// The fixture folder of `forge`.
pub fn testdata(forge: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(forge)
}

/// A token for the tests' fake store; its text is searched for in the cache.
pub const TOKEN: &str = "test-token-0123456789-never-in-the-cache";

pub struct Setup {
    pub server: FixtureServer,
    pub hub: Arc<Hub>,
    pub git: FakeGit,
    pub dir: tempfile::TempDir,
    pub host: String,
}

impl Setup {
    pub fn fixtures(&self) -> &Fixtures {
        &self.server.fixtures
    }

    pub fn run(&self, id: &str, input: Value) -> Result<Value, eludite_forge::ForgeError> {
        ops::run(&self.hub, &self.git, id, &input)
    }

    pub fn ok(&self, id: &str, input: Value) -> Value {
        match self.run(id, input.clone()) {
            Ok(v) => {
                conforms(id, &v);
                v
            }
            Err(e) => panic!(
                "{id} {input}: {e}\nmisses: {:?}",
                self.fixtures()
                    .seen()
                    .iter()
                    .filter(|s| s.status == 404)
                    .collect::<Vec<_>>()
            ),
        }
    }

    pub fn cache(&self) -> Cache {
        self.hub.cache().unwrap()
    }
}

/// A hub over the fixture server serving `forge`'s fixtures (and `extra` folders), with `host` mapped to it by
/// `forge.hosts` (`api_prefix` after the server's base), signed in as `login` when given.
pub fn setup(
    forge: &str,
    family: Family,
    host: &str,
    api_prefix: &str,
    remote: &str,
    login: Option<&str>,
) -> Setup {
    let fixtures = Fixtures::load(&testdata(forge)).expect("fixtures load");
    let server = FixtureServer::start(fixtures).unwrap();
    let store = MemoryStore::new();
    if let Some(l) = login {
        store.set_unavailable(false);
        eludite_forge::credentials::CredentialStore::set(
            &store,
            host,
            &Credential {
                family,
                token: Secret::new(TOKEN),
                refresh: None,
                method: SignInMethod::Token,
                account: Some(Account {
                    login: l.into(),
                    name: None,
                    url: None,
                }),
                did: Some("did:plc:testuser".into()),
                pds: None,
                basic: family == Family::AzureDevOps,
            },
        )
        .unwrap();
    }
    let hub = Hub::new(
        Arc::new(UreqTransport::default()),
        Credentials::new(Box::new(store), None),
    );
    hub.set_config(HubConfig {
        hosts: vec![HostEntry {
            host: host.into(),
            family,
            api: Some(format!("{}{api_prefix}", server.base())),
        }],
        ..Default::default()
    });
    let dir = tempfile::tempdir().unwrap();
    hub.set_cache(Some(Cache::for_workspace(dir.path())));
    hub.set_background(false);
    hub.set_fresh_for(std::time::Duration::ZERO);
    Setup {
        server,
        hub,
        git: FakeGit::new(remote),
        dir,
        host: host.into(),
    }
}

/// A hub over the real transport (the real-service tests and the recorder), signed in with `token` when given.
pub fn live(
    family: Family,
    host: &str,
    remote: &str,
    transport: Arc<dyn Transport>,
    token: Option<(&str, Option<&str>)>,
) -> (Arc<Hub>, FakeGit, tempfile::TempDir) {
    let store = MemoryStore::new();
    let hub = Hub::new(transport, Credentials::new(Box::new(store), None));
    let dir = tempfile::tempdir().unwrap();
    hub.set_cache(Some(Cache::for_workspace(dir.path())));
    hub.set_background(false);
    let repo = hub.detect(remote, true);
    if let Some((t, user)) = token {
        let method = if family == Family::Tangled {
            SignInMethod::AppPassword
        } else {
            SignInMethod::Token
        };
        hub.sign_in_token(&repo, method, Secret::new(t), user, false)
            .expect("signs in");
    }
    let _ = host;
    (hub, FakeGit::new(remote), dir)
}

// ----- The schema checker -----

fn schema_file(id: &str) -> PathBuf {
    let name = id.rsplit('.').next().unwrap().replace('_', "-");
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../protocol/schemas/forge-{name}.output.json"))
}

/// `value` conforms to command `id`'s output schema: required members, no undeclared member, enums, patterns,
/// integer bounds, types, through objects and arrays.
pub fn conforms(id: &str, value: &Value) {
    let text = std::fs::read_to_string(schema_file(id)).unwrap();
    let root: Value = serde_json::from_str(&text).unwrap();
    check(&root, value, "$", id);
}

fn check(schema: &Value, value: &Value, at: &str, id: &str) {
    if let Some(e) = schema["enum"].as_array() {
        assert!(e.contains(value), "{id} {at}: {value} not in {e:?}");
    }
    if let Some(p) = schema["pattern"].as_str() {
        let s = value
            .as_str()
            .unwrap_or_else(|| panic!("{id} {at}: not a string"));
        if p == "^[0-9a-f]{40}$" {
            assert!(
                s.len() == 40
                    && s.chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{id} {at}: {s} !~ {p}"
            );
        }
    }
    match schema["type"].as_str() {
        Some("object") => {
            let obj = value
                .as_object()
                .unwrap_or_else(|| panic!("{id} {at}: not an object: {value}"));
            for r in schema["required"].as_array().into_iter().flatten() {
                assert!(
                    obj.contains_key(r.as_str().unwrap()),
                    "{id} {at}: missing {r} in {value}"
                );
            }
            for (k, v) in obj {
                let p = &schema["properties"][k];
                if p.is_null() {
                    match &schema["additionalProperties"] {
                        Value::Bool(false) => panic!("{id} {at}: unexpected {k}"),
                        Value::Object(_) => {
                            check(&schema["additionalProperties"], v, &format!("{at}.{k}"), id)
                        }
                        _ => {}
                    }
                } else {
                    check(p, v, &format!("{at}.{k}"), id);
                }
            }
        }
        Some("array") => {
            let a = value
                .as_array()
                .unwrap_or_else(|| panic!("{id} {at}: not an array"));
            for (i, v) in a.iter().enumerate() {
                check(&schema["items"], v, &format!("{at}[{i}]"), id);
            }
        }
        Some("integer") => {
            let n = value
                .as_i64()
                .unwrap_or_else(|| panic!("{id} {at}: not an integer: {value}"));
            if let Some(min) = schema["minimum"].as_i64() {
                assert!(n >= min, "{id} {at}: {n} < {min}");
            }
        }
        Some("string") => assert!(value.is_string(), "{id} {at}: not a string: {value}"),
        Some("boolean") => assert!(value.is_boolean(), "{id} {at}: not a boolean: {value}"),
        _ => {}
    }
}

/// No file under the cache contains the token.
pub fn cache_has_no_token(cache: &Cache) {
    let files = cache.files();
    assert!(!files.is_empty(), "the cache is empty");
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        assert!(!text.contains(TOKEN), "{} holds the token", f.display());
    }
}

/// The read scenario every family runs (the recorder records it; the fixture tests replay it): detect, the open
/// pulls (a page and the next), one pull request, its checks and a log, the open issues and one issue.
pub fn read_scenario(
    hub: &Arc<Hub>,
    git: &FakeGit,
    pull: &Value,
    issue: Option<&Value>,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut call = |id: &str, input: Value| -> Option<Value> {
        match ops::run(hub, git, &format!("eludite.forge.{id}"), &input) {
            Ok(v) => {
                out.push((id.to_owned(), v.clone()));
                Some(v)
            }
            Err(e) => {
                eprintln!("{id} {input}: {e}");
                None
            }
        }
    };
    call("detect", json!({}));
    if let Some(page) = call("pulls", json!({"max": 3, "refresh": true}))
        && let Some(c) = page["next_cursor"].as_str()
    {
        call("pulls", json!({"max": 3, "cursor": c, "refresh": true}));
    }
    let mut input = pull.clone();
    input["refresh"] = json!(true);
    if let Some(p) = call("pull", input) {
        let mut ci = pull.clone();
        ci["refresh"] = json!(true);
        if let Some(checks) = call("checks", ci)
            && let Some(c) = checks["items"]
                .as_array()
                .and_then(|a| a.iter().find(|c| c["has_log"] == true))
        {
            call("check_log", json!({"id": c["id"], "max_bytes": 4096}));
        }
        let _ = p;
    }
    if let Some(list) = call("issues", json!({"max": 3, "refresh": true})) {
        let chosen = issue.cloned().or_else(|| {
            list["items"]
                .as_array()
                .and_then(|a| a.first())
                .map(|i| match i["number"].as_u64() {
                    Some(n) => json!({"number": n}),
                    None => json!({"id": i["id"]}),
                })
        });
        if let Some(mut ii) = chosen {
            ii["refresh"] = json!(true);
            call("issue", ii);
        }
    }
    out
}
