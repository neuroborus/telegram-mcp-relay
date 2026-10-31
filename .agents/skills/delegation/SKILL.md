---
name: delegation
description: Prepare and operate Agent Runner planning, implementation and polishing for Telegram MCP Relay while preserving independent review and Git ownership.
---

# Delegation

Use Agent Runner for product implementation and its tests/tooling. Use a delegated executor
and distinct reviewer through [subagent-quality-loop](../subagent-quality-loop/SKILL.md) for
repository workflow/documentation preparation when requested. Do not recursively spawn pairs.
The coordinator owns scope, acceptance and observations; it does not write fallback product
patches outside Runner when a run blocks.

Before operating a project, call `guidance_read` with its `projectPath` and follow the installed
combined operator guide. Inspect `pipelines_list` rather than guessing current schemas or
backend flags. Read [development](../../../docs/DEVELOPMENT.md) for this repository's local
configuration and [Runner finalization](../finalization/agent-runner.md) for check ownership.

Serialize validation against the actual phase schema. Structured check IDs match
`^C[1-9][0-9]{0,8}$`; IDs and exact commands are globally unique within an inventory.
Share one row per command: scoped rows merge applicability into one nonempty, strictly
ascending `steps` list; unscoped rows omit it. H/R/P labels in prose are logical names,
not structured IDs. Persisted per-step receipt IDs may differ from global catalog IDs;
export the observed receipt IDs, commands and provenance. Retain conditional checks.

Use `plan-authoring` to write a reviewed commit plan, then `plan-execution` to implement it
one verified local commit at a time. Select `independent` explicitly. `polishing` reviews and
stages existing changes but never commits. Only a Runner-authorized Worker `COMMIT` turn may
create execution commits; do not stage or edit while another run owns the workspace.

After successful polishing reaches `DONE` and releases workspace ownership, use the user's
standing permission to commit the unchanged, finalized and independently reviewed in-scope
handoff locally without another approval question. Follow
[git-workflow](../git-workflow/SKILL.md) and the Runner finalization handoff conditions.

Keep ignored task artifacts confined under `LOCAL_ARTIFACTS/`, run state outside both checkout
and task directory, and active inputs/configuration/guidance frozen. Resolve questions from
repository evidence where possible. A genuine material product decision pauses dependent work;
elapsed time or a tool timeout is not approval.

Monitor durable run status/activity and actual progress. Inspect the resulting work and
receipts independently before accepting it. Recover concrete environment failures through
supported Runner operations without bypassing checks, changing frozen configuration or
claiming unexecuted tests passed. Report the exact unresolved blocker if recovery is impossible.
