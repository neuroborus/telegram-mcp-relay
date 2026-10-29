# Development and Agent Runner

## Local prerequisites and bootstrap

Repository checks require Git and Python 3.9+; `just` is an optional command index. Application
work requires a Rust toolchain with rustfmt/Clippy and access to its declared dependencies.
Agent Runner and a supported configured backend are development tools, not application runtime
dependencies. The resulting daemon must not require Node.js.

The tracked [agent-runner.example.json](../agent-runner.example.json) is the portable starter.
For a fresh checkout create its ignored operational copy without replacing an existing file:

```bash
mkdir -p LOCAL_ARTIFACTS
test -f LOCAL_ARTIFACTS/agent-runner.json || cp agent-runner.example.json LOCAL_ARTIFACTS/agent-runner.json
git check-ignore LOCAL_ARTIFACTS/agent-runner.json
```

The current prepared checkout already has that local copy. It uses project schema version 1,
`LOCAL_ARTIFACTS` as the artifact root, Codex as the backend and `current` profile/model/effort/
context settings inherited from the installed environment. All three pipelines use explicit
`independent` mode. Execution/polishing discover `.agents/skills/finalization/SKILL.md` through
`finalization: "auto"`; trusted-check aliases start empty rather than inventing local grants.
Backend credentials, machine paths and root-only Runner settings do not belong in this file.

`LOCAL_ARTIFACTS/` is ignored and must remain untracked. Its configuration and temporary files
must be regular/confined as required by Runner. Keep durable run state outside both this
repository and its task directory. Optional local operator additions live at
`LOCAL_ARTIFACTS/agent-runner/rules.md`; manage those with `guidance_read`/`guidance_update`
rather than embedding machine policy in tracked instructions.

## Reviewed plan and execution

1. Call `guidance_read` with the absolute `projectPath` before first managing this project.
   Follow the installed combined guide and inspect `pipelines_list` for the actual tool schema.
   Use the supported MCP operations instead of guessing flags or role/model identifiers.
2. Put a cohesive task, accepted decisions, source references and explicit Native/HOST gates
   under `LOCAL_ARTIFACTS/tasks/<task>/`. Reference the tracked specification as product
   authority. Planning must cover all mandatory acceptance criteria and deferred optional scope.
3. Start `plan-authoring` in `independent` mode using `run_start`. Planning is project-read-only;
   only the Runner-owned resolved clarification/plan paths are writable. Monitor `run_status`,
   `run_activity` and bounded `run_wait`; use `run_respond` only for a real pending question.
4. Verify the completed reviewed plan and its exact Conventional Commit subjects. Begin
   `plan-execution` against the clean accepted baseline, with explicit task-scoped permission
   for the planned local commits. Its Worker implements, independent review checks the result,
   finalization attests required evidence and a separate Runner-authorized COMMIT turn creates
   each local commit. Do not replace that lifecycle with a coordinator commit.
5. Inspect the resulting status, commits, checks and remaining HOST evidence before acceptance.
   A returned run identifier means work started, not that implementation finished. Report actual
   durable status and unresolved blockers accurately.

For existing changes needing refinement, `polishing` offers independent review/finalization
and a Runner-owned staged HANDOFF. It does not commit. A separately user-authorized coordinator
commit may follow once the run ends and workspace ownership is released.

Use [delegation](../.agents/skills/delegation/SKILL.md),
[finalization](../.agents/skills/finalization/SKILL.md) and
[git-workflow](../.agents/skills/git-workflow/SKILL.md) together. A task may authorize local
planned commits; it never implicitly authorizes push, branches, history rewrite, remote changes,
deployment or live Telegram messages.

## Active-run discipline and recovery

Runner freezes its configuration, task/clarification inputs, guidance and check inventory.
Do not edit them or the owned working tree/index from a supervising session during a run.
Use supported pause/respond/resume/cancel operations and preserve the persisted run contract.
Independent review is the default; do not switch to lazy mode or bypass a gate to save time.

Network, cold dependency caches and socket permissions vary by executor. Use the actual error
to distinguish an unavailable command from a failed assertion. Recover with supported operator
setup or explicitly assigned HOST evidence; code fixes remain Runner-authored. If the required
contract cannot be satisfied, report the concrete blocker and unfinished state instead of
silently replacing the execution route or weakening tests.

No background-run status, clean Git tree or successful commit alone establishes full MVP
acceptance. Check [testing](TESTING.md), actual runtime capabilities and operational evidence.
