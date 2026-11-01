# Development and Agent Runner

## Local prerequisites and bootstrap

Repository checks require Git and Python 3.9+; `just` is an optional command index. Application
work requires a Rust toolchain with rustfmt/Clippy and access to its declared dependencies.
Agent Runner and a supported configured backend are development tools, not application runtime
dependencies. The resulting daemon must not require Node.js.

### Portable Runner configuration

The complete portable starter lives inline here; there is no separate tracked JSON template.
It uses project schema version 1, `LOCAL_ARTIFACTS` as the artifact root, Codex as the backend
and `current` profile/model/effort/context settings inherited from the installed environment.
All three pipelines default to explicit `independent` mode. Execution/polishing discover
`.agents/skills/finalization/SKILL.md` through `finalization: "auto"`; trusted-check aliases
start empty. Backend credentials, machine paths, command vectors and root-only settings do
not belong in this portable example.

<!-- runner-config-example -->
```json
{
  "schemaVersion": 1,
  "artifactRoot": "LOCAL_ARTIFACTS",
  "defaultBackend": "codex",
  "defaultProfile": "current",
  "defaultModel": "current",
  "defaultEffort": "current",
  "defaultContextSize": "current",
  "pipelines": {
    "plan-authoring": {
      "mode": "independent",
      "preferredCommitLineLimit": 900
    },
    "plan-execution": {
      "mode": "independent",
      "finalization": "auto",
      "trustedChecks": []
    },
    "polishing": {
      "mode": "independent",
      "finalization": "auto",
      "trustedChecks": []
    }
  }
}
```

For a fresh trusted checkout, run this from the repository root to create its ignored
operational configuration. The script rejects a symlinked artifact directory and uses exclusive
creation: an existing destination, including a dangling symlink, is never overwritten. Inspect
existing directory ownership/permissions first; this recipe does not repair them.

```bash
umask 077
python3 -B - <<'PY'
import json
from pathlib import Path
import re

source = Path("docs/DEVELOPMENT.md").read_text(encoding="utf-8")
examples = re.findall(
    r"(?ms)^<!-- runner-config-example -->\n```json\n(.*?)\n```[ \t]*$", source
)
if len(examples) != 1:
    raise SystemExit("Expected one marked inline Runner configuration.")
json.loads(examples[0])
directory = Path("LOCAL_ARTIFACTS")
directory.mkdir(mode=0o700, exist_ok=True)
if directory.is_symlink() or not directory.is_dir():
    raise SystemExit("LOCAL_ARTIFACTS must be a real directory.")
with (directory / "agent-runner.json").open("x", encoding="utf-8") as destination:
    destination.write(examples[0] + "\n")
PY
git check-ignore LOCAL_ARTIFACTS/agent-runner.json
```

If creation reports an existing configuration, preserve and use that file; do not delete it
to repeat bootstrap. No prepared checkout, existing configuration or trusted grant is assumed.
The repository checker validates this marked JSON example without reading ignored local settings.

Select trusted-check aliases only from locally established, explicitly authorized Runner
grants before ownership or at a supported setup boundary. Definitions, exact machine vectors,
capabilities, provisioning and private settings stay in protected local configuration; copying
this example grants no command execution capability. Preserve existing configurations, tasks,
receipts and counters rather than replacing them with starter defaults.

The final-eight agent-experience continuation is an explicit user-selected **lazy** exception:
fresh `plan-authoring` and `plan-execution` use lazy/current sessions without `sourceSession`.
Installed Worker convergence and fresh read-only confirmation apply, without an independent
per-step product-review gate. Full applicable FINALIZE checks and COMMIT ownership remain
mandatory. This exception does not change the portable independent/current/auto defaults or
authorize editing a run's resolved configuration.

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
3. Start `plan-authoring` using `run_start`, with `independent` as the default unless the user
   explicitly selects another mode as above. Planning is project-read-only;
   only the Runner-owned resolved clarification/plan paths are writable. Monitor `run_status`,
   `run_activity` and bounded `run_wait`; use `run_respond` only for a real pending question.
4. Verify the completed reviewed plan and its exact Conventional Commit subjects. Begin
   `plan-execution` against the clean accepted baseline, with explicit task-scoped permission
   for the planned local commits. Its Worker implements, review/confirmation follows the selected
   mode, finalization attests required evidence and a separate Runner-authorized COMMIT turn creates
   each local commit. Do not replace that lifecycle with a coordinator commit.
5. Inspect the resulting status, commits, checks and remaining HOST evidence before acceptance.
   A returned run identifier means work started, not that implementation finished. Report actual
   durable status and unresolved blockers accurately.

For existing changes needing refinement, `polishing` offers independent review/finalization
and a Runner-owned staged HANDOFF. The pipeline does not commit. The user grants standing
permission for the coordinator to commit its in-scope successful result locally without
asking again, after `DONE` and released workspace ownership. Verify that staged content still
matches the exact finalized, independently reviewed snapshot, and respect later instructions
to withhold a commit.

Use [delegation](../.agents/skills/delegation/SKILL.md),
[finalization](../.agents/skills/finalization/SKILL.md) and
[git-workflow](../.agents/skills/git-workflow/SKILL.md) together. A task may authorize local
planned commits; it never implicitly authorizes push, branches, history rewrite, remote changes,
deployment or live Telegram messages.

## Active-run discipline and recovery

Runner freezes its configuration, task/clarification inputs, guidance and check inventory.
Do not edit them or the owned working tree/index from a supervising session during a run.
Use supported pause/respond/resume/cancel operations and preserve the persisted run contract.
Independent review is the default; honor an explicit user-selected mode established before
the run freezes. Do not switch modes during ownership or bypass a gate to save time. Lazy
convergence/confirmation cannot substitute for required validation.

Network, cold dependency caches and socket permissions vary by executor. Use the actual error
to distinguish an unavailable command from a failed assertion. Recover with supported operator
setup or explicitly assigned HOST evidence; code fixes remain Runner-authored. If the required
contract cannot be satisfied, report the concrete blocker and unfinished state instead of
silently replacing the execution route or weakening tests.

No background-run status, clean Git tree or successful commit alone establishes full MVP
acceptance. Check [testing](TESTING.md), actual runtime capabilities and operational evidence.

## Read-only Cargo validation

`tools/runner-check/` is a dependency-free, standalone Rust development crate for the
Runner-trusted read-only checkout route. Its interface is exactly:

```text
runner-check MODE --toolchain-bin <absolute-bin> --registry-source <relative>
```

The six modes are `clippy-workspace`, `doc-workspace`, `test-workspace`, `clippy-helper`,
`doc-helper` and `test-helper`. Each executes the corresponding complete check in
[testing](TESTING.md), adding offline resolution and private target placement. Helper modes
select the standalone manifest; `test-helper` never recurses or runs the product suite.
The launcher replaces itself with Cargo so exit codes and signals propagate directly.

The exact actual Cargo executable and toolchain bin path belong in ignored operational
configuration. Child `RUSTC` and `RUSTDOC` select those actual binaries, and child `PATH` places
that same toolchain before ordinary system utilities for Cargo/Clippy lookup. This configures
the child inside the existing sandbox; it grants no new toolchain mount or Runner environment.
Host HOME and inherited Cargo overrides are excluded from the child environment.

Each invocation requires declared scratch and cache. The launcher creates exclusive private
directories below them, places all target and temporary output in scratch and Cargo state in
cache, then relies on Runner's process-tree retirement and storage cleanup after exec. It
reads the selected committed lockfile and copies only its crates.io sparse-index entries,
registry index configuration and matching archives from the supplied repository-relative
registry root into its fresh `CARGO_HOME`. Archive SHA-256 values must match the lockfile;
Cargo performs complete locked, offline dependency resolution. Missing/corrupt inputs fail
without fetching. Links, special files and path escapes on input paths are rejected. Registry
sources, credentials, Cargo configuration, global cache databases and unrelated ignored files
are never copied. Project sources stay in the read-only checkout.

Provision the approved repository-local index/archive cache only before run ownership or at
a supported verified paused-idle setup boundary. Keep it immutable while owned. Preserve old
configuration, task, plan and guidance files; select a fresh reviewed configuration before
starting a run that needs different command declarations. Do not weaken isolation or treat
synthetic capability probes as evidence that product assertions passed. Selected commands
run only in FINALIZE through their frozen vectors, followed by the selected mode's confirmation
and Runner-owned COMMIT/HANDOFF as appropriate. Pending HTTP execution claims remain pending
until genuine results.

## Release probe ownership

The explicit [library-linked release probe](MEASUREMENTS.md) is a root Cargo example, not a
new runtime service or production endpoint setting. Its R1/R2 release builds remain mandatory
Runner-managed FINALIZE checks on the declared Native route, with locked dependency inputs
and private writable output. The standalone helper still has exactly the six modes above;
probe introduction adds no helper mode, dependency or selected trusted command.
Do not infer release-build success from development compilation or a HOST measurement.

[Bounded Python collectors](MEASUREMENTS.md#bounded-collectors) inspect existing artifacts and
Linux samples without building or orchestrating a workload. Their P1/P2 source/unit gates are
Native FINALIZE obligations; no Python lint gate is added. The [HOST controller](MEASUREMENTS.md#host-controller-and-prerequisites)
requires coordinator-exported actual public successful R1/R2/tooling receipts and records
source/build/workload identity before capture. It may materialize unchanged offline release
artifacts in private temporary output only after those gates pass. Runtime capture and
user-unit verification belong to the task's HOST boundary, after applicable tooling/release
gates and verified quiescent ownership. They do not
run inside implementation/review turns or replace required checks. The
[recorded capture](MEASUREMENTS.md#recorded-host-capture) integrates actual mocked figures at
commit `968802371f94f51eedab66998eff6031762db5ea` and prior public aggregate check receipts;
later documentation changes do not relabel that source identity or inherit its check results.
The initial HOST unit verification failure remains preserved. A
[later successful verification](MEASUREMENTS.md#unit-verification-and-remaining-acceptance)
followed explicitly authorized installation of the matching prebuilt binary. Those static/mock
phases installed, enabled or started no service unit and performed no live-client trial.
The separately dated [baseline observations](MEASUREMENTS.md#dated-baseline-host-observations)
record later limited text flow, transient one-process operation, no idle wake and canonical-unit
runtime failure. The [extension HOST obligations](MEASUREMENTS.md#agent-experience-host-acceptance)
remain pending after actual final-eight DONE and released ownership; they require no new
benchmark or changes to the historical controller's closed independent-run receipt schema.
