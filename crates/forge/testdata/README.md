# Forge fixtures (brief 0046)

One folder per forge, each with two kinds of exchange files (the format is in `crates/forge/src/replay.rs`):

- `recorded/`: what the real service answered to `eludite-forge`'s own requests, written by
  `tools/forge-corpus/record.sh` (through `crates/forge/tests/record.rs` and the recording transport), scrubbed
  (no request headers, so no token; email addresses, full names and avatars blanked; token-shaped strings replaced;
  strings cut at 4,000 characters and arrays at 50). Recorded on 2026-10-04 without a token from public
  repositories: codeberg.org `forgejo/forgejo` (Forgejo 16.0.0-dev), gitlab.com `gitlab-org/cli`, dev.azure.com
  `dnceng-public/public/dotnet-public-wiki`, tangled.org `tangled.org/core` (through `api.tangled.org` and
  `plc.directory`). Each file's `source` says where and when.
- `synthesized/`: written by `tools/forge-corpus/synthesize.py` from the pinned API descriptions in
  `protocol/forge/`, for what could not be recorded here: every write, the sign-in flows, what gitlab.com and Azure
  DevOps show only to a signed-in reader (discussions, notes, job logs; iterations, policies, work items), and all of
  **GitHub**, whose API (api.github.com) was not reachable from the machine brief 0046 ran on. Each file's `source`
  says "synthesized"; the owner's recorder run with tokens replaces them.

A request matches the fixture naming the most of its query and body; `{{base}}` is the fixture server's url. The
tests (`crates/forge/tests/*.rs`) serve a forge's folder from a loopback server and map the forge's host to it with
`forge.hosts`, so they run everywhere with no network (a fixture test that reaches a real host fails).
