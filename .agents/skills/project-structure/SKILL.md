---
name: project-structure
description: Choose file ownership and update navigation when adding, moving or splitting Telegram MCP Relay modules, configuration, tools or documentation.
---

# Project Structure

Read [conventions](../../../docs/CONVENTIONS.md) and the [document map](../../../docs/README.md).
Use one root Rust application crate; create directories only with real content. The intended
`dto`, `handlers` and `routes` boundaries follow the Axum template, with domain/services and
adapters introduced only as needed. Do not create a duplicate HTTP stack around the MCP SDK.

Keep transport types at boundaries, rules in domain code, orchestration in services, SQLite
transactions in repositories and Telegram HTTP in clients. Configuration/errors stay central.
Place integration tests under `tests/`, examples under `config/`, service units under `deploy/`
and justified tooling under `tools/`. No generic `utils` dumping ground or speculative traits.

Update `AGENTS.md` and owning documents when the actual layout changes. Distinguish planned
paths from implemented ones. Canonical skills live here; `.claude/skills` remains a symlink.
Keep plans, runner config and reports ignored under `LOCAL_ARTIFACTS/`; durable documents
must not depend on those local files.
