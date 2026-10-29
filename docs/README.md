# Documentation Map

Read the owning document before changing its behavior, and update it in the same change.
Keep implementation status explicit; requirements and intended structure are not runtime proof.

| Document | Responsibility | Update when |
| --- | --- | --- |
| [Technical specification](product/TECHNICAL_SPECIFICATION.md) | Original requirements, phases, acceptance and external references | The owner approves a product requirement revision; otherwise preserve the input |
| [Conventions](CONVENTIONS.md) | File ownership, Rust and HTTP conventions | Layout, naming or dependency policy changes |
| [Architecture](ARCHITECTURE.md) | Intended boundaries and current implementation status | A component or protocol capability is implemented or changes |
| [Testing](TESTING.md) | Required gates, test ownership and acceptance evidence | A check or verification boundary changes |
| [Development](DEVELOPMENT.md) | Runner setup, task/plan/execution lifecycle and Git boundaries | Operator workflow or runner configuration changes |

Root [README.md](../README.md) owns the short product entry and current availability;
[AGENTS.md](../AGENTS.md) owns contributor instructions. Skills under
[.agents/skills](../.agents/skills/) own reusable workflows and link to these documents.

Working task documents, clarifications, commit plans and review reports belong under ignored
`LOCAL_ARTIFACTS/`. Keep durable implemented decisions in the owning document, rather than
making tracked documentation depend on a private local artifact.
