# Brief 0045 report: Git over https and ssh

Status: done on Linux. Windows and macOS: not run (no machines); what their builds use is in section 3. CI: not run
(nothing pushed).
Branch: `brief/0045-git-https-and-ssh`, based on `main` at `ed8f1a1`; not rebased (the coordinator merges). Date:
2026-10-04.
Brief: [0045-git-https-and-ssh.md](0045-git-https-and-ssh.md).

## 1. Summary

- **git2's `https` and `ssh` features are on** (`Cargo.toml`: `default-features = false, features = ["https", "ssh"]`),
  with libgit2 still bundled (1.9.7 through libgit2-sys 0.18.8) and libssh2 bundled too (libssh2-sys 0.3.3 builds its
  own copy; the system's libssh2 is not used). No `vendored` OpenSSL. `Cargo.lock` gains `openssl-sys`,
  `openssl-probe` and `libssh2-sys`, nothing else (section 4).
- **Fetch, pull and push work over `https://` and ssh**, proven against servers on loopback with no network:
  `git http-backend` behind a small HTTP listener demanding basic authentication, `openssl s_server` in front of it
  for TLS, and a loopback `sshd` (section 5).
- **The credential callback** (`crates/git/src/credentials.rs`) tries, per libgit2's request: for ssh the user (the
  url's, else the local user, as `ssh` does), the ssh agent, then the key files in `~/.ssh` in OpenSSH's order
  (`id_rsa`, `id_ecdsa`, `id_ed25519`; a key with a passphrase is skipped and named: it needs the agent); for http(s)
  the configured `credential.helper`, then the system's default credentials (Negotiate and NTLM servers), then a user
  name and password or token the shell supplies from its prompt. When nothing answers, the transfer fails with
  `credentials_required: <host> ...` (the prompt can answer) or, for a key-only ssh server, `Authentication failed for
  <url>: tried ...` (it cannot). Each message says what was tried.
- **The credential prompt** (`git.credentialPrompt`, `crates/eludite/src/shell/git/credentials.rs`): when the person's
  Fetch, Pull, Push, Sync or Commit and Push fails with `credentials_required`, a dialog asks for the host's user name
  and password or token, with "Remember for this session"; OK runs the command again with the answer. A refused answer
  reopens it saying so; Cancel says "Fetch canceled: no credentials for <host>". Answers live in memory only
  (`SessionCredentials`): a one-time answer is dropped when its transfer ends, a remembered one when the workspace
  closes. Nothing is written.
- **Agents never see the prompt.** The same command answers an agent with `credentials_required: <host> ...` and
  "Agents cannot answer the credential prompt: ask the person to run this in Eludite ... do not retry it another way."
  A credential the person remembered for the session is offered to an agent's transfer too, as a credential helper's
  would be (section 7.3).
- **Certificates**: libgit2's own check, with `http.sslVerify = false` honored as git honors it; a refused certificate
  names the host ("The certificate of 127.0.0.1:41863 could not be verified (...)"), and an unknown ssh host key does
  too ("The ssh host key of 127.0.0.1 is not trusted ..."). The Git Changes window shows a warning line while
  `http.sslVerify` is false ("⚠ http.sslVerify is false: the certificates of this repository's https remotes are not
  checked"); the watcher now also watches `.git/config`, so the line follows a change within its debounce.
- **Proxy**: for an https remote, `http.proxy` from git config (empty: none, as in git), else `HTTPS_PROXY`,
  `https_proxy`, `HTTP_PROXY`, else none; `NO_PROXY`/`no_proxy` and loopback hosts go direct. Plain `http://` gets no
  proxy because libgit2 1.9 does not use one for it (section 7.1).
- **No `git` process in the product** (the test servers start `git http-backend`, `openssl`, `ssh-keygen` and
  `sshd`). The one exception is unchanged from brief 0040: a credential helper named without a path (`store`,
  `manager`) is run by git2 as `git credential-<name>`, only when an http remote asks for credentials.
- **Build time** of `crates/git` (section 8): switching the features on rebuilt git2's dependency graph in 54 s (dev,
  sccache warm for the Rust crates); a cold release build of `eludite-git` and its dependencies took 3 min 17 s under a
  load average of about 8, of which libgit2's C build script ran 122 s and libssh2's 31 s; the crate itself rebuilds
  in 2.4 s.
- **Tests**: `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` **995 passed, 0 failed,
  1 ignored**, with `ELUDITE_TEST_SSHD=/usr/sbin/sshd` so the ssh test ran (section 10). `cargo fmt --check`,
  `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings` and
  `dotnet build dotnet/Eludite.slnx` (0 warnings) clean.

## 2. What was built

`crates/git`:

- `credentials.rs` (new): `UserPass` (its `Debug` hides the password), `SessionCredentials` (one-time and remembered
  answers by host, shared by clones, cleared on demand), `remote_host` and `remote_scheme` (`https://h:8443/r` gives
  `h:8443`, `git@h:r` gives `h` and `ssh`, local paths give none), `ssh_key_candidates` and `key_has_passphrase`
  (OpenSSH's own format by its cipher field, PEM `Proc-Type: 4,ENCRYPTED`, PKCS#8 `ENCRYPTED`), the `Sources` trait
  the callback reads, and `CredentialState::next`, the order above as a state machine that returns an `Attempt` or
  the failure. The constants `CREDENTIALS_REQUIRED` and `CREDENTIAL_PROMPT` name the answer and the dialog.
- `transport.rs` (new): `proxy_for(url, http.proxy, env)`, `no_proxy_matches` (`*`, hosts, domain suffixes with or
  without a leading dot, addresses, IPv4 ranges), `certificate_refusal(host, ssh, detail)`.
- `remote.rs`: the callback reads `LiveSources` (the environment, the effective git config, the session's
  credentials) and turns each `Attempt` into a `Cred`; `certificate_check` accepts any X.509 certificate while
  `http.sslVerify` is false and passes everything else to libgit2 (ssh host keys stay libgit2's, against
  `~/.ssh/known_hosts`); fetch and push set `ProxyOptions` from `proxy_for`; `transfer_error` prefers the callback's
  own failure, then maps a certificate error to the refusal naming the host.
- `error.rs`: `ErrorKind::CredentialsRequired` and `ErrorKind::Certificate`; `GitError` gains `host` and `refused`.
- `lib.rs`: `Repo::with_credentials` and `credentials`, `Repo::effective_config` (the repository's config and the
  global configuration the handle reads: the user's, or exactly the test's files), `Repo::ssl_verify`.
- `status.rs`: `Status::ssl_verify_off`. `watch.rs`: `config` joins the stat-ed files.

`crates/eludite`:

- `shell/git/service.rs`: the service owns the session's `SessionCredentials`, gives them to the repository it
  watches, clears them when the workspace changes or closes; a transfer's `credentials_required` forgets a refused
  answer and, for an agent caller (`current_caller()`), adds `AGENT_CANNOT_ANSWER`; `credentials_required_host` parses
  the host back out of the answer.
- `shell/git/credentials.rs` (new): the dialog (`dialog_panel`, two text boxes, the password as bullets, the check
  box, OK and Cancel; Tab, Enter, Escape).
- `shell/git.rs`: `git_finished` opens the prompt for the person's command that failed with `credentials_required`
  (one prompt at a time), `git_credential_answer` supplies the answer and runs the command again, or reports the
  cancel; a one-time answer is forgotten when the retry finishes.
- `shell/git/changes.rs`: the warning line (`SSL_WARNING`) from `ChangesModel::ssl_verify_off`.
- `shell.rs`: one line, the prompt rendered beside the Rename dialog.

`docs/agents/git.md`: the limits paragraph now says https and ssh work, and "Agents cannot answer credential prompts;
the person does: a fetch, pull or push no credential helper or ssh key answers fails with `credentials_required` and
the host, so tell the user rather than retrying." (721 words; the guide test's bound is 800.)

## 3. Which transport each OS uses

| OS | https | ssh |
|---|---|---|
| Linux | libgit2's OpenSSL stream on the system's OpenSSL (`libssl-dev`, found by `openssl-sys` through pkg-config, linked dynamically; 3.0.13 here); `openssl-probe` points it at the system's certificate store | the bundled libssh2 (1.11, built by libssh2-sys) with OpenSSL for its crypto |
| macOS | libgit2's SecureTransport stream (the Security framework); `openssl-sys` is still compiled, for libssh2 | the bundled libssh2 with OpenSSL for its crypto: `openssl-sys` finds the runner's Homebrew `openssl@3` |
| Windows | libgit2's WinHTTP transport (Schannel underneath); no OpenSSL | the bundled libssh2 with its WinCNG crypto backend; no OpenSSL |

Only the Linux row was built and run here. The other two rows are what libgit2-sys's and libssh2-sys's build scripts
and git2's target-specific dependencies select (git2 takes `openssl-sys` and `openssl-probe` only on Unix other than
macOS; libgit2-sys defines `GIT_WINHTTP`, `GIT_SECURE_TRANSPORT` or `GIT_OPENSSL`; libssh2-sys defines
`LIBSSH2_WINCNG` on Windows). The macOS build depends on OpenSSL being findable, which GitHub's macOS runners give
through Homebrew; if CI says otherwise, `.github/workflows/ci.yml` is the place (the brief allows it), and no change
was made there.

## 4. Packages and dependencies

- **Linux packages**: unchanged. `libssl-dev` (already listed in CLAUDE.md, README.md and CI) supplies OpenSSL;
  libssh2 is bundled, so CLAUDE.md's package line keeps no `libssh2`. `libgit2-dev` stays listed though libgit2 is
  bundled (as in brief 0040: the system's 1.7.2 is older than libgit2-sys 0.18 accepts).
- **New in `Cargo.lock`** (through git2's features only; the PR description should carry these):

  | Crate | Version | SPDX | Why |
  |---|---|---|---|
  | `openssl-sys` | 0.9.117 | MIT | libgit2's TLS on Linux, libssh2's crypto on Unix |
  | `openssl-probe` | 0.1.6 | MIT OR Apache-2.0 | finds the system's certificate store on Linux |
  | `libssh2-sys` | 0.3.3 | MIT OR Apache-2.0 | builds the bundled libssh2 (BSD-3-Clause) |

  `libgit2-sys` gains the edges to `openssl-sys` and `libssh2-sys`; `libssh2-sys` uses `cc`, `libc`, `libz-sys`,
  `pkg-config` and `vcpkg`, all already in the lock. `openssl-src` is absent (no `vendored`). All compatible with
  GPL-3.0-or-later (ADR-0005).

## 5. The test servers

All in `crates/git/tests/support/server.rs` (std only, no new dependency), shared by `crates/git/tests/remote.rs` and
the shell's `git_tests.rs` (included there with `#[path]`):

- **Smart HTTP**: a `std::net::TcpListener` on 127.0.0.1, a thread per connection, HTTP/1.1 keep-alive with
  `Content-Length` and chunked request bodies. A request without the right `Authorization: Basic` header gets 401 with
  `WWW-Authenticate: Basic`; an authorized one runs `git http-backend` as CGI (`GIT_PROJECT_ROOT`,
  `GIT_HTTP_EXPORT_ALL`, `REMOTE_USER` so receive-pack is on, `HOME` in the test folder and `GIT_CONFIG_NOSYSTEM` so the
  machine's git config stays out). Every request is logged (method, target, user, status) for the tests to assert on.
  Requests in proxy form are answered too.
- **TLS**: `openssl req -x509 -newkey rsa:2048 -nodes -subj /CN=localhost` writes a self-signed certificate into a
  temporary folder; `openssl s_server -quiet -accept 127.0.0.1:<port>` terminates TLS and hands the decrypted bytes to
  the same request loop through its standard output and input (`-quiet` prints nothing else and turns off its
  single-letter commands). One connection at a time, kept alive, which is how libgit2 uses it. It skips on Windows:
  `s_server` there reads standard input as a console, not a pipe.
- **CONNECT proxy**: a listener that answers `CONNECT host:port` with 200 and relays the tunnel to the TLS server,
  logging each target.
- **sshd** (`crates/git/tests/ssh.rs`): runs only when `ELUDITE_TEST_SSHD` names an `sshd` binary by absolute path.
  It makes a host key and a user key with `ssh-keygen`, starts `sshd -D -e -f <config>` on a free loopback port
  (only that key authorized, `StrictModes no`, `UsePAM no`), and runs the git half in a child process of the test
  binary with `HOME` in the temporary folder and no `SSH_AUTH_SOCK`, so libgit2 reads that folder's `.ssh` and the
  callback takes the key-file path. Run here with OpenSSH 9.6p1 (`openssh-server` installed for the run; running as
  root needs `/run/sshd` to exist).

Each server skips its tests (with a line on stderr) when `git`, `openssl` or `ssh-keygen` is not on `PATH`.

## 6. Tests

What each proves:

- **`crates/git` unit** (`credentials.rs`, 7; `transport.rs`, 3; `remote.rs`, 2 updated):
  - https tries **the helper, then the default credentials, then the prompt's answer, then fails refused**, and asks
    no ssh source; **each https source absent falls through** (no helper: `credentials_required`, not refused; a
    helper with no answer: the prompt's answer next; a helper's answer refused: `credentials_required`; the default
    credentials once, and a Negotiate-only server fails as `Credentials`, which the prompt cannot answer);
  - ssh asks **the user, then the agent, then each key file, then fails without a prompt** on a key-only server,
    naming the keys tried and the one skipped for its passphrase; without an agent or keys, a password server reaches
    the prompt's answer;
  - **key selection**: OpenSSH's order, the `.pub` paired when present, encrypted keys (OpenSSH, PEM, PKCS#8) skipped;
  - hosts and schemes of 12 url shapes; the session store's one-time and remembered answers, sharing, `forget`,
    `clear`, and a `Debug` that hides passwords;
  - **the proxy order**: `http.proxy` first, an empty one turning it off, then `HTTPS_PROXY`, `https_proxy`,
    `HTTP_PROXY`, else none; plain http, ssh, `git://` and local remotes get none; **`NO_PROXY` and loopback go
    direct** (hosts, suffixes, CIDR, IPv6), for `http.proxy` too; the refusals name the host.
- **`crates/git/tests/remote.rs`** (4 new, the brief 0040 refusal test removed):
  - `an_http_remote_asking_for_credentials_takes_the_prompts_answer`: nothing answers → `credentials_required` with the
    host after only 401s; a wrong answer → refused; the right one remembered → fetch, pull and push as alice (the
    server saw receive-pack authorized);
  - `the_configured_credential_helper_answers_before_the_prompt`: a `!` helper answers although a wrong prompt answer
    is kept;
  - `http_proxy_carries_an_https_transfer`: an https remote on a host that does not resolve is fetched through
    `http.proxy` (the proxy logged `CONNECT git.example.invalid:443`); an empty `http.proxy` turns it off;
  - `https_with_a_self_signed_certificate_needs_ssl_verify_off`: **refused naming the host with `sslVerify` on**,
    nothing sent; `Status::ssl_verify_off` follows the config; with it off, **fetch, pull and push over TLS**, the
    server's first answer a 401 the callback answered.
- **`crates/git/tests/ssh.rs`**: an unknown host key is refused naming the host; with it in `known_hosts`, **fetch
  and push over ssh with `~/.ssh/id_ed25519`** and no agent (the bare remote has the pushed commit); with the key
  gone, `Credentials` naming "no key file in ~/.ssh". Skipped without `ELUDITE_TEST_SSHD`.
- **`crates/eludite` headless** (`shell/git_tests.rs`, 3 new, against the http and https test servers):
  - `the_credential_prompt_asks_the_user_and_an_agent_is_refused`: **an agent's fetch is refused with
    `credentials_required`, the host and the sentence for agents, and no prompt opens**; the person's Fetch opens the
    prompt for the host; a wrong password reopens it marked refused; the right one, not remembered, fetches and is not
    kept; Cancel runs nothing and says why;
  - `a_remembered_credential_is_reused_in_the_session_and_forgotten_at_close`: remembered, **the person's Push and an
    agent's fetch run without asking**; closing the solution forgets it, and Fetch asks again after reopening;
  - `the_warning_line_follows_ssl_verify_and_a_refused_certificate_names_its_host`: Fetch from a self-signed https
    remote shows "The certificate of 127.0.0.1:<port> could not be verified" in the window and the status bar, no
    prompt; **the warning line appears when `http.sslVerify` is set false and goes when it is removed**.

## 7. Deviations and decisions

1. **Plain `http://` gets no proxy.** libgit2 1.9's http client tunnels only https through a proxy (CONNECT); for
   http it writes proxy-form requests but connects to the server itself (`httpclient.c`, `use_connect_proxy`), so a
   proxy for http would do nothing useful. The brief's order applies to https remotes, which is where proxies matter.
   Loopback hosts are never proxied (a proxy cannot reach the machine's loopback), and `NO_PROXY` is honored, as curl
   honors it for git; neither is in the brief, both keep this container's own `HTTPS_PROXY` from swallowing the
   loopback test servers.
2. **"Forgotten at close"** is read as the workspace closing (or another opening, or `git.enabled` turned off): the
   service clears the store then, and of course at exit.
3. **A remembered credential serves agents too.** The prompt is never shown to an agent, but once the person ticked
   "Remember for this session", an agent's fetch or push to that host uses it, as a configured credential helper's
   answer would be used. Push stays class dangerous for agents (`git.push: prompt`), so the person still approves each
   push.
4. **The ssh host key** is libgit2's to check against `~/.ssh/known_hosts` (passthrough); an unknown key is refused
   with the host and "connect once with `ssh`", rather than a trust-on-first-use dialog, which the brief does not ask
   for. `http.sslVerify = false` never affects ssh.
5. **ssh key passphrases** are not asked for: such a key is skipped and named (add it to the agent). The prompt
   answers user name and password requests only (https, and ssh servers that allow passwords).
6. **The key files** are OpenSSH's defaults only (`id_rsa`, `id_ecdsa`, `id_ed25519`); `~/.ssh/config`
   (`IdentityFile`, `Host` aliases, ports) is not read, and `core.sshCommand` is ignored, since libssh2 is not `ssh`.
7. **Files outside the brief's list**: `crates/eludite/src/shell/git/credentials.rs` (new, the dialog),
   `shell/git/service.rs` and `shell/git/changes.rs` (the store, the agent text, the warning line), `shell.rs` (one
   render line), `crates/git/tests/support/server.rs` and `crates/git/tests/ssh.rs` (new test files).
8. **CI**: `ELUDITE_TEST_SSHD` is not set in `.github/workflows/ci.yml` (the brief allows touching it only for a
   build package). GitHub's Ubuntu runners have `/usr/sbin/sshd`, so a later change could set it on the Linux job.

## 8. Build time of `crates/git`

This machine (4 cores, the other worktree's agent building beside, load average 4 to 8; sccache wraps rustc, not the
C compiler of build scripts):

- Turning the features on, dev profile: **54 s** wall to rebuild `eludite-git` and the git2 graph (libgit2-sys with
  OpenSSL and libssh2, libssh2-sys, openssl-sys, git2, `url` and its ICU crates, which git2 needs with `https`).
- Cold release build of `eludite-git` and every dependency (`cargo build -p eludite-git --release --timings`):
  **3 min 17 s** wall; libgit2-sys's build script (libgit2's C) **122.1 s**, libssh2-sys's **30.8 s**, `eludite-git`
  itself 10.6 s, git2 8.0 s. Brief 0040 already compiled libgit2's C; libssh2 and OpenSSL's glue are what this adds.
- `eludite-git` alone after a touch, dev: **2.4 s**.

## 9. For the merge

- Shared with the 0042 branch: `docs/briefs/README.md` (the 0045 row only), `Cargo.toml` (only the `git2` line),
  `Cargo.lock` (three new packages and two edges), `crates/eludite/src/shell.rs` (one line,
  `.children(self.git.prompt.clone())`, after the Rename dialog's). No settings key was added
  (`protocol/schemas/settings.json` and `crates/commands/src/settings.rs` untouched).
- Merge the branch as a whole: between `85fe338` (features on) and `4ba4d32` (which replaces it) brief 0040's refusal
  test fails, and the shell tests' commit is clippy-clean only with the one-line fix after it.
- Disk: about 7 GB free at the end of this run, with both worktrees' targets.

## 10. Counts

This machine, `DISPLAY=:99`, `CEF_PATH`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`, `ELUDITE_DBG_MONO`,
`ELUDITE_JS_DEBUG`, `/opt/node22/bin` on `PATH`, `ELUDITE_TEST_SSHD=/usr/sbin/sshd`, the corpus built
(`bash corpus/tests/build.sh`), `dotnet build dotnet/Eludite.slnx` first:

- `cargo test --workspace --no-fail-fast --features eludite-chromium/cef`: **995 passed, 0 failed, 1 ignored**. Of
  them `eludite-git`: 36 unit (26 before, 10 new), 8 in `remote.rs`, 1 in `ssh.rs` (ran),
  3 in `budget.rs`; the shell's git tests 24 (20 of brief 0040, 3 new, and the server's base64 check).
- `cargo fmt --check`: clean. `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`:
  clean. `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors.

## 11. How to reproduce

```
cargo test -p eludite-git                                        # unit, remote (http, https, proxy), budget
ELUDITE_TEST_SSHD=/usr/sbin/sshd cargo test -p eludite-git --test ssh
DISPLAY=:99 cargo test -p eludite --bins git_tests                # the shell, the prompt and the warning line
```
