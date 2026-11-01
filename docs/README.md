# Documentation Map

Read the owning document before changing its behavior, and update it in the same change.
Keep implementation status explicit; requirements and intended structure are not runtime proof.

| Document | Responsibility | Update when |
| --- | --- | --- |
| [Technical specification](product/TECHNICAL_SPECIFICATION.md) | Original requirements, phases, acceptance and external references | The owner approves a product requirement revision; otherwise preserve the input |
| [Conventions](CONVENTIONS.md) | File ownership, Rust and HTTP conventions | Layout, naming or dependency policy changes |
| [Architecture](ARCHITECTURE.md) | Intended boundaries and current implementation status | A component or protocol capability is implemented or changes |
| [Configuration](CONFIGURATION.md) | Settings, bounds and protected secret sources | Startup configuration or secret handling changes |
| [Setup](SETUP.md) | Locked builds, Telegram setup, local secrets, MCP clients and optional user service | Installation or client requirements change |
| [Agent operating policy](AGENT_POLICY.md) | Copyable section 14 policy, guidance-first interaction and durable reply/file recovery | Installed-agent interaction guidance changes |
| [Resource measurements](MEASUREMENTS.md) | Recorded HOST benchmark, user-unit verification outcome, release/build identities and capture prerequisites | Measurement tooling or operational evidence changes |
| [Testing](TESTING.md) | Required gates, recorded validation provenance, specification acceptance/traceability and deliverable coverage | A check, receipt or verification boundary changes |
| [Development](DEVELOPMENT.md) | Runner setup, task/plan/execution lifecycle and Git boundaries | Operator workflow or runner configuration changes |

Root [README.md](../README.md) owns the public tool/resource and DTO reference, handle permissions,
error/recovery categories, synthetic workflows and current availability;
[AGENTS.md](../AGENTS.md) owns contributor instructions. Skills under
[.agents/skills](../.agents/skills/) own reusable workflows and link to these documents.

For acceptance, distinguish source behavior in architecture, executed checks in
[testing](TESTING.md#agent-experience-composition-validation), and dated HOST observations in
[measurements](MEASUREMENTS.md). Setup commands and specification criteria are not executed evidence.

Working task documents, clarifications, commit plans and review reports belong under ignored
`LOCAL_ARTIFACTS/`. Keep durable implemented decisions in the owning document, rather than
making tracked documentation depend on a private local artifact.
