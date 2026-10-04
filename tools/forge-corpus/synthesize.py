#!/usr/bin/env python3
"""Write the synthesized forge fixtures (crates/forge/testdata/<forge>/synthesized/).

The recorder (record.sh) captures what the real services answer; what it cannot capture without a token or a
throwaway repository (every write, the sign-in flows, review threads gitlab.com hides from anonymous readers,
Azure DevOps' work items and policies, and all of GitHub, whose API was not reachable where brief 0046 ran) is
written here from each API's documented shapes (the pinned descriptions in protocol/forge/). Every file says so in
its `source`. The owner's recorder run with tokens replaces these with recorded ones; the tests stay the same.

    python3 tools/forge-corpus/synthesize.py
"""
import json
import os
import shutil
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "crates", "forge", "testdata")

SOURCES = {
    "github": "synthesized from the GitHub REST API v3 (2022-11-28) and GraphQL descriptions in protocol/forge/github",
    "gitlab": "synthesized from the GitLab REST API v4 description in protocol/forge/gitlab",
    "azure": "synthesized from the Azure DevOps REST API 7.1 specification in protocol/forge/azure-devops",
    "forgejo": "synthesized from the Forgejo API description (v1) in protocol/forge/forgejo",
    "tangled": "synthesized from the sh.tangled.* and com.atproto.* lexicons in protocol/forge/tangled",
}

_forge = None
_n = 0


def start(forge):
    global _forge, _n
    _forge, _n = forge, 0
    d = os.path.join(ROOT, forge, "synthesized")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)


def ex(name, method, path, status=200, body=None, query=None, contains=None, headers=None, times=None, text=None):
    global _n
    _n += 1
    e = {"source": SOURCES[_forge] + " (the owner's recorder run replaces it)",
         "request": {"method": method, "path": path}}
    if query:
        e["request"]["query"] = query
    if contains:
        e["request"]["body_contains"] = contains
    e["response"] = {"status": status}
    if headers:
        e["response"]["headers"] = headers
    if text is not None:
        e["response"]["body_text"] = text
    elif body is not None:
        e["response"]["body"] = body
    if times:
        e["times"] = times
    with open(os.path.join(ROOT, _forge, "synthesized", f"{_n:03}-{name}.json"), "w") as f:
        json.dump(e, f, indent=2, ensure_ascii=False)
        f.write("\n")


H = "1" * 40
B = "b" * 40
M = "9" * 40
H13 = "c" * 40

LOG = "\n".join(
    ["2026-10-04T05:00:00.0000000Z ##[group]Run cargo test"]
    + [f"2026-10-04T05:00:{i // 10:02}.{i % 10}000000Z test case_{i} ... ok" for i in range(40)]
    + ["2026-10-04T05:00:05.0000000Z test login::rejects_empty_password ... FAILED",
       "2026-10-04T05:00:05.1000000Z ##[error]Process completed with exit code 101."]
) + "\n"


# ----------------------------------------------------------------------------------------------------------- GitHub
def github():
    start("github")
    R = "/repos/octo-org/hello-world"
    web = "https://github.com/octo-org/hello-world"

    def user(login):
        return {"login": login, "id": 1, "html_url": f"https://github.com/{login}", "type": "User"}

    repo = {"full_name": "octo-org/hello-world", "clone_url": f"{web}.git", "node_id": "R_kgDOtest"}

    def pr(n, title, head, sha, state="open", draft=False, merged=False, mergeable=True, mstate="clean",
           labels=(), reviewers=(), body="", comments=0):
        return {
            "number": n, "node_id": f"PR_kwDO{n}", "title": title, "state": state, "draft": draft,
            "merged": merged, "merged_at": "2026-10-03T12:00:00Z" if merged else None,
            "user": user("octocat" if n != 15 else "hubot"),
            "head": {"ref": head, "sha": sha, "repo": repo}, "base": {"ref": "main", "sha": B, "repo": repo},
            "body": body, "html_url": f"{web}/pull/{n}", "labels": [{"name": l} for l in labels],
            "comments": comments, "requested_reviewers": [user(r) for r in reviewers],
            "mergeable": mergeable, "mergeable_state": mstate, "auto_merge": None,
            "created_at": "2026-10-01T09:00:00Z", "updated_at": f"2026-10-0{min(n % 9 + 1, 9)}T10:00:00Z",
        }

    p12 = pr(12, "Add the login form", "feature/login", H, labels=["enhancement"], reviewers=["hubot"],
             body="Adds the login form.\n\n- posts to `/session`\n- **validates** the password", comments=1,
             mstate="unstable")
    p13 = pr(13, "Fix the typo in the README", "fix/typo", H13)
    p15 = pr(15, "Draft: rework the session store", "feature/session", "d" * 40, draft=True, reviewers=["octocat"],
             mstate="draft")
    ex("user", "GET", "/user", body={"login": "octocat", "name": "The Octocat", "html_url": "https://github.com/octocat"})
    ex("repo", "GET", R, body={**repo, "allow_merge_commit": True, "allow_squash_merge": True, "allow_rebase_merge": False,
                               "default_branch": "main"})
    ex("pulls-page-1", "GET", f"{R}/pulls", query={"state": "open"},
       headers={"ETag": "\"pulls-open-v1\"", "Link": f"<{{{{base}}}}{R}/pulls?state=open&page=2>; rel=\"next\""},
       body=[p12, p13])
    ex("pulls-page-2", "GET", f"{R}/pulls", query={"state": "open", "page": "2"}, body=[p15])
    ex("pulls-closed", "GET", f"{R}/pulls", query={"state": "closed"},
       body=[pr(9, "Bump the toolchain", "chore/toolchain", "e" * 40, state="closed", merged=True)])
    for p in (p12, p13, p15):
        ex(f"pull-{p['number']}", "GET", f"{R}/pulls/{p['number']}", body=p, headers={"ETag": f"\"pull-{p['number']}-v1\""})
    ex("commits-12", "GET", f"{R}/pulls/12/commits", body=[
        {"sha": "2" * 40, "commit": {"message": "Add the login form\n\nIt posts to /session.",
                                      "author": {"name": "Octo Cat", "date": "2026-10-01T09:00:00Z"}}},
        {"sha": H, "commit": {"message": "Validate the password", "author": {"name": "Octo Cat", "date": "2026-10-02T09:00:00Z"}}},
    ])
    ex("files-12", "GET", f"{R}/pulls/12/files", body=[
        {"filename": "src/login.rs", "status": "added", "additions": 40, "deletions": 0, "changes": 40, "patch": "@@ -0,0 +1,40 @@"},
        {"filename": "src/main.rs", "status": "modified", "additions": 3, "deletions": 1, "changes": 4, "patch": "@@ -1 +1,3 @@"},
        {"filename": "docs/login.md", "previous_filename": "docs/auth.md", "status": "renamed", "additions": 0, "deletions": 0, "changes": 0},
    ])
    rc = lambda i, path, line, body, who="hubot", reply=None, side="RIGHT": {
        "id": i, "path": path, "line": line, "original_line": line, "side": side, "body": body, "user": user(who),
        "created_at": "2026-10-02T11:00:00Z", "html_url": f"{web}/pull/12#discussion_r{i}",
        **({"in_reply_to_id": reply} if reply else {}),
    }
    ex("review-comments-12", "GET", f"{R}/pulls/12/comments", body=[
        rc(100, "src/login.rs", 14, "This accepts an empty password; reject it here."),
        rc(101, "src/login.rs", 14, "Good catch, will fix.", who="octocat", reply=100),
        rc(102, "src/main.rs", 3, "Nit: keep the imports sorted."),
    ])
    ex("graphql-threads", "POST", "/graphql", contains=["reviewThreads"], body={"data": {"repository": {"pullRequest": {
        "reviewThreads": {"nodes": [
            {"id": "PRRT_100", "isResolved": False, "isOutdated": False, "comments": {"nodes": [{"databaseId": 100}]}},
            {"id": "PRRT_102", "isResolved": True, "isOutdated": False, "comments": {"nodes": [{"databaseId": 102}]}},
        ]}}}}})
    ex("issue-comments-12", "GET", f"{R}/issues/12/comments", body=[
        {"id": 2001, "user": user("hubot"), "body": "Thanks! See the [contributing guide](https://example.invalid).",
         "created_at": "2026-10-01T10:00:00Z", "html_url": f"{web}/pull/12#issuecomment-2001"}])
    ex("reviews-12", "GET", f"{R}/pulls/12/reviews", body=[
        {"id": 900, "user": user("hubot"), "state": "CHANGES_REQUESTED", "body": "Two things to fix.", "submitted_at": "2026-10-02T11:05:00Z"}])
    ex("check-runs-12", "GET", f"{R}/commits/{H}/check-runs", body={"total_count": 2, "check_runs": [
        {"id": 5001, "name": "build", "status": "completed", "conclusion": "success", "app": {"slug": "github-actions"},
         "html_url": f"{web}/actions/runs/1/job/5001", "started_at": "2026-10-02T09:01:00Z", "completed_at": "2026-10-02T09:04:30Z"},
        {"id": 5002, "name": "test", "status": "completed", "conclusion": "failure", "app": {"slug": "github-actions"},
         "html_url": f"{web}/actions/runs/1/job/5002", "started_at": "2026-10-02T09:01:00Z", "completed_at": "2026-10-02T09:06:00Z"},
    ]})
    ex("status-12", "GET", f"{R}/commits/{H}/status", body={"state": "success", "statuses": [
        {"id": 7001, "context": "ci/legacy", "state": "success", "target_url": "https://ci.example.invalid/7001", "created_at": "2026-10-02T09:02:00Z"}]})
    # Pull requests 13 and 15: nothing more than their list entries.
    for n, sha in ((13, H13), (15, "d" * 40)):
        for what in ("commits", "files", "comments", "reviews"):
            ex(f"{what}-{n}", "GET", f"{R}/pulls/{n}/{what}", body=[])
        ex(f"issue-comments-{n}", "GET", f"{R}/issues/{n}/comments", body=[])
        ex(f"check-runs-{n}", "GET", f"{R}/commits/{sha}/check-runs", body={"total_count": 1, "check_runs": [
            {"id": 5100 + n, "name": "build", "status": "completed", "conclusion": "success", "app": {"slug": "github-actions"},
             "html_url": f"{web}/actions/runs/2/job/{5100 + n}"}]})
        ex(f"status-{n}", "GET", f"{R}/commits/{sha}/status", body={"state": "pending", "statuses": []})
    # Writes.
    p14 = pr(14, "Add the login form", "feature/login", H, reviewers=["hubot"], labels=["enhancement"])
    ex("create-pull", "POST", f"{R}/pulls", status=201, contains=["\"head\":\"feature/login\""], body=p14)
    ex("request-reviewers-14", "POST", f"{R}/pulls/14/requested_reviewers", status=201, body=p14)
    ex("labels-14", "POST", f"{R}/issues/14/labels", body=[{"name": "enhancement"}])
    ex("update-12", "PATCH", f"{R}/pulls/12", contains=["\"title\""], body={**p12, "title": "Add the login form (v2)"})
    ex("close-12", "PATCH", f"{R}/pulls/12", contains=["\"state\":\"closed\""], body={**p12, "state": "closed"})
    ex("reopen-12", "PATCH", f"{R}/pulls/12", contains=["\"state\":\"open\""], body=p12)
    ex("comment-12", "POST", f"{R}/issues/12/comments", status=201, body={"id": 3001, "html_url": f"{web}/pull/12#issuecomment-3001"})
    ex("line-comment-12", "POST", f"{R}/pulls/12/comments", status=201, body={"id": 3002, "html_url": f"{web}/pull/12#discussion_r3002"})
    ex("reply-12", "POST", f"{R}/pulls/12/comments/100/replies", status=201, body={"id": 3003, "html_url": f"{web}/pull/12#discussion_r3003"})
    ex("review-approve-12", "POST", f"{R}/pulls/12/reviews", contains=["\"event\":\"APPROVE\""],
       body={"id": 901, "user": user("octocat"), "state": "APPROVED", "submitted_at": "2026-10-04T08:00:00Z"})
    ex("review-comment-12", "POST", f"{R}/pulls/12/reviews", contains=["\"event\":\"COMMENT\""],
       body={"id": 902, "user": user("octocat"), "state": "COMMENTED", "submitted_at": "2026-10-04T08:00:00Z"})
    ex("rerequest-12", "POST", f"{R}/pulls/12/requested_reviewers", status=201, body={**p12, "requested_reviewers": [user("hubot")]})
    ex("merge-13", "PUT", f"{R}/pulls/13/merge", body={"merged": True, "sha": M, "message": "Pull Request successfully merged"})
    ex("graphql-ready", "POST", "/graphql", contains=["markPullRequestReadyForReview"],
       body={"data": {"markPullRequestReadyForReview": {"pullRequest": {"isDraft": False}}}})
    ex("graphql-draft", "POST", "/graphql", contains=["convertPullRequestToDraft"],
       body={"data": {"convertPullRequestToDraft": {"pullRequest": {"isDraft": True}}}})
    ex("graphql-resolve", "POST", "/graphql", contains=["{resolveReviewThread("],
       body={"data": {"resolveReviewThread": {"thread": {"isResolved": True}}}})
    ex("graphql-unresolve", "POST", "/graphql", contains=["{unresolveReviewThread("],
       body={"data": {"unresolveReviewThread": {"thread": {"isResolved": False}}}})
    ex("graphql-auto-merge", "POST", "/graphql", contains=["enablePullRequestAutoMerge"],
       body={"data": {"enablePullRequestAutoMerge": {"pullRequest": {"number": 13}}}})
    ex("graphql-linked-branch", "POST", "/graphql", contains=["createLinkedBranch"],
       body={"data": {"createLinkedBranch": {"linkedBranch": {"id": "LB_1"}}}})
    # Issues.
    issue = lambda n, title, state="open", labels=(), assignees=(): {
        "number": n, "node_id": f"I_kwDO{n}", "title": title, "state": state, "user": user("octocat"),
        "assignees": [user(a) for a in assignees], "labels": [{"name": l} for l in labels],
        "milestone": {"number": 1, "title": "v1.0"} if n == 42 else None, "comments": 1 if n == 42 else 0,
        "html_url": f"{web}/issues/{n}", "body": "The form submits with an empty password." if n == 42 else "",
        "created_at": "2026-09-30T09:00:00Z", "updated_at": "2026-10-02T09:00:00Z",
    }
    ex("issues", "GET", f"{R}/issues", query={"state": "open"}, body=[
        issue(42, "Login accepts an empty password", labels=["bug"], assignees=["octocat"]),
        {**issue(12, "Add the login form"), "pull_request": {"url": "x"}},
        issue(41, "Document the session store", labels=["docs"]),
    ])
    ex("issue-42", "GET", f"{R}/issues/42", body=issue(42, "Login accepts an empty password", labels=["bug"], assignees=["octocat"]))
    ex("issue-comments-42", "GET", f"{R}/issues/42/comments", body=[
        {"id": 4001, "user": user("hubot"), "body": "Reproduced on `main`.", "created_at": "2026-10-01T09:00:00Z"}])
    ex("create-issue", "POST", f"{R}/issues", status=201, body=issue(43, "Rate-limit the login endpoint"))
    ex("comment-42", "POST", f"{R}/issues/42/comments", status=201, body={"id": 4002, "html_url": f"{web}/issues/42#issuecomment-4002"})
    ex("update-42", "PATCH", f"{R}/issues/42", body=issue(42, "Login accepts an empty password", state="closed", labels=["bug", "security"]))
    ex("milestones", "GET", f"{R}/milestones", body=[{"number": 1, "title": "v1.0"}])
    ex("job-log-5002", "GET", f"{R}/actions/jobs/5002/logs", text=LOG)
    ex("rerun-5002", "POST", f"{R}/actions/jobs/5002/rerun", status=201, body={})
    # Sign-in: the device flow (pending once, then the token) and the token check.
    ex("device-code", "POST", "/login/device/code", body={"device_code": "dc-test", "user_code": "WDJB-MJHT",
       "verification_uri": "https://github.com/login/device", "expires_in": 900, "interval": 5})
    ex("device-pending", "POST", "/login/oauth/access_token", times=1, body={"error": "authorization_pending"})
    ex("device-token", "POST", "/login/oauth/access_token", body={"access_token": "gho_devicetoken", "token_type": "bearer", "scope": "repo"})


# ----------------------------------------------------------------------------------------------------------- Forgejo
def forgejo():
    start("forgejo")
    R = "/api/v1/repos/forgejo/forgejo"
    web = "https://codeberg.org/forgejo/forgejo"
    u = lambda l: {"id": 1, "login": l, "username": l, "html_url": f"https://codeberg.org/{l}"}
    repo = {"full_name": "forgejo/forgejo", "clone_url": f"{web}.git"}
    p77 = {"number": 77, "title": "fix: reject an empty password", "state": "open", "draft": False, "merged": False,
           "mergeable": True, "user": u("octocat"), "head": {"ref": "feature/login", "sha": H, "repo": repo},
           "base": {"ref": "forgejo", "sha": B, "repo": repo}, "html_url": f"{web}/pulls/77", "labels": [],
           "requested_reviewers": [], "comments": 0, "body": "Fixes #14684.", "created_at": "2026-10-04T07:00:00+02:00",
           "updated_at": "2026-10-04T07:00:00+02:00", "merge_base": B}
    ex("user", "GET", "/api/v1/user", body={"id": 7, "login": "octocat", "full_name": "", "html_url": "https://codeberg.org/octocat"})
    ex("pull-77", "GET", f"{R}/pulls/77", body=p77)
    for what in ("commits", "files", "reviews"):
        ex(f"{what}-77", "GET", f"{R}/pulls/77/{what}", body=[])
    ex("comments-77", "GET", f"{R}/issues/77/comments", body=[])
    ex("statuses-77", "GET", f"{R}/commits/{H}/statuses", body=[
        {"id": 9001, "status": "success", "context": "ci / build (pull_request)", "target_url": "/forgejo/forgejo/actions/runs/9/jobs/0", "created_at": "2026-10-04T07:01:00+02:00"}])
    ex("create-pull", "POST", f"{R}/pulls", status=201, contains=["\"head\":\"feature/login\""], body=p77)
    ex("update-77", "PATCH", f"{R}/pulls/77", contains=["\"title\""], body={**p77, "title": "WIP: fix: reject an empty password"})
    ex("close-77", "PATCH", f"{R}/pulls/77", contains=["\"state\":\"closed\""], body={**p77, "state": "closed"})
    ex("comment-77", "POST", f"{R}/issues/77/comments", status=201, body={"id": 5001, "user": u("octocat"), "body": "x", "html_url": f"{web}/pulls/77#issuecomment-5001"})
    ex("review-77", "POST", f"{R}/pulls/77/reviews", contains=["\"event\":\"APPROVED\""],
       body={"id": 6001, "user": u("octocat"), "state": "APPROVED", "submitted_at": "2026-10-04T08:00:00+02:00"})
    ex("review-comment-77", "POST", f"{R}/pulls/77/reviews", contains=["\"event\":\"COMMENT\""],
       body={"id": 6002, "user": u("octocat"), "state": "COMMENT", "submitted_at": "2026-10-04T08:00:00+02:00"})
    ex("reviewers-77", "POST", f"{R}/pulls/77/requested_reviewers", status=201, body=[])
    ex("merge-77", "POST", f"{R}/pulls/77/merge", body=None)
    ex("issue-create", "POST", f"{R}/issues", status=201, body={"number": 14700, "title": "Rate-limit the login endpoint", "state": "open",
       "user": u("octocat"), "html_url": f"{web}/issues/14700", "labels": [], "assignees": []})
    ex("issue-comment", "POST", f"{R}/issues/14684/comments", status=201, body={"id": 5002, "user": u("octocat"), "body": "x"})
    ex("issue-update", "PATCH", f"{R}/issues/14684", body={"number": 14684, "title": "Issue", "state": "closed", "user": u("octocat"),
       "html_url": f"{web}/issues/14684", "labels": [{"name": "bug"}], "assignees": []})
    ex("labels", "GET", f"{R}/labels", body=[{"id": 11, "name": "bug"}, {"id": 12, "name": "enhancement"}])
    ex("issue-labels", "PUT", f"{R}/issues/14684/labels", body=[{"id": 11, "name": "bug"}])


# ------------------------------------------------------------------------------------------------------------ GitLab
def gitlab():
    start("gitlab")
    P = "/api/v4/projects/gitlab-org/cli"
    web = "https://gitlab.com/gitlab-org/cli"
    u = lambda l: {"id": 1, "username": l, "name": l.title(), "web_url": f"https://gitlab.com/{l}"}
    ex("user", "GET", "/api/v4/user", body={"id": 99, "username": "octocat", "name": "Octo Cat", "web_url": "https://gitlab.com/octocat"})
    ex("discussions-3992", "GET", f"{P}/merge_requests/3992/discussions", body=[
        {"id": "d1a2b3", "individual_note": False, "notes": [
            {"id": 701, "body": "This overrides the host the user chose.", "author": u("reviewer"), "created_at": "2026-10-02T09:00:00Z",
             "system": False, "resolvable": True, "resolved": False,
             "position": {"position_type": "text", "new_path": "internal/commands/auth/login/login.go", "old_path": "internal/commands/auth/login/login.go",
                          "new_line": 118, "old_line": None, "head_sha": "d900a435b60dee9d1532bc9d4391968637567334"}},
            {"id": 702, "body": "Fixed in the next push.", "author": u("gkepas"), "created_at": "2026-10-02T10:00:00Z",
             "system": False, "resolvable": True, "resolved": False}]},
        {"id": "e4f5a6", "individual_note": False, "notes": [
            {"id": 703, "body": "Nit: wording.", "author": u("reviewer"), "created_at": "2026-10-02T09:05:00Z", "system": False,
             "resolvable": True, "resolved": True,
             "position": {"position_type": "text", "new_path": "docs/source/auth/login.md", "old_path": "docs/source/auth/login.md",
                          "new_line": 45, "old_line": None, "head_sha": "d900a435b60dee9d1532bc9d4391968637567334"}}]},
        {"id": "f7a8b9", "individual_note": True, "notes": [
            {"id": 704, "body": "Thanks for the contribution!", "author": u("maintainer"), "created_at": "2026-10-01T09:00:00Z",
             "system": False, "resolvable": False}]},
        {"id": "s1", "individual_note": True, "notes": [{"id": 705, "body": "added 1 commit", "author": u("gkepas"), "system": True}]},
    ])
    ex("notes-8577", "GET", f"{P}/issues/8577/notes", body=[
        {"id": 801, "body": "Reproduced with glab 1.50.", "author": u("maintainer"), "created_at": "2026-10-01T09:00:00Z", "system": False},
        {"id": 802, "body": "changed the description", "author": u("maintainer"), "system": True}])
    ex("trace", "GET", "/api/v4/projects/45049979/jobs/16914440454/trace", text=LOG)
    ex("retry", "POST", "/api/v4/projects/45049979/jobs/16914440454/retry", status=201, body={"id": 16914449999, "status": "pending"})
    mr = lambda iid, title, state="opened", draft=False: {
        "iid": iid, "title": title, "state": state, "draft": draft, "author": u("octocat"), "source_branch": "feature/login",
        "target_branch": "main", "source_project_id": 34675721, "target_project_id": 34675721, "web_url": f"{web}/-/merge_requests/{iid}",
        "labels": [], "reviewers": [], "user_notes_count": 0, "created_at": "2026-10-04T09:00:00Z", "updated_at": "2026-10-04T09:00:00Z",
        "detailed_merge_status": "mergeable", "sha": H, "diff_refs": {"base_sha": B, "head_sha": H, "start_sha": B}}
    ex("create-mr", "POST", f"{P}/merge_requests", status=201, contains=["\"source_branch\":\"feature/login\""], body=mr(4000, "Add the login form"))
    ex("update-mr", "PUT", f"{P}/merge_requests/3992", contains=["\"title\""], body={**mr(3992, "Draft: feat(auth): set default host"), "draft": True})
    ex("close-mr", "PUT", f"{P}/merge_requests/3992", contains=["\"state_event\":\"close\""], body=mr(3992, "feat(auth)", state="closed"))
    ex("reviewers-mr", "PUT", f"{P}/merge_requests/3992", contains=["\"reviewer_ids\""], body={**mr(3992, "feat(auth)"), "reviewers": [u("reviewer")]})
    ex("users-reviewer", "GET", "/api/v4/users", query={"username": "reviewer"}, body=[{"id": 55, "username": "reviewer"}])
    ex("note", "POST", f"{P}/merge_requests/3992/notes", status=201, body={"id": 901, "body": "x"})
    ex("discussion", "POST", f"{P}/merge_requests/3992/discussions", status=201, body={"id": "n3w", "notes": [{"id": 902}]})
    ex("reply", "POST", f"{P}/merge_requests/3992/discussions/d1a2b3/notes", status=201, body={"id": 903})
    ex("resolve", "PUT", f"{P}/merge_requests/3992/discussions/d1a2b3", body={"id": "d1a2b3", "notes": [{"id": 701, "resolvable": True, "resolved": True}]})
    ex("draft-note", "POST", f"{P}/merge_requests/3992/draft_notes", status=201, body={"id": 1001, "note": "x"})
    ex("draft-notes", "GET", f"{P}/merge_requests/3992/draft_notes", body=[{"id": 1001}])
    ex("draft-delete", "DELETE", f"{P}/merge_requests/3992/draft_notes/1001", status=204)
    ex("bulk-publish", "POST", f"{P}/merge_requests/3992/draft_notes/bulk_publish", status=204)
    ex("approve", "POST", f"{P}/merge_requests/3992/approve", status=201, body={"approved": True})
    ex("merge", "PUT", f"{P}/merge_requests/3992/merge", contains=["\"merge_when_pipeline_succeeds\":true"],
       body={**mr(3992, "feat(auth)"), "merge_when_pipeline_succeeds": True})
    ex("issue-create", "POST", f"{P}/issues", status=201, body={"iid": 8600, "title": "Rate-limit the login endpoint", "state": "opened",
       "author": u("octocat"), "web_url": f"{web}/-/issues/8600", "labels": [], "assignees": []})
    ex("issue-note", "POST", f"{P}/issues/8577/notes", status=201, body={"id": 950, "body": "x"})
    ex("issue-update", "PUT", f"{P}/issues/8577", body={"iid": 8577, "title": "Issue", "state": "closed", "author": u("octocat"),
       "web_url": f"{web}/-/issues/8577", "labels": ["bug"], "assignees": []})
    # The device flow.
    ex("device", "POST", "/oauth/authorize_device", body={"device_code": "dc", "user_code": "GLAB-1234",
       "verification_uri": "https://gitlab.com/oauth/device", "expires_in": 300, "interval": 5})
    ex("device-token", "POST", "/oauth/token", body={"access_token": "gloas-test", "refresh_token": "r", "token_type": "Bearer"})


# ------------------------------------------------------------------------------------------------------ Azure DevOps
def azure():
    start("azure")
    O = "/dnceng-public"
    G = f"{O}/public/_apis/git/repositories/dotnet-public-wiki"
    project = "cbb18261-c48f-4abb-8651-8cdcb5474649"
    me = {"id": "6c8f3c5e-0000-4000-8000-000000000001", "displayName": "Octo Cat", "uniqueName": "octocat@example.invalid"}
    rev = lambda name, vote, rid: {"id": rid, "displayName": name.title(), "uniqueName": f"{name}@example.invalid", "vote": vote, "isRequired": vote == 0}
    pr17 = {"pullRequestId": 17, "codeReviewId": 17, "status": "active", "isDraft": False, "title": "Add the login form",
            "description": "Adds the login form.", "createdBy": me, "creationDate": "2026-10-03T09:00:00Z",
            "sourceRefName": "refs/heads/feature/login", "targetRefName": "refs/heads/main", "mergeStatus": "succeeded",
            "lastMergeSourceCommit": {"commitId": H}, "lastMergeTargetCommit": {"commitId": B},
            "reviewers": [rev("hubot", 10, "r-1"), rev("reviewer", -5, "r-2"), rev("lead", 0, "r-3")],
            "labels": [{"name": "feature"}], "repository": {"id": "2459d599-fdb2-4d28-9810-daeec061cf90", "project": {"id": project}}}
    ex("connection", "GET", f"{O}/_apis/connectionData", body={"authenticatedUser": {"id": me["id"], "providerDisplayName": "Octo Cat",
       "descriptor": "Microsoft.IdentityModel.Claims.ClaimsIdentity;x", "properties": {"Account": {"$type": "System.String", "$value": me["uniqueName"]}}}})
    ex("pulls-active", "GET", f"{G}/pullrequests", query={"searchCriteria.status": "active"}, body={"value": [pr17], "count": 1})
    ex("pull-17", "GET", f"{G}/pullrequests/17", body=pr17)
    ex("commits-17", "GET", f"{G}/pullrequests/17/commits", body={"value": [
        {"commitId": H, "comment": "Add the login form\n\nIt posts to /session.", "author": {"name": "Octo Cat", "date": "2026-10-03T09:00:00Z"}}]})
    ex("iterations-17", "GET", f"{G}/pullrequests/17/iterations", body={"count": 2, "value": [{"id": 1}, {"id": 2}]})
    ex("changes-17", "GET", f"{G}/pullrequests/17/iterations/2/changes", body={"changeEntries": [
        {"item": {"path": "/src/Login.cs", "gitObjectType": "blob"}, "changeType": "add"},
        {"item": {"path": "/src/Program.cs", "gitObjectType": "blob"}, "changeType": "edit"},
        {"item": {"path": "/src", "gitObjectType": "tree"}, "changeType": "edit"}]})
    ex("threads-17", "GET", f"{G}/pullrequests/17/threads", body={"value": [
        {"id": 31, "status": "active", "threadContext": {"filePath": "/src/Login.cs", "rightFileStart": {"line": 12, "offset": 1}},
         "comments": [{"id": 1, "author": rev("reviewer", 0, "r-2"), "content": "Check for an empty password here.", "commentType": "text",
                       "publishedDate": "2026-10-03T10:00:00Z"}]},
        {"id": 32, "status": "fixed", "threadContext": {"filePath": "/src/Program.cs", "rightFileStart": {"line": 3, "offset": 1}},
         "comments": [{"id": 1, "author": rev("hubot", 0, "r-1"), "content": "Sort the usings.", "commentType": "text"}]},
        {"id": 33, "comments": [{"id": 1, "author": me, "content": "Ready for review.", "commentType": "text", "publishedDate": "2026-10-03T09:30:00Z"}]},
        {"id": 34, "comments": [{"id": 1, "author": me, "content": "Octo Cat voted 10", "commentType": "system"}]}]})
    ex("statuses", "GET", f"{G}/commits/{H}/statuses", body={"value": [
        {"id": 1, "state": "succeeded", "context": {"genre": "continuous-integration", "name": "build"}, "targetUrl": "https://dev.azure.com/x/build/1"}]})
    ex("policies-17", "GET", f"{O}/public/_apis/policy/evaluations", body={"value": [
        {"evaluationId": "ev-build", "status": "rejected", "configuration": {"isBlocking": True, "type": {"displayName": "Build"},
         "settings": {"displayName": "PR build"}}, "context": {"buildId": 4242}},
        {"evaluationId": "ev-reviewers", "status": "approved", "configuration": {"isBlocking": True, "type": {"displayName": "Minimum number of reviewers"}, "settings": {}}},
        {"evaluationId": "ev-workitems", "status": "running", "configuration": {"isBlocking": False, "type": {"displayName": "Work item linking"}, "settings": {}}}]})
    ex("build-4242", "GET", f"{O}/public/_apis/build/builds/4242", body={"id": 4242, "status": "completed", "result": "failed",
       "definition": {"name": "dotnet-public-wiki-ci"}, "startTime": "2026-10-03T09:01:00Z", "finishTime": "2026-10-03T09:09:00Z",
       "_links": {"web": {"href": "https://dev.azure.com/dnceng-public/public/_build/results?buildId=4242"}}})
    ex("build-logs", "GET", f"{O}/public/_apis/build/builds/4242/logs", body={"count": 1, "value": [{"id": 7}]})
    ex("build-log-7", "GET", f"{O}/public/_apis/build/builds/4242/logs/7", text=LOG)
    ex("build-retry", "PATCH", f"{O}/public/_apis/build/builds/4242", body={"id": 4242, "status": "notStarted"})
    ex("policy-requeue", "PATCH", f"{O}/public/_apis/policy/evaluations/ev-build", body={"evaluationId": "ev-build", "status": "queued"})
    # Writes.
    ex("create-pr", "POST", f"{G}/pullrequests", status=201, contains=["\"sourceRefName\":\"refs/heads/feature/login\""], body={**pr17, "pullRequestId": 18})
    ex("vote", "PUT", f"{G}/pullrequests/17/reviewers/{me['id']}", body={"id": me["id"], "vote": 10})
    ex("thread", "POST", f"{G}/pullrequests/17/threads", status=201, body={"id": 40, "comments": [{"id": 1}]})
    ex("reply", "POST", f"{G}/pullrequests/17/threads/31/comments", status=201, body={"id": 2})
    ex("resolve", "PATCH", f"{G}/pullrequests/17/threads/31", body={"id": 31, "status": "fixed"})
    ex("autocomplete", "PATCH", f"{G}/pullrequests/17", contains=["\"autoCompleteSetBy\""], body={**pr17, "autoCompleteSetBy": me})
    ex("complete", "PATCH", f"{G}/pullrequests/17", contains=["\"status\":\"completed\""], body={**pr17, "status": "completed", "lastMergeCommit": {"commitId": M}})
    ex("abandon", "PATCH", f"{G}/pullrequests/17", contains=["\"status\":\"abandoned\""], body={**pr17, "status": "abandoned"})
    ex("draft", "PATCH", f"{G}/pullrequests/17", contains=["\"isDraft\""], body={**pr17, "isDraft": True})
    # Work items.
    wi = lambda i, title, kind, state: {"id": i, "fields": {"System.Title": title, "System.WorkItemType": kind, "System.State": state,
        "System.CreatedBy": me, "System.AssignedTo": me if i == 101 else None, "System.Tags": "auth; security" if i == 101 else "",
        "System.IterationPath": "public\\Sprint 12", "System.CreatedDate": "2026-09-30T09:00:00Z", "System.ChangedDate": "2026-10-02T09:00:00Z",
        "System.CommentCount": 1 if i == 101 else 0, "System.Description": "<div>The form submits with an <b>empty</b> password.</div>",
        "Microsoft.VSTS.Common.Priority": 2, "Microsoft.VSTS.TCM.ReproSteps": "<ol><li>Open</li><li>Submit</li></ol>"},
        "relations": []}
    ex("wiql", "POST", f"{O}/public/_apis/wit/wiql", body={"workItems": [{"id": 101}, {"id": 102}]})
    ex("workitems", "GET", f"{O}/public/_apis/wit/workitems", body={"value": [wi(101, "Login accepts an empty password", "Bug", "Active"),
                                                                                wi(102, "Document the session store", "Task", "New")]})
    ex("workitem-101", "GET", f"{O}/public/_apis/wit/workitems/101", body=wi(101, "Login accepts an empty password", "Bug", "Active"))
    ex("comments-101", "GET", f"{O}/public/_apis/wit/workItems/101/comments", body={"comments": [
        {"id": 1, "text": "<div>Reproduced on <i>main</i>.</div>", "createdBy": me, "createdDate": "2026-10-01T09:00:00Z"}]})
    ex("types", "GET", f"{O}/public/_apis/wit/workitemtypes", body={"value": [{"name": "Bug"}, {"name": "Task"}, {"name": "User Story"}]})
    ex("states-bug", "GET", f"{O}/public/_apis/wit/workitemtypes/Bug/states", body={"value": [
        {"name": "New", "category": "Proposed"}, {"name": "Active", "category": "InProgress"}, {"name": "Closed", "category": "Completed"}]})
    ex("create-bug", "POST", f"{O}/public/_apis/wit/workitems/$Bug", body=wi(103, "Rate-limit the login endpoint", "Bug", "New"))
    ex("update-101", "PATCH", f"{O}/public/_apis/wit/workitems/101", body=wi(101, "Login accepts an empty password", "Bug", "Closed"))
    ex("comment-101", "POST", f"{O}/public/_apis/wit/workItems/101/comments", body={"id": 2, "text": "x"})
    ex("repo", "GET", G, body={"id": "2459d599-fdb2-4d28-9810-daeec061cf90", "project": {"id": project}})
    # The Microsoft identity platform's device code flow (`<api>/login` against the fixture server).
    ex("device", "POST", f"{O}/login/oauth2/v2.0/devicecode", body={"device_code": "dc", "user_code": "AZDO-5678",
       "verification_uri": "https://microsoft.com/devicelogin", "expires_in": 900, "interval": 5})
    ex("device-token", "POST", f"{O}/login/oauth2/v2.0/token", body={"access_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJub25lIn0.e30.", "refresh_token": "r"})


# ----------------------------------------------------------------------------------------------------------- Tangled
def tangled():
    start("tangled")
    pds = "/pds"
    ex("resolve-me", "GET", "/xrpc/com.atproto.identity.resolveHandle", query={"handle": "octocat.example"}, body={"did": "did:plc:testuser"})
    ex("plc-me", "GET", "/plc/did:plc:testuser", body={"id": "did:plc:testuser", "alsoKnownAs": ["at://octocat.example"],
       "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer", "serviceEndpoint": "{{base}}/pds"}]})
    ex("session", "POST", f"{pds}/xrpc/com.atproto.server.createSession", contains=["\"password\":\"app-pass\""],
       body={"did": "did:plc:testuser", "handle": "octocat.example", "accessJwt": "access-jwt-test", "refreshJwt": "refresh"})
    ex("session-refused", "POST", f"{pds}/xrpc/com.atproto.server.createSession", status=401,
       body={"error": "AuthenticationRequired", "message": "Invalid identifier or password"})
    ex("create-record", "POST", f"{pds}/xrpc/com.atproto.repo.createRecord",
       body={"uri": "at://did:plc:testuser/sh.tangled.feed.comment/3newcomment", "cid": "bafytest"})
    ex("create-issue", "POST", f"{pds}/xrpc/com.atproto.repo.createRecord", contains=["\"collection\":\"sh.tangled.repo.issue\""],
       body={"uri": "at://did:plc:testuser/sh.tangled.repo.issue/3newissue", "cid": "bafyissue"})


def main():
    for f in (github, forgejo, gitlab, azure, tangled):
        f()
    print("written under", os.path.normpath(ROOT))


if __name__ == "__main__":
    sys.exit(main())
