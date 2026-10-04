# Forge API descriptions (brief 0046)

Reference only: nothing is generated from these. `crates/forge` hand-writes the subset of each forge's API it uses,
and its fixture tests check that subset against recorded (or, where nothing could be recorded, synthesized) responses.
Each folder pins one forge's description with a `PIN` (the version or commit, and a checksum) and a `WHY.md`.

| Folder | What | License |
|---|---|---|
| `github/` | GitHub's REST API description for API version 2022-11-28 (gzip of the OpenAPI JSON) | MIT |
| `gitlab/` | Pin only: GitLab's OpenAPI v2 description lives under GitLab's `doc/`, which is CC BY-SA 4.0, not MIT, so it is not copied into this MIT folder | CC BY-SA 4.0 (not copied) |
| `azure-devops/` | Azure DevOps REST API 7.1 specifications for Git, Work Item Tracking, Build, Policy and Core (vsts-rest-api-specs) | MIT |
| `forgejo/` | Forgejo's Swagger description, as codeberg.org served it | MIT ("for the purpose of interoperability") |
| `tangled/` | The `sh.tangled.*` lexicons Eludite reads and writes, and the `com.atproto` ones they reference | MIT |
