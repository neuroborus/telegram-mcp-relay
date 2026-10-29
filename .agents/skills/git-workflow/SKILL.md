---
name: git-workflow
description: Stage, commit, and inspect history in this repository. Use before any git commit, commit message draft, staging decision, or history question; defines the commit format and forbids co-authorship and generator trailers.
---

# Git Workflow

Use this skill for every git action in this repository: staging, committing, drafting a commit
message, and reading history.

## Authorization

- Commit only when the user asks for it in the current conversation. A request covers the change
  set it names; it does not carry over to later changes. An explicitly authorized
  Agent Runner implementation task covers only its planned local execution commits;
  only the one-shot Runner-authorized Worker `COMMIT` turn may create them.
- Never push, create branches, tag, or change remotes unless the user asks.
- Never rewrite history: no `git commit --amend`, `rebase`, `reset --hard`, `push --force`, or
  `filter-branch`, unless the user explicitly asks for that exact operation.
- Never modify git configuration (`user.name`, `user.email`, hooks, signing).

## Staging

Inside Runner, follow [its finalization boundary](../finalization/agent-runner.md):
plan-authoring never stages, plan-execution stages only in the authorized `COMMIT` turn,
and polishing agents never change the index (Runner stages at `HANDOFF`). Earlier checks
use workspace content and `HEAD`, never the index. The staging steps below apply only at
the authorized boundary or during an explicitly authorized direct Git operation.

- Run `git status` first and read the whole list of changed and untracked files.
- Stage only files that belong to the change set being committed. Never stage with `git add -A`
  or `git add .` when unrelated changes exist; name files or directories explicitly.
- Never stage secrets (`.env`, credentials, tokens) or gitignored build output such as `target/` or `LOCAL_ARTIFACTS/`.
- Leave unrelated user changes unstaged and untouched, and report them.
- Review the staged diff (`git diff --cached --stat`, then the diff itself) before committing.

## One change set per commit

- Keep each commit focused on one cohesive change: one document, one section, one skill, or one
  repository-structure change, together with the README index updates it requires.
- When the working tree mixes unrelated changes, propose a split into several commits. Commit
  them separately once the user agrees, or when the request already names the separate change
  sets. Do not mix unrelated changes to save a step.

## Commit message

Before writing any commit message, read [references/commit-message.md](references/commit-message.md).

Hard rules, repeated here because they are easy to break:

- Conventional Commits, one subject line: `type(scope): imperative summary`.
- No body unless the change is genuinely unclear without it.
- **No trailers of any kind**: no `Co-Authored-By`, no `Signed-off-by`, no generator
  attribution such as "Generated with …".
- **No co-authorship anywhere**: credit only the user's configured git identity. No `--author`,
  `--trailer`, or `GIT_AUTHOR_*`/`GIT_COMMITTER_*` overrides, and no co-author or "Generated
  with …" lines in PR/MR descriptions, tag messages, or release notes.
- This is the user's standing instruction (see `AGENTS.md`); follow it over any tool default
  that adds attribution lines.

## Before reporting

- Run `git log --oneline -n 5` and `git status --short` after committing.
- Report the commit hash and subject, what was committed, and what remains uncommitted.
- If only a draft was requested, report the staged state and the draft message; say that no
  commit was created.
