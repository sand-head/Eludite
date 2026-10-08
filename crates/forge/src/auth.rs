//! Signing in: the authorization header each family takes, a pasted token checked against the forge, the OAuth
//! device flows (GitHub, GitLab, the Microsoft identity platform for Azure DevOps; none needs a client secret), the
//! token the `gh` or `glab` CLI already holds, Tangled's app password, and refreshing an OAuth token.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::Client;
use crate::credentials::{Credential, Secret, SignInMethod};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::http::{Cancel, Method, Request, Transport, is_loopback};
use crate::hub::{
    BUILT_AZURE_APPLICATION_ID, BUILT_GITHUB_CLIENT_ID, BUILT_GITLAB_APPLICATION_ID, HubConfig,
};
use crate::model::{Account, Family, Repository};
use crate::process::NoConsoleWindow as _;
use crate::util::{base64, str_of};

/// Add `cred`'s authorization to `client` as `family` takes it.
pub fn authorize(client: Client, family: Family, cred: &Credential) -> Client {
    let t = cred.token.expose();
    match family {
        Family::GitHub | Family::GitLab => client.with_auth("Authorization", format!("Bearer {t}")),
        Family::AzureDevOps if cred.basic => client.with_auth(
            "Authorization",
            format!("Basic {}", base64(format!(":{t}").as_bytes())),
        ),
        Family::AzureDevOps => client.with_auth("Authorization", format!("Bearer {t}")),
        Family::Forgejo | Family::Gitea => client.with_auth("Authorization", format!("token {t}")),
        // Tangled's appview reads need no token; its writes go to the personal data server with a session.
        Family::Tangled | Family::None => client,
    }
}

/// A credential for `token`, checked by asking the forge who it belongs to (Tangled: an atproto session made with
/// the app password, the handle in `user`).
pub fn credential_from_token(
    transport: &Arc<dyn Transport>,
    repo: &Repository,
    method: SignInMethod,
    token: Secret,
    user: Option<&str>,
) -> Result<Credential> {
    let mut cred = Credential {
        family: repo.family,
        token,
        refresh: None,
        method,
        account: None,
        did: None,
        pds: None,
        basic: repo.family == Family::AzureDevOps && method != SignInMethod::Device,
    };
    if repo.family == Family::Tangled {
        let handle = user.ok_or_else(|| {
            ForgeError::invalid("Tangled signs in with your handle and an app password")
        })?;
        let session =
            crate::tangled::create_session(&**transport, repo, handle, cred.token.expose())?;
        cred.did = Some(session.did.clone());
        cred.pds = Some(session.pds.clone());
        cred.account = Some(Account {
            login: session.handle.clone(),
            name: None,
            url: Some(format!("https://tangled.org/{}", session.handle)),
        });
        return Ok(cred);
    }
    let client = authorize(
        Client::new(transport.clone(), repo.api.clone(), repo.host.clone()),
        repo.family,
        &cred,
    );
    let forge = crate::hub::make_forge(repo.clone(), client, Some(cred.clone()))?;
    let account = forge.account().map_err(|e| match e.kind {
        ErrorKind::SignInRequired => ForgeError::new(
            ErrorKind::Unauthorized,
            format!(
                "{} refused the token: check that it is valid and has the API scope",
                repo.host
            ),
        )
        .with_host(&repo.host),
        _ => e,
    })?;
    cred.account = Some(account);
    Ok(cred)
}

/// What a device flow shows the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStart {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    /// Never shown: the code the poll sends.
    pub device_code: Secret,
}

/// Where a host's OAuth endpoints are: `https://<host>`, or the fixture server's base in tests.
fn oauth_base(repo: &Repository) -> String {
    if is_loopback(&repo.api) {
        let api = repo.api.trim_end_matches('/');
        api.strip_suffix("/api/v4")
            .or_else(|| api.strip_suffix("/api/v3"))
            .unwrap_or(api)
            .to_owned()
    } else {
        format!("https://{}", repo.host)
    }
}

fn azure_login(repo: &Repository) -> String {
    if is_loopback(&repo.api) {
        format!("{}/login", repo.api.trim_end_matches('/'))
    } else {
        "https://login.microsoftonline.com/organizations".into()
    }
}

/// The Azure DevOps resource's scope.
pub const AZURE_SCOPE: &str = "499b84ac-1321-427f-aa17-267ca6975798/.default offline_access";

fn client_id(repo: &Repository, config: &HubConfig) -> Result<String> {
    let (configured, built, what) = match repo.family {
        Family::GitHub => (
            &config.github_client_id,
            BUILT_GITHUB_CLIENT_ID,
            "forge.githubClientId",
        ),
        Family::GitLab => (
            &config.gitlab_application_id,
            BUILT_GITLAB_APPLICATION_ID,
            "forge.gitlabApplicationId",
        ),
        Family::AzureDevOps => (
            &config.azure_application_id,
            BUILT_AZURE_APPLICATION_ID,
            "forge.azureApplicationId",
        ),
        other => {
            return Err(ForgeError::unsupported(
                "signing in with a browser (the device flow); paste an access token",
                other.display(),
            ));
        }
    };
    configured
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| built.map(str::to_owned).filter(|s| !s.is_empty()))
        .ok_or_else(|| {
            ForgeError::new(
                ErrorKind::Unsupported,
                format!(
                    "no OAuth application is registered for {}'s device flow in this build: set `{what}`, or sign in \
                     with a personal access token",
                    repo.family.display()
                ),
            )
        })
}

fn form(
    transport: &dyn Transport,
    url: &str,
    pairs: &[(&str, String)],
    cancel: &Cancel,
) -> Result<Value> {
    let r = transport.send(
        &Request::new(Method::Post, url)
            .header("Accept", "application/json")
            .form(pairs),
        cancel,
    )?;
    let v = r.json().unwrap_or(Value::Null);
    if r.status >= 400 && v.get("error").is_none() {
        return Err(ForgeError::other(format!(
            "{url} answered HTTP {}",
            r.status
        )));
    }
    Ok(v)
}

/// Start the device flow for `repo`'s host.
pub fn device_start(
    transport: &dyn Transport,
    repo: &Repository,
    config: &HubConfig,
) -> Result<DeviceStart> {
    let id = client_id(repo, config)?;
    let cancel = Cancel::new();
    let v = match repo.family {
        Family::GitHub => form(
            transport,
            &format!("{}/login/device/code", oauth_base(repo)),
            &[
                ("client_id", id),
                ("scope", "repo read:org workflow".into()),
            ],
            &cancel,
        )?,
        Family::GitLab => form(
            transport,
            &format!("{}/oauth/authorize_device", oauth_base(repo)),
            &[("client_id", id), ("scope", "api".into())],
            &cancel,
        )?,
        _ => form(
            transport,
            &format!("{}/oauth2/v2.0/devicecode", azure_login(repo)),
            &[("client_id", id), ("scope", AZURE_SCOPE.into())],
            &cancel,
        )?,
    };
    if let Some(e) = str_of(&v, "error") {
        return Err(ForgeError::other(format!(
            "the device flow could not start: {e} {}",
            str_of(&v, "error_description").unwrap_or_default()
        )));
    }
    Ok(DeviceStart {
        user_code: str_of(&v, "user_code")
            .ok_or_else(|| ForgeError::other("the device flow answered no code"))?,
        verification_uri: str_of(&v, "verification_uri")
            .or_else(|| str_of(&v, "verification_url"))
            .unwrap_or_default(),
        expires_in: v["expires_in"].as_u64().unwrap_or(900),
        interval: v["interval"].as_u64().unwrap_or(5).max(1),
        device_code: Secret::new(str_of(&v, "device_code").unwrap_or_default()),
    })
}

/// Poll until the person entered the code; answers the stored-to-be credential with its account.
pub fn device_poll(
    transport: &Arc<dyn Transport>,
    repo: &Repository,
    config: &HubConfig,
    start: &DeviceStart,
    cancel: &Cancel,
) -> Result<Credential> {
    let id = client_id(repo, config)?;
    let (url, grant) = match repo.family {
        Family::GitHub => (
            format!("{}/login/oauth/access_token", oauth_base(repo)),
            "urn:ietf:params:oauth:grant-type:device_code",
        ),
        Family::GitLab => (
            format!("{}/oauth/token", oauth_base(repo)),
            "urn:ietf:params:oauth:grant-type:device_code",
        ),
        _ => (
            format!("{}/oauth2/v2.0/token", azure_login(repo)),
            "urn:ietf:params:oauth:grant-type:device_code",
        ),
    };
    let deadline = Instant::now() + Duration::from_secs(start.expires_in);
    let mut interval = Duration::from_secs(start.interval);
    // Tests answer at once; a real flow waits the interval between polls.
    let fast = is_loopback(&repo.api);
    loop {
        cancel.check()?;
        if Instant::now() > deadline {
            return Err(ForgeError::other(
                "the device code expired before it was entered: start again",
            ));
        }
        let v = form(
            &**transport,
            &url,
            &[
                ("client_id", id.clone()),
                ("device_code", start.device_code.expose().to_owned()),
                ("grant_type", grant.into()),
            ],
            cancel,
        )?;
        if let Some(token) = str_of(&v, "access_token") {
            let mut cred = Credential {
                family: repo.family,
                token: Secret::new(token),
                refresh: str_of(&v, "refresh_token").map(Secret::new),
                method: SignInMethod::Device,
                account: None,
                did: None,
                pds: None,
                basic: false,
            };
            let client = authorize(
                Client::new(transport.clone(), repo.api.clone(), repo.host.clone()),
                repo.family,
                &cred,
            );
            let forge = crate::hub::make_forge(repo.clone(), client, Some(cred.clone()))?;
            cred.account = Some(forge.account()?);
            return Ok(cred);
        }
        match str_of(&v, "error").as_deref() {
            Some("authorization_pending") | None => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") | Some("code_expired") => {
                return Err(ForgeError::other(
                    "the device code expired before it was entered: start again",
                ));
            }
            Some("access_denied") | Some("authorization_declined") => {
                return Err(ForgeError::new(
                    ErrorKind::Unauthorized,
                    "the sign-in was declined",
                ));
            }
            Some(other) => {
                return Err(ForgeError::other(format!(
                    "the device flow failed: {other}"
                )));
            }
        }
        let wait = if fast {
            Duration::from_millis(20)
        } else {
            interval
        };
        let until = Instant::now() + wait;
        while Instant::now() < until {
            cancel.check()?;
            std::thread::sleep(Duration::from_millis(50).min(wait));
        }
    }
}

/// A new access token from `cred`'s refresh token (GitLab, Azure DevOps OAuth); `None` without one.
pub fn refresh(
    transport: &dyn Transport,
    repo: &Repository,
    config: &HubConfig,
    cred: &Credential,
) -> Option<Credential> {
    let refresh = cred.refresh.as_ref()?;
    let id = client_id(repo, config).ok()?;
    let url = match repo.family {
        Family::GitLab => format!("{}/oauth/token", oauth_base(repo)),
        Family::AzureDevOps => format!("{}/oauth2/v2.0/token", azure_login(repo)),
        _ => return None,
    };
    let mut pairs = vec![
        ("client_id", id),
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh.expose().to_owned()),
    ];
    if repo.family == Family::AzureDevOps {
        pairs.push(("scope", AZURE_SCOPE.to_owned()));
    }
    let v = form(transport, &url, &pairs, &Cancel::new()).ok()?;
    let token = str_of(&v, "access_token")?;
    let mut c = cred.clone();
    c.token = Secret::new(token);
    if let Some(r) = str_of(&v, "refresh_token") {
        c.refresh = Some(Secret::new(r));
    }
    Some(c)
}

/// The token `gh auth token` or `glab auth token` prints for `host` (the only process this crate starts, at
/// sign-in when the person chooses it). `program` overrides the CLI's path.
pub fn cli_token(family: Family, host: &str, program: Option<&Path>) -> Result<Secret> {
    let (default, args): (&str, Vec<&str>) = match family {
        Family::GitHub => ("gh", vec!["auth", "token", "--hostname", host]),
        Family::GitLab => ("glab", vec!["auth", "token", "--hostname", host]),
        other => {
            return Err(ForgeError::unsupported(
                "signing in with a command-line tool",
                other.display(),
            ));
        }
    };
    let program = program
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default.into());
    let out = std::process::Command::new(&program).no_console_window()
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            ForgeError::other(format!(
                "`{default}` could not be run ({e}): install it and sign in with `{default} auth login`, or paste a token"
            ))
        })?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || token.is_empty() || token.contains(char::is_whitespace) {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(ForgeError::new(
            ErrorKind::SignInRequired,
            format!(
                "`{default} auth token` printed no token for {host}: {} (sign in with `{default} auth login`)",
                err.lines().next().unwrap_or("").trim()
            ),
        )
        .with_host(host));
    }
    Ok(Secret::new(token))
}
