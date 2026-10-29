---
name: test-authoring
description: Design or review deterministic Relay unit, SQLite recovery, MCP and mocked Telegram tests that protect observable contracts.
---

# Test Authoring

Use the contracts and acceptance matrix in specification sections 4–12 and 16. Implement tests
through [delegation](../delegation/SKILL.md); find required commands in
[testing](../../../docs/TESTING.md).

- Every case must catch a concrete regression or a distinct observable requirement. Avoid
  assertions about copied wording, file layout, private function shape or coverage counts.
- Use pure in-module tests for formatting/state rules and integration tests for transactions,
  restart, protocol delivery and streaming. Do not duplicate the same proof across layers.
- Use neutral synthetic data, temporary databases/files and a mocked Telegram API. Never send
  real user messages in default tests or require a bot token for ordinary validation.
- Control time and scheduling using injected clocks, paused Tokio time or explicit events.
  No artificial sleeps, retry-until-green, timing-dependent ordering or leaked tasks/resources.
- Cover 1,000/1,001 final scalar boundaries and native caption bounds, same-title interleaved
  routes, durable cursor atomicity, token fencing, event/wait races, cancellation/restart,
  processing notices, retention, authorization and bounded file streaming as features land.
- For a reproduced bug, retain evidence that the regression fails before the fix and passes
  after it when practical. Preserve safety assertions; do not skip failures or reduce
  concurrency to conceal races. Explain a retained replacement before deleting redundant tests.
- Run focused affected tests plus required gates. Record actual outcomes; a sandbox failure
  before execution is neither an assertion failure nor a pass. Keep live Telegram checks opt-in
  and separately authorized. Do not repeat expensive broad runs without an unresolved reason.
