---
name: subagent-quality-loop
description: Execute, check or refine a task with a separate executor and independent reviewer until a complete review finds zero issues. Use when the user asks for subagent quality control, such as "довести сабагентами", "проверить с сабагентами", "проследить с сабагентами", or "improve with subagents".
---

# Subagent Quality Loop

Use this workflow for any task domain when the user requests execution, verification or improvement through subagents.
The parent owns scope and acceptance. A separate executor performs the work, and a separate independent reviewer checks it.
Completion requires one complete review pass of the final result with **zero findings**.

## Establish scope and roles

- Read applicable instructions and domain skills. This skill coordinates their execution without replacing their requirements or permission boundaries.
- State the requested outcome, relevant inputs, acceptance criteria, deliverables and authorized actions from the conversation. Include correctness criteria appropriate to the task, not only the user's explicit checklist.
- Derive clear criteria from the request and applicable instructions without a routine questionnaire or approval step. Clarify only material ambiguity that cannot be resolved from context.
- Preserve explicit read-only limits, unrelated work and existing user changes. A review request grants no additional permission to publish, send messages, commit, push, deploy or mutate external systems.
- Use existing authorization without requesting it again. If a necessary correction exceeds authorized scope, prepare a concrete correction proposal and report that part as pending.
- Assign two distinct subagents: an executor/fixer and an independent reviewer who did not author the result being checked. The parent cannot substitute its own work or review for either role.
- Reuse a domain workflow's distinct executor/reviewer pair when it covers the full task and satisfies these gates. Assigned agents must not recursively create more pairs.
- Give both agents the user request, constraints, source locations, acceptance criteria, permitted actions and baseline where relevant. Give the executor clear ownership to avoid concurrent edits.
- For multipart work, the parent defines component boundaries and retains responsibility for the integrated result. Clean component reviews alone do not establish a clean combined result.

## Execute and hand off

1. The executor performs or improves the authorized work, saves the deliverable and runs appropriate checks required by the task or domain skill.
2. Identify the exact review target using file paths and hashes, a revision identifier, an immutable answer draft or an equivalent reproducible snapshot. Do not create a commit merely to identify a version.
3. The executor hands over the result, change summary, check results and remaining uncertainties. These are review inputs, not proof of correctness.
4. The parent sends that exact result and its underlying inputs to the reviewer. Keep it stable during review. If it changes, the review must restart on the new version.

## Independent review

- The reviewer independently reads the actual result and relevant sources, verifies task completion and checks correctness, consistency and regressions appropriate to the task.
- Cover the entire requested scope, including combined outputs and interactions. A checklist-only pass, a narrow diff check, passing tests or reliance on the executor's claims is insufficient.
- Reproduce or inspect material evidence where appropriate and authorized. Missing essential evidence or unavailable access is a review blocker, never an assumed pass.
- For completed external actions, inspect the resulting state and receipts through permitted reads. Never repeat a mutation merely to obtain another review pass.
- The reviewer does not edit the deliverable. Return actionable findings with a stable ID, location or affected output, evidence, impact and required correction.
- Treat any demonstrated defect within scope as a finding regardless of severity. Do not hide minor issues to obtain a clean result.
- Keep optional alternatives separate from findings. A personal preference or unrelated enhancement is not a defect without a concrete task requirement or correctness reason, and must not expand the task into endless polishing.
- Report the reviewed version, scope covered, checks performed and either all findings or an explicit complete zero-finding result. Do not claim a complete pass if any essential part was skipped or blocked.

## Fix and repeat

1. The parent tracks every finding and its status in the conversation or an appropriate existing task record. Do not require a new artifact solely to run this loop.
2. The executor fixes every valid finding within the authorized scope and reports the correction and relevant checks by finding ID.
3. If a finding is disputed, the executor supplies evidence. Only the reviewer may withdraw it, with a source-based explanation recorded in the handoff. Never silently ignore or waive a valid finding.
4. Return the complete updated result to the same independent reviewer where possible. A replacement reviewer must receive the full context and perform a complete independent pass.
5. The reviewer verifies previous resolutions and conducts a fresh review of the entire current result for new issues and regressions. Fix verification alone does not qualify.
6. Continue until a full pass reports zero new, reopened or unresolved findings. After withdrawals or fixes, require a fresh full pass rather than relabeling a pass that contained findings as clean.

Do not replace this stopping condition with an iteration cap, severity threshold, tests, partial coverage or executor self-approval.
If an essential fix, evidence source or independent agent remains unavailable, report the concrete blocker and unfinished state. Do not fabricate a clean review or broaden permissions to bypass the blocker.
User cancellation or an actual execution limit can stop work, but cannot make incomplete work complete.

## Final acceptance

- The parent checks that the final combined result matches the exact version reviewed, all findings are resolved or explicitly withdrawn with evidence, and the last complete review itself had zero findings.
- Any subsequent change to the deliverable invalidates that acceptance. Revalidate and obtain another complete independent pass before claiming completion.
- Preserve separate domain gates for publication, deployment, operational acceptance and other external actions. A clean review certifies only the task scope actually reviewed.
- Hand off the result concisely with its location or content, material validation and the final zero-finding review outcome. State remaining blockers instead of claiming completion when the gate was not met.
