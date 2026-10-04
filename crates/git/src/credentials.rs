//! The credentials a transfer offers (brief 0045), in the order libgit2's credential callback tries them: for ssh the
//! ssh agent, then the key files in `~/.ssh`; for http(s) the configured `git-credential` helper, then the system's
//! default credentials (Negotiate and NTLM servers), then a user name and a password or token the shell supplies
//! from its credential prompt ([`SessionCredentials`]). When nothing is left the transfer fails with
//! [`ErrorKind::CredentialsRequired`] (the prompt can answer) or [`ErrorKind::Credentials`] (it cannot: an ssh server
//! that takes keys only), naming the remote's host and what was tried.
//!
//! The order is a small state machine ([`CredentialState`]) over a [`Sources`] the callback reads, so it is tested
//! without a server. Credentials the prompt supplies stay in memory ([`SessionCredentials`]): nothing is written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git2::CredentialType;

use crate::{ErrorKind, GitError};

/// A user name and a password or token. `Debug` never prints the password.
#[derive(Clone, PartialEq, Eq)]
pub struct UserPass {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for UserPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserPass")
            .field("username", &self.username)
            .field("password", &"<hidden>")
            .finish()
    }
}

#[derive(Default)]
struct Kept {
    /// "Remember for this session".
    remembered: HashMap<String, UserPass>,
    /// For the next transfer only (the prompt's answer without "remember").
    once: HashMap<String, UserPass>,
}

/// The credentials the shell's prompt supplied, by remote host (`host` or `host:port`), kept in memory for the
/// session: never written to disk, cleared with [`SessionCredentials::clear`]. Cheap to clone; clones share.
#[derive(Clone, Default)]
pub struct SessionCredentials(Arc<Mutex<Kept>>);

impl SessionCredentials {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keep `credential` for `host`: for the session with `remember`, else for the next transfers until
    /// [`SessionCredentials::forget_once`].
    pub fn supply(&self, host: &str, credential: UserPass, remember: bool) {
        let mut k = self.lock();
        if remember {
            k.once.remove(host);
            k.remembered.insert(host.to_owned(), credential);
        } else {
            k.once.insert(host.to_owned(), credential);
        }
    }

    /// The credential for `host`: a one-time answer first, then a remembered one.
    pub fn get(&self, host: &str) -> Option<UserPass> {
        let k = self.lock();
        k.once.get(host).or_else(|| k.remembered.get(host)).cloned()
    }

    /// Whether a credential is remembered for `host`.
    pub fn remembers(&self, host: &str) -> bool {
        self.lock().remembered.contains_key(host)
    }

    /// Drop the one-time answer for `host` (its transfer ended).
    pub fn forget_once(&self, host: &str) {
        self.lock().once.remove(host);
    }

    /// Drop everything kept for `host` (the server refused it).
    pub fn forget(&self, host: &str) {
        let mut k = self.lock();
        k.once.remove(host);
        k.remembered.remove(host);
    }

    /// Drop everything (the workspace closed).
    pub fn clear(&self) {
        let mut k = self.lock();
        k.once.clear();
        k.remembered.clear();
    }

    /// The hosts with a credential kept, sorted.
    pub fn hosts(&self) -> Vec<String> {
        let k = self.lock();
        let mut h: Vec<String> = k.once.keys().chain(k.remembered.keys()).cloned().collect();
        h.sort();
        h.dedup();
        h
    }
}

impl std::fmt::Debug for SessionCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCredentials")
            .field("hosts", &self.hosts())
            .finish()
    }
}

/// Two handles are equal when they share their store.
impl PartialEq for SessionCredentials {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SessionCredentials {}

/// The host (with its port when the url names one) of a remote url: `https://host:8443/r.git` gives `host:8443`,
/// `ssh://git@host/r.git` and `git@host:r.git` give `host`; a local path gives `None`.
pub fn remote_host(url: &str) -> Option<String> {
    if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        return (!host.is_empty()).then(|| host.to_owned());
    }
    // scp-like: [user@]host:path, where the colon comes before any slash (not `C:\x` or `./a:b`).
    let (before, _) = url.split_once(':')?;
    if before.contains('/') || before.contains('\\') || before.len() < 2 {
        return None;
    }
    let host = before.rsplit_once('@').map_or(before, |(_, h)| h);
    (!host.is_empty()).then(|| host.to_owned())
}

/// The scheme a remote url uses, lower case: `https`, `http`, `ssh` (an scp-like url too), `git`, `file` (a local
/// path too).
pub fn remote_scheme(url: &str) -> String {
    match url.split_once("://") {
        Some((s, _)) => s.to_ascii_lowercase(),
        None if remote_host(url).is_some() => "ssh".into(),
        None => "file".into(),
    }
}

/// A private key file and its public half when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFile {
    pub private: PathBuf,
    pub public: Option<PathBuf>,
}

/// The default key files OpenSSH offers, in its order (`ssh -i`'s documented default, without the security-key
/// types libssh2 cannot use).
pub const DEFAULT_KEY_NAMES: [&str; 3] = ["id_rsa", "id_ecdsa", "id_ed25519"];

/// Whether the private key text is protected by a passphrase (OpenSSH's own format, legacy PEM, or PKCS#8).
pub fn key_has_passphrase(text: &str) -> bool {
    if text.contains("ENCRYPTED") {
        return true;
    }
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .flat_map(|l| l.trim().chars())
        .collect();
    // "openssh-key-v1\0" then the cipher name: "none" (4 bytes) when it is not encrypted.
    const OPENSSH: &str = "b3BlbnNzaC1rZXktdjEA";
    const OPENSSH_NONE: &str = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmU";
    body.starts_with(OPENSSH) && !body.starts_with(OPENSSH_NONE)
}

/// The key files in `ssh_dir` libssh2 can use without a passphrase, in OpenSSH's order; the second list names the
/// keys skipped because they have one (they need the ssh agent).
pub fn ssh_key_candidates(ssh_dir: &Path) -> (Vec<KeyFile>, Vec<PathBuf>) {
    let mut keys = Vec::new();
    let mut locked = Vec::new();
    for name in DEFAULT_KEY_NAMES {
        let private = ssh_dir.join(name);
        let Ok(text) = std::fs::read_to_string(&private) else {
            continue;
        };
        if key_has_passphrase(&text) {
            locked.push(private);
            continue;
        }
        let public = ssh_dir.join(format!("{name}.pub"));
        keys.push(KeyFile {
            public: public.is_file().then_some(public),
            private,
        });
    }
    (keys, locked)
}

/// The user's ssh folder: `$HOME/.ssh` (`%USERPROFILE%\.ssh` on Windows).
pub fn default_ssh_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))?;
    Some(PathBuf::from(home).join(".ssh"))
}

/// What the credential callback reads. The real one ([`crate::remote`]'s) asks the environment, the git config and
/// the session; tests give fixed answers.
pub trait Sources {
    /// An ssh agent may answer (`SSH_AUTH_SOCK` is set; on Windows, Pageant or OpenSSH's agent pipe may).
    fn agent(&self) -> bool;
    /// The usable key files, and the ones skipped for a passphrase.
    fn ssh_keys(&self) -> (Vec<KeyFile>, Vec<PathBuf>);
    /// The configured `credential.helper` (its name, or `None` when there is none) and its answer for `url`.
    fn helper(
        &self,
        url: &str,
        username: Option<&str>,
    ) -> (Option<String>, Option<(String, String)>);
    /// What the shell's prompt supplied for `host`.
    fn supplied(&self, host: &str) -> Option<UserPass>;
    /// The local user name, for an ssh url that names none (as `ssh` does).
    fn local_user(&self) -> String;
}

/// What the callback answers libgit2 with next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    /// libgit2 asks which user an ssh url without one is for.
    Username(String),
    /// The ssh agent's keys, for this user.
    Agent(String),
    /// A key file, for this user.
    KeyFile(String, KeyFile),
    /// The helper's answer.
    Helper(UserPass),
    /// The system's default credentials (Negotiate, NTLM).
    Default,
    /// The shell's prompt's answer.
    Supplied(UserPass),
}

/// What a transfer's credential callback has tried, and the order it tries the rest in.
#[derive(Debug, Default)]
pub struct CredentialState {
    agent: Option<bool>,
    keys: Option<(Vec<KeyFile>, Vec<PathBuf>)>,
    next_key: usize,
    helper: Option<Option<String>>,
    helper_answered: bool,
    default: bool,
    supplied: Option<bool>,
    /// The error the callback ended with: it outranks libgit2's message.
    pub failure: Option<GitError>,
}

impl CredentialState {
    pub fn new() -> Self {
        Self::default()
    }

    /// What to answer libgit2's request for `allowed` on `url` (`username` is the url's or the one answered before),
    /// or the failure when nothing is left (also kept in [`CredentialState::failure`]).
    pub fn next(
        &mut self,
        url: &str,
        username: Option<&str>,
        allowed: CredentialType,
        sources: &dyn Sources,
    ) -> Result<Attempt, GitError> {
        if allowed.contains(CredentialType::USERNAME) {
            return Ok(Attempt::Username(
                username
                    .map(str::to_owned)
                    .unwrap_or_else(|| sources.local_user()),
            ));
        }
        let host = remote_host(url).unwrap_or_else(|| url.to_owned());
        let user = username
            .map(str::to_owned)
            .unwrap_or_else(|| sources.local_user());
        if allowed.contains(CredentialType::SSH_KEY) {
            if self.agent.is_none() {
                let available = sources.agent();
                self.agent = Some(available);
                if available {
                    return Ok(Attempt::Agent(user));
                }
            }
            let (keys, _) = self.keys.get_or_insert_with(|| sources.ssh_keys());
            if let Some(k) = keys.get(self.next_key) {
                self.next_key += 1;
                return Ok(Attempt::KeyFile(user, k.clone()));
            }
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) && self.helper.is_none() {
            let (name, answer) = sources.helper(url, username);
            self.helper = Some(name);
            if let Some((username, password)) = answer {
                self.helper_answered = true;
                return Ok(Attempt::Helper(UserPass { username, password }));
            }
        }
        if allowed.contains(CredentialType::DEFAULT) && !self.default {
            self.default = true;
            return Ok(Attempt::Default);
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
            if self.supplied.is_none() {
                let up = sources.supplied(&host);
                self.supplied = Some(up.is_some());
                if let Some(up) = up {
                    return Ok(Attempt::Supplied(up));
                }
            }
            let refused = self.supplied == Some(true);
            let e = credentials_required(&host, &self.describe(), refused);
            self.failure = Some(e.clone());
            return Err(e);
        }
        let e = credential_failure(&host, url, &self.describe());
        self.failure = Some(e.clone());
        Err(e)
    }

    /// Whether the prompt's answer was used (and, when the callback is asked again, refused).
    pub fn used_supplied(&self) -> bool {
        self.supplied == Some(true)
    }

    /// What was tried, in order, for the failure message.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        match self.agent {
            Some(true) => parts.push("the ssh agent".to_owned()),
            Some(false) => parts.push("no ssh agent (SSH_AUTH_SOCK is not set)".to_owned()),
            None => {}
        }
        if let Some((keys, locked)) = &self.keys {
            let tried: Vec<String> = keys[..self.next_key]
                .iter()
                .map(|k| format!("`{}`", k.private.display()))
                .collect();
            if !tried.is_empty() {
                parts.push(format!("the key files {}", tried.join(", ")));
            } else if locked.is_empty() {
                parts.push("no key file in ~/.ssh".to_owned());
            }
            for l in locked {
                parts.push(format!(
                    "not `{}` (it has a passphrase: add it to the ssh agent with `ssh-add`)",
                    l.display()
                ));
            }
        }
        match &self.helper {
            Some(Some(h)) if self.helper_answered => {
                parts.push(format!("the credential helper `{h}`"))
            }
            Some(Some(h)) => parts.push(format!("the credential helper `{h}` (it had no answer)")),
            Some(None) => parts.push("no credential helper (none configured)".to_owned()),
            None => {}
        }
        if self.default {
            parts.push("the system's default credentials".to_owned());
        }
        if self.supplied == Some(true) {
            parts.push("the user name and password given in the credential prompt".to_owned());
        }
        if parts.is_empty() {
            "nothing (the remote asked for no method Eludite supports)".into()
        } else {
            parts.join(", then ")
        }
    }
}

/// The prefix of a [`ErrorKind::CredentialsRequired`] message: the command's answer names it, then the host.
pub const CREDENTIALS_REQUIRED: &str = "credentials_required";

/// The shell's dialog that answers [`CREDENTIALS_REQUIRED`] for the person (never for an agent).
pub const CREDENTIAL_PROMPT: &str = "git.credentialPrompt";

/// Nothing answered `host`'s request for a user name and a password or token (`refused`: the prompt's answer was
/// refused): the shell's prompt can answer.
pub fn credentials_required(host: &str, tried: &str, refused: bool) -> GitError {
    let what = if refused {
        format!("{host} refused the user name and password or token given for it")
    } else {
        format!("{host} asks for a user name and a password or token, and nothing answered")
    };
    GitError {
        kind: ErrorKind::CredentialsRequired,
        message: format!(
            "{CREDENTIALS_REQUIRED}: {what} (tried {tried}). The person answers in Eludite's credential prompt \
             ({CREDENTIAL_PROMPT}); a credential helper (`git config --global credential.helper <helper>`, such as \
             Git Credential Manager) answers without asking."
        ),
        paths: Vec::new(),
        host: Some(host.to_owned()),
        refused,
    }
}

/// Authentication failed and the prompt cannot help (an ssh server that takes keys only).
pub fn credential_failure(host: &str, url: &str, tried: &str) -> GitError {
    GitError {
        kind: ErrorKind::Credentials,
        message: format!(
            "Authentication failed for {url}: tried {tried}. For ssh add your key to the ssh agent (`ssh-add`) or \
             put it in ~/.ssh without a passphrase; for https set up a credential helper (`git config --global \
             credential.helper <helper>`, such as Git Credential Manager or `store`)."
        ),
        paths: Vec::new(),
        host: Some(host.to_owned()),
        refused: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Fixed answers, and which were asked for.
    #[derive(Default)]
    struct Fake {
        agent: bool,
        keys: Vec<KeyFile>,
        locked: Vec<PathBuf>,
        helper: Option<String>,
        helper_answer: Option<(String, String)>,
        supplied: Option<UserPass>,
        asked: RefCell<Vec<&'static str>>,
    }

    impl Sources for Fake {
        fn agent(&self) -> bool {
            self.asked.borrow_mut().push("agent");
            self.agent
        }
        fn ssh_keys(&self) -> (Vec<KeyFile>, Vec<PathBuf>) {
            self.asked.borrow_mut().push("keys");
            (self.keys.clone(), self.locked.clone())
        }
        fn helper(&self, _: &str, _: Option<&str>) -> (Option<String>, Option<(String, String)>) {
            self.asked.borrow_mut().push("helper");
            (self.helper.clone(), self.helper_answer.clone())
        }
        fn supplied(&self, host: &str) -> Option<UserPass> {
            assert_eq!(host, "example.com:8443");
            self.asked.borrow_mut().push("supplied");
            self.supplied.clone()
        }
        fn local_user(&self) -> String {
            "me".into()
        }
    }

    const HTTPS: &str = "https://example.com:8443/r.git";
    const SSH: &str = "ssh://example.com:8443/r.git";

    fn up(u: &str, p: &str) -> UserPass {
        UserPass {
            username: u.into(),
            password: p.into(),
        }
    }

    fn key(name: &str) -> KeyFile {
        KeyFile {
            private: PathBuf::from(name),
            public: None,
        }
    }

    fn userpass() -> CredentialType {
        CredentialType::USER_PASS_PLAINTEXT
    }

    #[test]
    fn https_tries_the_helper_then_default_then_the_prompt_then_fails() {
        let f = Fake {
            helper: Some("store".into()),
            helper_answer: Some(("h".into(), "hp".into())),
            supplied: Some(up("p", "pp")),
            ..Default::default()
        };
        let mut s = CredentialState::new();
        let both = userpass() | CredentialType::DEFAULT;
        assert_eq!(
            s.next(HTTPS, None, both, &f).unwrap(),
            Attempt::Helper(up("h", "hp"))
        );
        assert_eq!(s.next(HTTPS, None, both, &f).unwrap(), Attempt::Default);
        assert_eq!(
            s.next(HTTPS, None, both, &f).unwrap(),
            Attempt::Supplied(up("p", "pp"))
        );
        assert!(s.used_supplied());
        // The prompt's answer was refused: the prompt is asked for again, saying so.
        let e = s.next(HTTPS, None, both, &f).unwrap_err();
        assert_eq!(e.kind, ErrorKind::CredentialsRequired);
        assert!(e.refused);
        assert_eq!(e.host.as_deref(), Some("example.com:8443"));
        assert!(
            e.message
                .starts_with("credentials_required: example.com:8443 refused"),
            "{e}"
        );
        assert!(
            e.message.contains(
                "the credential helper `store`, then the system's default credentials, then the user name and \
                 password given in the credential prompt"
            ),
            "{e}"
        );
        assert_eq!(s.failure.as_ref(), Some(&e));
        // No agent or key was asked about for an https remote.
        assert_eq!(*f.asked.borrow(), ["helper", "supplied"]);
    }

    #[test]
    fn each_https_source_absent_falls_through_to_the_next() {
        // No helper configured, no prompt answer yet: credentials_required, not refused.
        let f = Fake::default();
        let mut s = CredentialState::new();
        let e = s.next(HTTPS, None, userpass(), &f).unwrap_err();
        assert_eq!((e.kind, e.refused), (ErrorKind::CredentialsRequired, false));
        assert!(
            e.message
                .contains("asks for a user name and a password or token"),
            "{e}"
        );
        assert!(
            e.message.contains("no credential helper (none configured)"),
            "{e}"
        );
        assert!(e.message.contains(CREDENTIAL_PROMPT), "{e}");
        // A helper that has no answer: the prompt's answer is next.
        let f = Fake {
            helper: Some("store".into()),
            supplied: Some(up("p", "pp")),
            ..Default::default()
        };
        let mut s = CredentialState::new();
        assert_eq!(
            s.next(HTTPS, None, userpass(), &f).unwrap(),
            Attempt::Supplied(up("p", "pp"))
        );
        assert!(s.describe().contains("`store` (it had no answer)"));
        // A helper's answer refused, nothing supplied: credentials_required.
        let f = Fake {
            helper: Some("!h".into()),
            helper_answer: Some(("h".into(), "x".into())),
            ..Default::default()
        };
        let mut s = CredentialState::new();
        assert!(matches!(
            s.next(HTTPS, None, userpass(), &f),
            Ok(Attempt::Helper(_))
        ));
        let e = s.next(HTTPS, None, userpass(), &f).unwrap_err();
        assert_eq!((e.kind, e.refused), (ErrorKind::CredentialsRequired, false));
        // The default credentials only when the server offers them (Negotiate, NTLM), and once.
        let mut s = CredentialState::new();
        assert_eq!(
            s.next(HTTPS, None, CredentialType::DEFAULT, &Fake::default())
                .unwrap(),
            Attempt::Default
        );
        let e = s
            .next(HTTPS, None, CredentialType::DEFAULT, &Fake::default())
            .unwrap_err();
        assert_eq!(
            e.kind,
            ErrorKind::Credentials,
            "the prompt cannot answer a Negotiate server"
        );
    }

    #[test]
    fn ssh_asks_the_user_then_the_agent_then_each_key_then_fails_without_a_prompt() {
        let f = Fake {
            agent: true,
            keys: vec![key("id_rsa"), key("id_ed25519")],
            locked: vec![PathBuf::from("id_ecdsa")],
            supplied: Some(up("p", "pp")),
            ..Default::default()
        };
        let mut s = CredentialState::new();
        assert_eq!(
            s.next(SSH, None, CredentialType::USERNAME, &f).unwrap(),
            Attempt::Username("me".into()),
            "an ssh url without a user is for the local user, as with ssh"
        );
        let key_only = CredentialType::SSH_KEY;
        assert_eq!(
            s.next(SSH, Some("git"), key_only, &f).unwrap(),
            Attempt::Agent("git".into())
        );
        assert_eq!(
            s.next(SSH, Some("git"), key_only, &f).unwrap(),
            Attempt::KeyFile("git".into(), key("id_rsa"))
        );
        assert_eq!(
            s.next(SSH, Some("git"), key_only, &f).unwrap(),
            Attempt::KeyFile("git".into(), key("id_ed25519"))
        );
        let e = s.next(SSH, Some("git"), key_only, &f).unwrap_err();
        assert_eq!(
            e.kind,
            ErrorKind::Credentials,
            "a key-only server: the prompt cannot help"
        );
        assert!(
            e.message.contains(
                "tried the ssh agent, then the key files `id_rsa`, `id_ed25519`, then not `id_ecdsa` (it has a \
                 passphrase"
            ),
            "{e}"
        );
        assert!(e.message.contains("ssh-add"), "{e}");
        assert_eq!(*f.asked.borrow(), ["agent", "keys"]);
    }

    #[test]
    fn ssh_without_an_agent_or_keys_and_a_password_server_reaches_the_prompt() {
        let f = Fake {
            supplied: Some(up("me", "pw")),
            ..Default::default()
        };
        let mut s = CredentialState::new();
        let both = CredentialType::SSH_KEY | userpass();
        assert_eq!(
            s.next(SSH, Some("me"), both, &f).unwrap(),
            Attempt::Supplied(up("me", "pw"))
        );
        assert!(
            s.describe()
                .starts_with("no ssh agent (SSH_AUTH_SOCK is not set), then no key file in ~/.ssh")
        );
        let e = s.next(SSH, Some("me"), both, &f).unwrap_err();
        assert!(e.refused);
    }

    #[test]
    fn key_selection_follows_openssh_and_skips_keys_with_a_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(ssh_key_candidates(d), (vec![], vec![]));
        // An unencrypted OpenSSH key, with its public half; a PEM key without; an encrypted OpenSSH key.
        std::fs::write(
            d.join("id_ed25519"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtz\n\
             -----END OPENSSH PRIVATE KEY-----\n",
        )
        .unwrap();
        std::fs::write(d.join("id_ed25519.pub"), "ssh-ed25519 AAAA me\n").unwrap();
        std::fs::write(
            d.join("id_rsa"),
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\n",
        )
        .unwrap();
        std::fs::write(
            d.join("id_ecdsa"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABD\n\
             -----END OPENSSH PRIVATE KEY-----\n",
        )
        .unwrap();
        std::fs::write(d.join("id_dsa"), "ignored: not offered").unwrap();
        let (keys, locked) = ssh_key_candidates(d);
        assert_eq!(
            keys,
            [
                KeyFile {
                    private: d.join("id_rsa"),
                    public: None
                },
                KeyFile {
                    private: d.join("id_ed25519"),
                    public: Some(d.join("id_ed25519.pub"))
                },
            ]
        );
        assert_eq!(locked, [d.join("id_ecdsa")]);
        assert!(key_has_passphrase(
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\n\n-----END RSA PRIVATE KEY-----"
        ));
        assert!(key_has_passphrase(
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIF\n-----END ENCRYPTED PRIVATE KEY-----"
        ));
    }

    #[test]
    fn hosts_and_schemes_of_remote_urls() {
        for (url, host, scheme) in [
            ("https://github.com/o/r.git", Some("github.com"), "https"),
            (
                "https://user:tok@example.com:8443/r.git",
                Some("example.com:8443"),
                "https",
            ),
            (
                "http://127.0.0.1:4000/r.git",
                Some("127.0.0.1:4000"),
                "http",
            ),
            (
                "ssh://git@example.com:2222/r.git",
                Some("example.com:2222"),
                "ssh",
            ),
            ("git@github.com:o/r.git", Some("github.com"), "ssh"),
            ("github.com:o/r.git", Some("github.com"), "ssh"),
            ("git://example.com/r.git", Some("example.com"), "git"),
            ("https://[::1]:8443/r.git", Some("[::1]:8443"), "https"),
            ("file:///tmp/r.git", None, "file"),
            ("/tmp/r.git", None, "file"),
            ("C:\\repos\\r.git", None, "file"),
            ("../r.git", None, "file"),
        ] {
            assert_eq!(remote_host(url).as_deref(), host, "{url}");
            assert_eq!(remote_scheme(url), scheme, "{url}");
        }
    }

    #[test]
    fn session_credentials_stay_in_memory_once_or_for_the_session() {
        let s = SessionCredentials::new();
        let clone = s.clone();
        assert_eq!(s, clone);
        assert_ne!(s, SessionCredentials::new());
        s.supply("h", up("a", "1"), false);
        assert_eq!(clone.get("h"), Some(up("a", "1")), "clones share the store");
        assert!(!s.remembers("h"));
        s.forget_once("h");
        assert_eq!(s.get("h"), None);
        s.supply("h", up("b", "2"), true);
        s.supply("h", up("c", "3"), false);
        assert_eq!(
            s.get("h"),
            Some(up("c", "3")),
            "a one-time answer comes first"
        );
        s.forget_once("h");
        assert_eq!(s.get("h"), Some(up("b", "2")));
        assert_eq!(s.hosts(), ["h"]);
        assert!(
            !format!("{s:?} {:?}", up("u", "secret")).contains("secret"),
            "Debug hides passwords"
        );
        s.forget("h");
        assert_eq!(s.get("h"), None);
        s.supply("h", up("b", "2"), true);
        s.clear();
        assert!(s.hosts().is_empty());
    }
}
