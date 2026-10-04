# GitLab REST API description (pinned, not copied)

`crates/forge/src/gitlab.rs` calls the REST API v4. GitLab's OpenAPI description is generated into its repository's
`doc/` folder, and GitLab's `LICENSE` puts everything under `doc/` under CC BY-SA 4.0, not MIT. Brief 0046 expected
it to be MIT; since `protocol/` is MIT, the file is pinned here by commit and checksum rather than copied, and
`PIN` says how to fetch exactly that file. The OpenAPI v2 description also covers only part of the API (merge
request discussions, draft notes and approvals are documented in GitLab's Markdown pages, not in it); the fixture
tests carry the shapes the code reads.
