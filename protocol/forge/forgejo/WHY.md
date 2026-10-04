# Forgejo API description

`crates/forge/src/forgejo.rs` calls `/api/v1` on Forgejo and Gitea. This is the Swagger description codeberg.org
served when brief 0046 recorded its fixtures (its `info.license`: MIT, for interoperability). It is a development
build of Forgejo 16: the `actions/runs`, `actions/runs/{id}/jobs` and `actions/jobs/{id}/logs` endpoints the Checks
log uses are newer than Forgejo 11 and Gitea 1.22, which answer 404 there (the code then says the log is on the
status's page).
