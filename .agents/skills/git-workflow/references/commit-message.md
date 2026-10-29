# Commit Messages

Read before drafting a message through [git-workflow](../SKILL.md#commit-message).

Use one Conventional Commits subject, about 72 characters or fewer:

```text
type(scope): imperative summary
```

Use lowercase type/scope, imperative verbs and no trailing period. No body by default; add one
only outside Runner when the change is genuinely unclear without it. Runner plan subjects
are exact subject-only commit messages, consumed unchanged at execution. Never add trailers,
co-authorship, generator attribution, identity overrides or file-list bodies.

| Type | Meaning |
| --- | --- |
| `feat` | New behavior or capability |
| `fix` | Correctness, security, build or documented factual defect |
| `docs` | Documentation changes |
| `refactor` | Structure changes preserving behavior |
| `test` | Behavioral coverage and fixtures |
| `perf` | Measured resource/performance improvement |
| `chore` | Repository workflow, tooling or dependency maintenance |
| `ci` | Automated check configuration |

| Scope | Owner |
| --- | --- |
| `repo` | Root structure and shared repository configuration |
| `agents` | `AGENTS.md`, skills and agent workflow |
| `mcp` | MCP tools, resources and transport |
| `telegram` | Telegram transport, formatting and upload |
| `storage` | SQLite, claims, cursor and retention |
| `config` | Application configuration and validation |
| `docs` | Cross-cutting project documentation |
| `ops` | Local deployment and measurements |

Prefer scopes already in recent history. Add a short capability scope when genuinely useful;
use a bare `type: summary` when none fits. Never imitate historical commits that violate these
rules and never rewrite them to enforce this convention.

```text
chore(repo): establish Rust project and runner conventions
feat(storage): persist replies with an atomic update cursor
fix(mcp): reject stale reply claim tokens
test(telegram): cover final caption Unicode limits
```
