# Testing and Required Checks

## Repository gates

Run `just check-repo` (or `python3 tools/check_repository.py`) after documentation, skills,
navigation or repository workflow changes. It checks local Markdown link destinations, required
skill frontmatter, canonical symlinks, the tracked Runner starter configuration and ignored,
untracked local artifacts. It uses only Python 3.9+ and Git, no third-party modules. It checks
workspace files including untracked nonignored files; it does not validate external URLs,
Markdown anchors, full YAML schemas or the installed Runner's complete configuration schema.
Runner itself validates the operational configuration when starting a run.

During skill authoring, also use the available skill-creator `quick_validate.py` if installed.
Its environment-specific path is an operator detail, not a portable required command.
The repository gate is the committed skill/navigation baseline.

After the initial bootstrap commit, also run `git diff --check HEAD`. During the initial
unborn-HEAD bootstrap, inspect all new content and run `git diff --cached --check` only after
authorized staging. `.gitattributes` preserves the original specification's Markdown hard-break
spaces; other files retain normal Git whitespace checks.

## Rust gates

The root application crate makes Rust gates applicable. The complete dependency resolution
is in `Cargo.lock`; source, tests, Cargo/dependency or Rust tooling changes require:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items
cargo test --locked --workspace
```

`just check-rust` runs these unchanged. `just check` runs the repository gate and, when the
manifest exists, the complete Rust gate; it explicitly reports non-applicability otherwise.
Commit `Cargo.lock` with the first manifest and use locked checks thereafter. Creating/updating
the lock is a deliberate implementation step, not an automatic repair performed by a check.
Run focused affected tests while developing as well; do not repeat broad gates after a clean
result without a subsequent change or unresolved concern.

## Behavioral acceptance

Use the specification's complete section 16 matrix and [test-authoring](../.agents/skills/test-authoring/SKILL.md).
Pure domain tests cover grapheme title normalization, final scalar text limits and native
caption limits. Integration tests cover the interactions that pure tests cannot prove:

- Three concurrent sessions, same-title isolation and Replies to older text/documents.
- Durable Reply/cursor transactions, restart and duplicate ingestion, claim fencing,
  idempotent acknowledgment, capacity pressure and independent TTL cleanup.
- Resource reads, modern subscribed notifications and an active waiter together; cancellation,
  timeout, reconnect and recovery without duplicate claims or lost pending replies.
- Confirmed-processing typing, its non-resettable maximum and once-only unconfirmed notices
  with controlled time; long processing must not produce a warning.
- Mock Telegram failures/uncertain sends, authorization, safe path opening and near-limit
  streaming with bounded buffers. Preserve attachment metadata without downloading by default.

Default tests use local fake transports, synthetic IDs and temporary SQLite/files. Never require
real bot credentials or send real Telegram messages in CI. Optional live integration needs a
separate explicit user request and must disclose what client/API behavior it actually proves.

## Measurements and check ownership

Release acceptance records Linux target/toolchain, stripped locked release binary size,
dependency profile, startup time, idle RSS/CPU and peak RSS during a near-limit streamed upload.
Use an identified reproducible workload and state whether Telegram is mocked. The sub-30 MB
idle target is best effort, not an unmeasured guarantee or a brittle unit-test assertion.
Add the measurement command and owning document when the implementation provides them.

Native Runner commands use the accepted staging-independent inventory and
[Runner finalization](../.agents/skills/finalization/agent-runner.md). Staged inspection belongs
only at COMMIT/HANDOFF. Assign unavailable HOST measurements explicitly in task gates; they
remain mandatory final acceptance evidence after the run and never become fabricated passes.
Observe actual network/cache/socket failures before classifying a capability blocker. Report
the command, cause and required owner; do not weaken a check or substitute an unrelated test.
