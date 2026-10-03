# Brief 0045: Git over https and ssh

Status: open
Phase: 1 close-out (follow-up to brief 0040)
Plan reference: PLAN.md sections 2 (principle 1), 4.8 (no shelling out to `git` on hot paths), 10 (Phase 1 exit); brief 0040's report (the "not done" item: https and ssh remotes are refused by this build)
Related ADRs: ADR-0005 (licensing)
Depends on: brief 0040 (`crates/git`, the credential callback, the transports test).

## Goal

Fetch, pull and push work against the remotes people actually have: `https://` with a credential helper or a token, and `ssh://` and `git@host:` with the ssh agent or a key file. Brief 0040 built git2 with `default-features = false`, so libgit2 has no TLS and no ssh transport and refuses those remotes with a message. This brief turns on git2's `https` and `ssh` features, keeps the bundled libgit2, and proves both transports against local servers in the tests, with no network. The system `libssl-dev` that the Linux build already needs supplies TLS on Linux; macOS uses its own TLS through libgit2's SecureTransport backend; Windows uses WinHTTP and Schannel through libgit2, so no OpenSSL is needed there.

## Files in scope

- `Cargo.toml` (the workspace `git2` line gains `features = ["https", "ssh"]`; the SPDX ids of what that pulls in go in the PR: `openssl-sys` (MIT), `openssl-probe` (MIT OR Apache-2.0), `libssh2-sys` (MIT OR Apache-2.0, bundling libssh2 which is BSD-3-Clause), `openssl-src` only if the `vendored` feature is ever enabled, which this brief does not do), `Cargo.lock`.
- `crates/git/src/**` (the credential callback as brief 0040 left it: ssh agent, then the configured helper, then default credentials, then a username and password or token the shell supplies through a prompt; `git.credentialPrompt` is a shell dialog that the command's answer names when it was needed; certificate check: libgit2's default, with `http.sslVerify` honored and a refusal naming the host on a bad certificate; the proxy from `http.proxy` and the `HTTPS_PROXY` and `https_proxy` variables, so a corporate proxy and this repository's own environment work), `crates/git/tests/transports.rs` (brief 0040's refusal test becomes: a local https server (a tiny TLS listener on a loopback port with a self-signed certificate and `http.sslVerify=false` for the test repository only) serving a bare repository through the smart HTTP protocol via `git http-backend`... is a `git` process, which the no-`git`-process rule forbids in the product but not in a test harness; the test harness runs `git daemon` for `git://` already? check what brief 0040 did and keep the same stance: a test may start `git` to stand up a server, the product never does), an ssh server for the test: `sshd` is not available on CI runners by default; the ssh test uses libssh2 against a loopback sshd when `ELUDITE_TEST_SSHD` names one (skips otherwise) and always tests the key-file and agent credential selection logic with the callback in isolation).
- `crates/eludite/src/shell/git.rs` (the credential prompt dialog: user name and password or token, "remember for this session"; the certificate refusal message with the host), `crates/eludite/src/shell/git_tests.rs` (the prompt appears for the user, is refused for an agent with the message, the remembered credential is reused within the session and forgotten at close), `docs/agents/git.md` (one sentence: agents cannot answer credential prompts; the person does), `docs/briefs/0040-report.md` unchanged, `docs/briefs/0045-report.md` (new), `docs/briefs/README.md`, `CLAUDE.md` (the Linux packages line already lists `libssl-dev`; add `libssh2` only if the bundled one is not used), `README.md` only if the package list changes.
- `.github/workflows/ci.yml` only if Windows or macOS need a package for the build (they should not; the report says what happened).

## Contract

- `fetch`, `pull` and `push` against `https://` remotes work with a credential helper configured in git, with the ssh agent for `ssh://`, and with a prompt in the shell when neither answers; the prompt is never shown to an agent (its command is refused with `credentials_required` and the remote's host); the credential is kept in memory for the session only, never written.
- Certificate validation is libgit2's default; `http.sslVerify=false` in the repository's config is honored (as git does) and the window shows a warning line while it is set.
- Proxies: `http.proxy` from git config, else `HTTPS_PROXY`/`https_proxy`/`HTTP_PROXY` from the environment, else none.
- No `git` process is started by the product (a test harness may start one to stand up a server).
- Build: `cargo build --workspace` still works on a clean Linux machine with the packages CLAUDE.md lists; Windows and macOS CI jobs build without a new package.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/git`: fetch, pull and push through a loopback https server with a self-signed certificate and `sslVerify` off for the test repository; the certificate refusal with `sslVerify` on; the proxy selection order; the credential order with each source present and absent; the ssh path against `ELUDITE_TEST_SSHD` when set, else skipped with the key-selection logic tested alone.
- `crates/eludite`: the prompt and its refusal for agents, the session memory, the warning line.
- CI green on all three OSes with the features on.

## Budget

- No change to brief 0040's budgets; the build time of `crates/git` is reported.
- Dependencies: `openssl-sys`, `openssl-probe`, `libssh2-sys` as above, through git2's features; no `vendored` OpenSSL.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green; the Windows and macOS CI jobs build.
2. The report records which transport each OS uses, the package situation, and the test servers used.
3. The briefs index and `CLAUDE.md` match the repository.

## Out of scope

- Credential storage on disk (the OS keychain): a later brief; GPG and ssh commit signing; PR integration.
