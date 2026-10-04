# Tangled lexicons

`crates/forge/src/tangled.rs` reads Tangled through its appview's XRPC queries (`api.tangled.org`:
`sh.tangled.repo.listPulls`, `getPull`, `listIssues`, `getIssue`, `listRepos`, `getRepoByRepoDid`, `compare`,
`sh.tangled.repo.pull.listStatuses`, `sh.tangled.repo.issue.listStates`, `sh.tangled.feed.listComments`) and writes
records into the signed-in account's personal data server (`sh.tangled.feed.comment`, `sh.tangled.repo.issue`,
`sh.tangled.repo.issue.state`, `sh.tangled.repo.pull.status`). These are the lexicons for those, from Tangled's
monorepo at the pinned commit. The pull request record (`sh.tangled.repo.pull`) carries its patch as a gzipped blob
per round, so creating one needs a blob upload Eludite does not do; review, approval and line-comment lexicons do not
exist.
