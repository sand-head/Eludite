//! Which forge a remote url lives on: the well-known hosts, the `forge.hosts` setting, and for any other host a
//! version probe (`/api/v4/version` answers for GitLab, even with a 401; `/api/v1/version` for Forgejo and Gitea,
//! told apart by Forgejo's `/api/forgejo/v1/version`; `/api/v3/meta` for GitHub Enterprise Server). Detection never
//! runs at startup: a window opening or a command asks.

use serde::{Deserialize, Serialize};

use crate::http::{Cancel, Method, Request, Transport};
use crate::model::{Family, Repository};

/// One `forge.hosts` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    pub host: String,
    pub family: Family,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
}

/// A remote url taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    /// `https`, `http`, `ssh`.
    pub scheme: String,
    /// The host, with the port for http(s) (`gitlab.example.com:8443`).
    pub host: String,
    /// The user before `@`, if any (`git`, Azure DevOps' organization).
    pub user: Option<String>,
    /// The path's segments, `.git` stripped from the last.
    pub segments: Vec<String>,
}

/// Take `url` apart: `https://host/a/b.git`, `ssh://git@host:22/a/b.git`, `git@host:a/b.git`. `None` for a local
/// path or a `file://` url.
pub fn parse_remote(url: &str) -> Option<RemoteUrl> {
    let url = url.trim();
    let (scheme, rest) = if let Some((s, r)) = url.split_once("://") {
        (s.to_ascii_lowercase(), r)
    } else if !url.starts_with('/') && !url.starts_with('.') && url.contains(':') {
        let (left, right) = url.split_once(':')?;
        if left.contains('/') || left.len() == 1 {
            return None; // a path, or a Windows drive letter
        }
        return Some(RemoteUrl {
            scheme: "ssh".into(),
            host: left
                .rsplit_once('@')
                .map(|(_, h)| h)
                .unwrap_or(left)
                .to_ascii_lowercase(),
            user: left.rsplit_once('@').map(|(u, _)| u.to_owned()),
            segments: segments(right),
        });
    } else {
        return None;
    };
    if !matches!(
        scheme.as_str(),
        "https" | "http" | "ssh" | "git" | "git+ssh"
    ) {
        return None;
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u.split(':').next().unwrap_or(u).to_owned()), h),
        None => (None, authority),
    };
    let host = if scheme.starts_with("http") {
        hostport.to_ascii_lowercase()
    } else {
        hostport
            .split(':')
            .next()
            .unwrap_or(hostport)
            .to_ascii_lowercase()
    };
    Some(RemoteUrl {
        scheme: if scheme.contains("ssh") {
            "ssh".into()
        } else {
            scheme
        },
        host,
        user,
        segments: segments(path),
    })
}

fn segments(path: &str) -> Vec<String> {
    let mut s: Vec<String> = path
        .split('/')
        .filter(|p| !p.is_empty())
        .map(crate::util::decode)
        .collect();
    if let Some(last) = s.last_mut()
        && let Some(stripped) = last.strip_suffix(".git")
    {
        *last = stripped.to_owned();
    }
    s
}

/// The host without its port.
fn bare(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

/// The built-in family of a well-known host.
pub fn known_family(host: &str) -> Option<Family> {
    let h = bare(host);
    Some(match h {
        "github.com" | "ssh.github.com" | "www.github.com" => Family::GitHub,
        "gitlab.com" | "altssh.gitlab.com" => Family::GitLab,
        "dev.azure.com" | "ssh.dev.azure.com" | "vs-ssh.visualstudio.com" => Family::AzureDevOps,
        h if h.ends_with(".visualstudio.com") => Family::AzureDevOps,
        "codeberg.org" | "next.forgejo.org" | "code.forgejo.org" => Family::Forgejo,
        "gitea.com" => Family::Gitea,
        "tangled.org" | "tangled.sh" => Family::Tangled,
        h if h.ends_with(".tangled.sh") || h.ends_with(".tangled.org") => Family::Tangled,
        _ => return None,
    })
}

/// The `forge.hosts` entry for `host` (with or without its port).
pub fn host_entry<'a>(host: &str, hosts: &'a [HostEntry]) -> Option<&'a HostEntry> {
    hosts
        .iter()
        .find(|e| e.host.eq_ignore_ascii_case(host))
        .or_else(|| {
            hosts
                .iter()
                .find(|e| bare(&e.host).eq_ignore_ascii_case(bare(host)))
        })
}

/// A probe's answer: the family and its API base.
pub type Probed = Option<(Family, Option<String>)>;

/// Tangled's appview API (the `sh.tangled.*` queries).
pub const TANGLED_API: &str = "https://api.tangled.org";

/// Detect the repository of `url`. `probe` answers the family (and API base) of a host neither the built-in list
/// nor `hosts` names; without it such a host is `none`.
pub fn detect(
    url: &str,
    hosts: &[HostEntry],
    probe: Option<&dyn Fn(&str) -> Probed>,
) -> Repository {
    let none = |host: String| Repository {
        family: Family::None,
        host,
        remote_url: Some(url.to_owned()),
        ..Default::default()
    };
    let Some(r) = parse_remote(url) else {
        return none(String::new());
    };
    let (family, api) = match host_entry(&r.host, hosts) {
        Some(e) => (e.family, e.api.clone()),
        None => match known_family(&r.host) {
            Some(f) => (f, None),
            None => match probe.and_then(|p| p(&r.host)) {
                Some((f, api)) => (f, api),
                None => return none(r.host),
            },
        },
    };
    let mut repo = match family {
        Family::AzureDevOps => azure(&r),
        Family::None => None,
        _ => owner_name(&r, family),
    }
    .unwrap_or_else(|| none(r.host.clone()));
    if repo.family == Family::None {
        return repo;
    }
    repo.remote_url = Some(url.to_owned());
    let web_host = web_host(&r, family);
    repo.api = api.unwrap_or_else(|| default_api(family, &web_host, &r, &repo));
    repo.host = web_host.clone();
    repo.web_url = match family {
        Family::AzureDevOps => format!(
            "{}/{}/_git/{}",
            azure_web_base(&r, &repo),
            repo.project.as_deref().unwrap_or(""),
            crate::util::encode(&repo.name)
        ),
        Family::Tangled => format!(
            "https://{}/{}/{}",
            tangled_web(&web_host),
            repo.owner,
            repo.name
        ),
        _ => format!("https://{web_host}/{}", repo.path),
    };
    repo
}

fn tangled_web(host: &str) -> &str {
    if host == "tangled.sh" || host.ends_with(".tangled.sh") || host.ends_with(".tangled.org") {
        "tangled.org"
    } else {
        host
    }
}

/// The host the person and the credential store know the forge by: ssh hosts map to their web host.
fn web_host(r: &RemoteUrl, family: Family) -> String {
    let h = r.host.as_str();
    match (family, h) {
        (Family::GitHub, "ssh.github.com" | "www.github.com") => "github.com".into(),
        (Family::GitLab, "altssh.gitlab.com") => "gitlab.com".into(),
        (Family::AzureDevOps, "ssh.dev.azure.com" | "vs-ssh.visualstudio.com") => {
            "dev.azure.com".into()
        }
        (Family::AzureDevOps, h) if h.ends_with(".visualstudio.com") => "dev.azure.com".into(),
        (Family::Tangled, "tangled.sh") => "tangled.org".into(),
        (Family::Tangled, h) if h.ends_with(".tangled.sh") || h.ends_with(".tangled.org") => {
            "tangled.org".into()
        }
        _ => h.to_owned(),
    }
}

fn owner_name(r: &RemoteUrl, family: Family) -> Option<Repository> {
    let segs = &r.segments;
    let (owner, name) = match family {
        Family::GitLab => {
            if segs.len() < 2 {
                return None;
            }
            let name = segs.last()?.clone();
            (segs[..segs.len() - 1].join("/"), name)
        }
        Family::Tangled => match segs.as_slice() {
            // A knot's url names the repository by its DID.
            [did] if did.starts_with("did:") => (String::new(), did.clone()),
            [owner, name, ..] => (owner.trim_start_matches('@').to_owned(), name.clone()),
            _ => return None,
        },
        _ => {
            if segs.len() < 2 {
                return None;
            }
            (segs[0].clone(), segs[1].clone())
        }
    };
    let path = if owner.is_empty() {
        name.clone()
    } else {
        format!("{owner}/{name}")
    };
    Some(Repository {
        family,
        host: r.host.clone(),
        owner,
        project: None,
        name,
        path,
        ..Default::default()
    })
}

/// Azure DevOps: `https://dev.azure.com/{org}/{project}/_git/{repo}`, `https://{org}.visualstudio.com/[DefaultCollection/]{project}/_git/{repo}`,
/// `git@ssh.dev.azure.com:v3/{org}/{project}/{repo}`, `{org}@vs-ssh.visualstudio.com:v3/{org}/{project}/{repo}`,
/// and a Server's `https://host/[tfs/]{collection}/{project}/_git/{repo}`.
fn azure(r: &RemoteUrl) -> Option<Repository> {
    let s = &r.segments;
    let (org, project, name) = if r.scheme == "ssh" {
        // v3/{org}/{project}/{repo}
        let i = s.iter().position(|x| x == "v3")?;
        (
            s.get(i + 1)?.clone(),
            s.get(i + 2)?.clone(),
            s.get(i + 3)?.clone(),
        )
    } else {
        let git = s.iter().position(|x| x == "_git")?;
        let name = s.get(git + 1)?.clone();
        let host = bare(&r.host);
        if let Some(org) = host.strip_suffix(".visualstudio.com") {
            // [DefaultCollection/]{project}/_git/{repo}, or _git/{repo} when the project shares the repo's name.
            let project = if git >= 1 {
                s[git - 1].clone()
            } else {
                name.clone()
            };
            (org.to_owned(), project, name)
        } else if git >= 2 {
            (s[..git - 1].join("/"), s[git - 1].clone(), name)
        } else if git == 1 {
            // {org}/_git/{repo}: the project has the repository's name.
            (s[0].clone(), name.clone(), name)
        } else {
            return None;
        }
    };
    Some(Repository {
        family: Family::AzureDevOps,
        host: r.host.clone(),
        path: format!("{org}/{project}/{name}"),
        owner: org,
        project: Some(project),
        name,
        ..Default::default()
    })
}

fn azure_web_base(r: &RemoteUrl, repo: &Repository) -> String {
    let h = bare(&r.host);
    if h == "dev.azure.com" || h == "ssh.dev.azure.com" || h.ends_with("visualstudio.com") {
        format!("https://dev.azure.com/{}", repo.owner)
    } else {
        format!("https://{}/{}", r.host, repo.owner)
    }
}

/// The API base a family uses on a host when `forge.hosts` gives none.
fn default_api(family: Family, web_host: &str, r: &RemoteUrl, repo: &Repository) -> String {
    match family {
        Family::GitHub if web_host == "github.com" => "https://api.github.com".into(),
        Family::GitHub => format!("https://{web_host}/api/v3"),
        Family::GitLab => format!("https://{web_host}/api/v4"),
        Family::AzureDevOps => azure_web_base(r, repo),
        Family::Forgejo | Family::Gitea => format!("https://{web_host}/api/v1"),
        Family::Tangled => TANGLED_API.into(),
        Family::None => String::new(),
    }
}

/// The version probe for an unknown host, through `transport`: GitLab, then Forgejo or Gitea, then GitHub
/// Enterprise Server. `base` is the host's root url (`https://host`). One to three requests; the hub caches the
/// answer per host.
pub fn probe(transport: &dyn Transport, base: &str) -> Option<(Family, Option<String>)> {
    let cancel = Cancel::new();
    let get = |path: &str| {
        transport
            .send(
                &Request::new(Method::Get, format!("{base}{path}"))
                    .header("Accept", "application/json"),
                &cancel,
            )
            .ok()
    };
    let json_of = |r: &crate::http::Response| r.json().ok().filter(|v| v.is_object());
    if let Some(r) = get("/api/v4/version") {
        // gitlab.com answers 401 {"message":"401 Unauthorized"} without a token: still GitLab.
        let ok = match r.status {
            200 => json_of(&r).is_some_and(|v| v.get("version").is_some()),
            401 => json_of(&r).is_some_and(|v| {
                v.get("message").and_then(|m| m.as_str()) == Some("401 Unauthorized")
            }),
            _ => false,
        };
        if ok {
            return Some((Family::GitLab, Some(format!("{base}/api/v4"))));
        }
    }
    if let Some(r) = get("/api/v1/version")
        && r.status == 200
        && let Some(v) = json_of(&r)
        && let Some(version) = v.get("version").and_then(|x| x.as_str())
    {
        let forgejo = version.contains("+gitea-")
            || get("/api/forgejo/v1/version").is_some_and(|r| r.status == 200);
        let family = if forgejo {
            Family::Forgejo
        } else {
            Family::Gitea
        };
        return Some((family, Some(format!("{base}/api/v1"))));
    }
    if let Some(r) = get("/api/v3/meta")
        && r.status == 200
        && json_of(&r).is_some_and(|v| {
            v.get("installed_version").is_some()
                || v.get("verifiable_password_authentication").is_some()
        })
    {
        return Some((Family::GitHub, Some(format!("{base}/api/v3"))));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(url: &str) -> Repository {
        detect(url, &[], None)
    }

    #[test]
    fn github_urls() {
        for url in [
            "https://github.com/sand-head/Eludite.git",
            "git@github.com:sand-head/Eludite.git",
            "ssh://git@ssh.github.com:443/sand-head/Eludite",
            "https://user:x@github.com/sand-head/Eludite",
        ] {
            let r = d(url);
            assert_eq!(r.family, Family::GitHub, "{url}");
            assert_eq!(
                (r.owner.as_str(), r.name.as_str()),
                ("sand-head", "Eludite"),
                "{url}"
            );
            assert_eq!(r.host, "github.com");
            assert_eq!(r.api, "https://api.github.com");
            assert_eq!(r.web_url, "https://github.com/sand-head/Eludite");
        }
    }

    #[test]
    fn an_enterprise_host_from_the_settings() {
        let hosts = vec![HostEntry {
            host: "git.corp.example".into(),
            family: Family::GitHub,
            api: None,
        }];
        let r = detect("git@git.corp.example:team/app.git", &hosts, None);
        assert_eq!(r.family, Family::GitHub);
        assert_eq!(r.api, "https://git.corp.example/api/v3");
        assert_eq!(r.path, "team/app");
        assert_eq!(
            detect("https://git.corp.example/team/app", &[], None).family,
            Family::None
        );
    }

    #[test]
    fn gitlab_groups_and_a_probed_host() {
        let r = d("https://gitlab.com/gitlab-org/frontend/ui.git");
        assert_eq!(r.family, Family::GitLab);
        assert_eq!(
            (r.owner.as_str(), r.name.as_str()),
            ("gitlab-org/frontend", "ui")
        );
        assert_eq!(r.api, "https://gitlab.com/api/v4");
        let probe = |h: &str| (h == "git.example.org").then_some((Family::GitLab, None));
        let r = detect("git@git.example.org:a/b.git", &[], Some(&probe));
        assert_eq!(r.family, Family::GitLab);
        assert_eq!(r.api, "https://git.example.org/api/v4");
    }

    #[test]
    fn azure_devops_urls() {
        for (url, api) in [
            (
                "https://dev.azure.com/contoso/Fabrikam/_git/web",
                "https://dev.azure.com/contoso",
            ),
            (
                "https://contoso@dev.azure.com/contoso/Fabrikam/_git/web",
                "https://dev.azure.com/contoso",
            ),
            (
                "git@ssh.dev.azure.com:v3/contoso/Fabrikam/web",
                "https://dev.azure.com/contoso",
            ),
            (
                "https://contoso.visualstudio.com/Fabrikam/_git/web",
                "https://dev.azure.com/contoso",
            ),
            (
                "https://contoso.visualstudio.com/DefaultCollection/Fabrikam/_git/web",
                "https://dev.azure.com/contoso",
            ),
            (
                "contoso@vs-ssh.visualstudio.com:v3/contoso/Fabrikam/web",
                "https://dev.azure.com/contoso",
            ),
        ] {
            let r = d(url);
            assert_eq!(r.family, Family::AzureDevOps, "{url}");
            assert_eq!(r.owner, "contoso", "{url}");
            assert_eq!(r.project.as_deref(), Some("Fabrikam"), "{url}");
            assert_eq!(r.name, "web", "{url}");
            assert_eq!(r.api, api, "{url}");
            assert_eq!(r.host, "dev.azure.com", "{url}");
            assert_eq!(r.web_url, "https://dev.azure.com/contoso/Fabrikam/_git/web");
        }
        let hosts = vec![HostEntry {
            host: "tfs.corp.example".into(),
            family: Family::AzureDevOps,
            api: None,
        }];
        let r = detect(
            "https://tfs.corp.example/tfs/DefaultCollection/Payroll/_git/api",
            &hosts,
            None,
        );
        assert_eq!(r.owner, "tfs/DefaultCollection");
        assert_eq!(r.project.as_deref(), Some("Payroll"));
        assert_eq!(r.api, "https://tfs.corp.example/tfs/DefaultCollection");
    }

    #[test]
    fn forgejo_gitea_and_tangled() {
        let r = d("https://codeberg.org/forgejo/forgejo.git");
        assert_eq!(r.family, Family::Forgejo);
        assert_eq!(r.api, "https://codeberg.org/api/v1");
        let r = d("git@gitea.com:gitea/tea.git");
        assert_eq!(r.family, Family::Gitea);
        let r = d("https://tangled.sh/@tangled.org/core");
        assert_eq!(r.family, Family::Tangled);
        assert_eq!((r.owner.as_str(), r.name.as_str()), ("tangled.org", "core"));
        assert_eq!(r.host, "tangled.org");
        assert_eq!(r.api, TANGLED_API);
        assert_eq!(r.web_url, "https://tangled.org/tangled.org/core");
        let r = d("https://knot1.tangled.sh/did:plc:j5hmlfdrwkvtxm7cjmu7j2is");
        assert_eq!(r.family, Family::Tangled);
        assert_eq!(r.name, "did:plc:j5hmlfdrwkvtxm7cjmu7j2is");
        let hosts = vec![HostEntry {
            host: "knot.example.net".into(),
            family: Family::Tangled,
            api: None,
        }];
        assert_eq!(
            detect("https://knot.example.net/did:plc:abc/repo", &hosts, None).family,
            Family::Tangled
        );
    }

    #[test]
    fn local_and_unknown_remotes_are_none() {
        assert_eq!(d("/srv/git/repo.git").family, Family::None);
        assert_eq!(d("file:///srv/git/repo.git").family, Family::None);
        assert_eq!(d("C:\\repos\\x").family, Family::None);
        let r = d("https://git.unknown.example/a/b");
        assert_eq!(
            (r.family, r.host.as_str()),
            (Family::None, "git.unknown.example")
        );
    }
}
