# Agent Runner Finalization

The installed Runner guidance and accepted task gates define the active contract. This file
preserves this repository's pipeline boundaries; it is a supporting reference, not a valid
`finalization` setting. Use `auto` or `.agents/skills/finalization/SKILL.md` as the skill setting.

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

A later coordinator commit of a polishing handoff is a separate user-authorized action after
the run has ended and ownership is released. No pipeline grants push, branch/ref changes,
identity overrides, history rewrites, bodies/footers or co-author trailers.
