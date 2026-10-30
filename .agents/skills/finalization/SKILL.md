---
name: finalization
description: Finalize Relay source, tests, documentation and repository workflow changes before handoff or an authorized local commit, including Runner staging boundaries.
---

# Finalization

Read [testing](../../../docs/TESTING.md) and apply the complete scope below. Inside any Agent
Runner pipeline also read [agent-runner.md](agent-runner.md); this `SKILL.md` is the discoverable
finalization entry point, while that reference defines the pipeline-specific division.

1. Check the entire result against the request, accepted plan and relevant specification
   sections. Preserve unrelated user changes. Current docs must describe actual implementation
   status and disclose protocol/client limitations.
2. Check boundaries from [conventions](../../../docs/CONVENTIONS.md): one lightweight Rust
   process, thin transport code, deterministic domain rules, atomic durable delivery and bounded
   effects. Apply [test-authoring](../test-authoring/SKILL.md) to test necessity and failure paths.
3. Inspect for credentials, authorization, session keys, unsafe logs, real fixtures, path
   escape, accidental artifacts and silently weakened validation. Verify changed security
   behavior against specification section 12.
4. Run every applicable gate in `docs/TESTING.md` and the accepted task inventory. An unrun
   required check remains pending with its actual cause and owner. Do not call absent Cargo
   checks passing in a documentation-only checkout.
5. Reconcile root README availability, `AGENTS.md` map, document index and affected owning docs.
   Validate changed skills and links, and keep canonical symlinks intact. Keep local plans,
   run state/configuration, databases and build output out of Git.
6. Complete the required independent review after the last content correction. For a requested
   subagent quality loop require a full final zero-finding pass, not only verification of old
   fixes. Any later content change invalidates that reviewed snapshot.
7. Follow [git-workflow](../git-workflow/SKILL.md). Outside Runner, stage/commit only a cohesive
   change set covered by the user request or standing polishing permission. Commit a successful
   in-scope polishing handoff without another approval question under the
   [handoff conditions](agent-runner.md). Report unauthorized work without claiming a commit.
   Inside Runner obey its exclusive COMMIT/HANDOFF ownership. Never push or rewrite history.

Report the result, material checks and independent review outcome, actual Git state and any
remaining acceptance evidence. Identify actual commits by hash/subject. Runtime/measurement
claims require observed results; a successful local commit is not full operational acceptance.
