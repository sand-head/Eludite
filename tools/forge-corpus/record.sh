#!/usr/bin/env bash
# Re-record brief 0046's forge fixtures (crates/forge/testdata/<forge>/recorded/) from the real services, through
# the forge crate's own requests (crates/forge/tests/record.rs drives the read scenario through a recording
# transport), then rewrite the synthesized ones (tools/forge-corpus/synthesize.py) and run the fixture tests.
#
#   tools/forge-corpus/record.sh [forgejo] [gitlab] [azure] [tangled] [github]
#
# With no forge named, every one is recorded. The read scenario uses public repositories (codeberg.org's
# forgejo/forgejo, gitlab.com's gitlab-org/cli, dev.azure.com's dnceng-public/public, tangled.org's core, github.com's
# cli/cli) and needs no token: request headers are never written, response headers are cut to the validators and
# paging, and the bodies are scrubbed (email addresses, full names, avatars, anything shaped like a token; strings
# cut at 4,000 characters and arrays at 50). Answers that need a token (gitlab.com's discussions and job logs, Azure
# DevOps' iterations, policies and work items) are not recorded; the synthesized fixtures cover them.
#
# The writes are exercised against throwaway repositories by the real-service tests (crates/forge/tests/live.rs),
# which run when the tokens are set:
#   ELUDITE_GITHUB_TOKEN ELUDITE_GITHUB_REPO
#   ELUDITE_GITLAB_TOKEN ELUDITE_GITLAB_HOST ELUDITE_GITLAB_REPO
#   ELUDITE_AZDO_TOKEN ELUDITE_AZDO_ORG ELUDITE_AZDO_PROJECT ELUDITE_AZDO_REPO
#   ELUDITE_FORGEJO_TOKEN ELUDITE_FORGEJO_HOST ELUDITE_FORGEJO_REPO
#   ELUDITE_TANGLED_APP_PASSWORD ELUDITE_TANGLED_HANDLE ELUDITE_TANGLED_REPO
# (and ELUDITE_<FORGE>_PR for an open pull request there). This script runs them after recording.
#
# Review the result with `git diff crates/forge/testdata` and commit it. Recordings are made only this way.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

only=""
for f in "$@"; do
  case "$f" in
    forgejo | gitlab | azure | tangled | github) only="${only:+$only,}$f" ;;
    *) echo "usage: $0 [forgejo] [gitlab] [azure] [tangled] [github]" >&2; exit 2 ;;
  esac
done

echo "recording: ${only:-every forge}"
ELUDITE_FORGE_RECORD="" ELUDITE_FORGE_RECORD_ONLY="$only" \
  cargo test -p eludite-forge --test record -- --nocapture
python3 tools/forge-corpus/synthesize.py
cargo test -p eludite-forge --test live -- --nocapture
cargo test -p eludite-forge
git status --short crates/forge/testdata
