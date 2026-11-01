---
name: project-structure
description: Choose file ownership and update navigation when adding, moving or splitting Telegram MCP Relay modules, configuration, tools or documentation.
---

# Project Structure

Read [conventions](../../../docs/CONVENTIONS.md) and the [document map](../../../docs/README.md).
Use one root Rust application crate; create directories only with real content. Typed MCP
schemas live in `dto`, shared SDK dispatch in `handlers` and protected HTTP wiring in `routes`.
Keep domain/services and adapters focused. Do not duplicate the SDK's HTTP stack.

Keep transport types at boundaries, rules in domain code, orchestration in services, SQLite
transactions in repositories and Telegram HTTP in clients. Configuration/errors stay central.
`services::history` authorizes read-only journal queries; `services::attachments` revalidates
retained own/shared provenance before download effects. `services::downloads` owns private
files/reservations/retirement; repository attachment/download modules own durable provenance
and file identities. `services::guidance` projects nonsecret current policy and capabilities.
Place integration tests under `tests/`, configuration examples under `config/`, service units
under `deploy/` and justified tooling under `tools/`. The `tools/resource_probe.rs` Cargo example
belongs to the root crate; the `tools/runner-check/` helper has its own workspace and lockfile.
No generic `utils` dumping ground or speculative traits.

Update owning documents when the actual layout changes and reconcile the `AGENTS.md` map
after Runner ownership is released; never edit run-frozen guidance. Distinguish planned paths
from implemented ones. Canonical skills live here; `.claude/skills` remains a symlink.
Keep plans, runner config and reports ignored under `LOCAL_ARTIFACTS/`; durable documents
must not depend on those local files.
The portable Runner creation recipe is inline in [development](../../../docs/DEVELOPMENT.md);
do not restore a tracked Runner JSON template. Application examples remain under `config/`.
