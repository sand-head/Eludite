# GitHub REST API description

`crates/forge/src/github.rs` calls the REST API with `X-GitHub-Api-Version: 2022-11-28`; this is that version's
description, from GitHub's own repository (MIT, `LICENSE.md` beside it). GraphQL (review threads' resolution, draft
and ready, auto-merge, linked branches) has no description here: its four queries and mutations are written in
`github.rs` and their shapes are in the fixtures. api.github.com was not reachable where brief 0046 ran (only
raw.githubusercontent.com was), so the GitHub fixtures are synthesized from this description; the recorder replaces
them with recorded ones on a machine that reaches the API. Stored gzipped (13 MB uncompressed).
