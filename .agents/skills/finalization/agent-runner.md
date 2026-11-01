# Agent Runner Finalization

The installed Runner guidance and accepted task gates define the active contract. This file
preserves this repository's pipeline boundaries; it is a supporting reference, not a valid
`finalization` setting. Use `auto` or `.agents/skills/finalization/SKILL.md` as the skill setting.

Default to independent review unless the user explicitly selects another supported mode.
Honor `lazy` with installed convergence and fresh read-only confirmation; do not describe that
as independent code review or add an independent review per step. All applicable checks and
finalization remain mandatory in either mode.

## Checks and evidence

Keep bootstrap, candidate and finalization inventories independent of index contents. Run
`python3 tools/check_repository.py` and `git diff --check HEAD` against workspace content.
Apply all Rust gates from [testing](../../../docs/TESTING.md) when the manifest/source exists
and the change makes them applicable. Absence of a manifest during documentation bootstrap
is a recorded non-applicability reason, not a Rust test pass.

Declare task-specific tests and measurements before execution. Native checks belong to the
Runner's established inventory; optional live Telegram access requires explicit authorization.
A task may explicitly assign a genuinely unavailable runtime/network measurement to HOST.
That remains a mandatory acceptance gate after the run, not an implicit Native check or a
requirement to wait for the operator before the planned commit. Report HOST work as pending
until performed; environment anecdotes from other projects establish no capability here.

Never modify tests, checks or skill requirements merely to obtain a pass. Use the Runner's
structured environment-blocker path for an observed inability to execute required Native
checks. The Runner freezes inventories/configuration; only its authorized finalization turn
attests finalization. A later content repair invalidates earlier review/finalization evidence.

## Git ownership by pipeline

| Pipeline | Content/index/commit boundary |
| --- | --- |
| `plan-authoring` | Project content read-only; only resolved ignored clarification/plan artifact paths may be written by the Runner. No staging or commits. |
| `plan-execution` | Worker implements the accepted step. Only the one-shot Runner-authorized Worker `COMMIT` turn stages and creates its exact validated subject-only commit. Earlier turns never stage or commit. |
| `polishing` | Writable Worker turns may repair safe content. No agent turn writes the index or commits; Runner alone stages the reviewed result at `HANDOFF`. |

Reserve `git diff --cached --check` and staged diff inspection for the Runner-owned COMMIT or
HANDOFF boundary. Never add an index-relative prerequisite to bootstrap or FINALIZE. The
coordinator keeps checkout, inputs, guidance and config unchanged while Runner owns the run.

The user grants standing permission for the coordinator to commit in-scope work from a
successful `polishing` handoff locally, without asking again. Wait for `DONE` and released
workspace ownership, verify that staged content still matches the exact finalized snapshot
accepted by the selected mode's required review/confirmation, then commit under
[git-workflow](../git-workflow/SKILL.md).
Respect later user instructions to withhold a commit. This permission does not cover unrelated
manual work. No pipeline grants push, branch/ref changes, identity overrides, history rewrites,
bodies/footers or co-author trailers.
