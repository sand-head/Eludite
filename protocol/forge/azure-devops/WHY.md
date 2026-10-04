# Azure DevOps REST API 7.1 specifications

`crates/forge/src/azure.rs` sends `api-version=7.1` on every call (policy evaluations `7.1-preview.1`, work item
comments `7.1-preview.4`, as these specifications define them). The five areas it uses are here, from Microsoft's
specification repository (MIT, `LICENSE` beside them): Git (pull requests, threads, iterations, reviewers, commit
statuses), Work Item Tracking (WIQL, work items, comments, types and states), Build (builds, logs, retries), Policy
(evaluations) and Core (connection data).
