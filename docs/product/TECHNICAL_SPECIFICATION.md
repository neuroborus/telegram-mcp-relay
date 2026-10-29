# Telegram MCP Bridge — Technical Specification and Implementation Task

**Status:** Proposed MVP specification  
**Date:** 2026-10-29  
**Preferred implementation:** Rust  
**Target:** One lightweight, local, vendor-neutral MCP server shared by multiple agent sessions  
**Primary user interface:** A single Telegram bot

> **Implementation instruction:** Build the smallest correct solution satisfying the requirements and acceptance criteria below. Prefer well-defined behavior, persistence, and protocol compliance over extra abstractions. Do not implement Codex-, Claude-, or other vendor-specific adapters in the MVP.

---

## 1. Goal

Build a small **Telegram ↔ MCP bridge** that lets many independent AI agent sessions communicate with one Telegram user through one bot.

An agent can send short text, ask questions, and send files. **Every outgoing text message has a strict 1,000-character maximum, including the session header**; longer information must be sent as a file, not broken into a series of chat messages. Every agent-originated Telegram message identifies its session with a leading `[Session Title]` of at most **50 visible characters**. The user can **Reply** to *any* of those messages—including a document—and the bridge routes that reply to the correct agent session, not simply to the last agent that spoke.

For incoming replies, support **standard MCP resource-change notifications whenever clients subscribe**, **plus an optional, bounded wait mechanism** for clients that cannot act on asynchronous events. These are complementary mechanisms, never exclusive modes. Replies remain durable when no agent is available.

Keep resource usage extremely low: one native process, minimal allocations and CPU wakeups, a small bounded SQLite database, no in-process LLM, no Node.js, no message broker, and no service or process per agent.

### Core principles

1. **Vendor-neutral:** The bridge implements standard MCP and Telegram Bot API only. No client-specific wake APIs in the MVP.
2. **One shared process:** All agents share a local MCP endpoint, Telegram connection, and database.
3. **Event-first, optional wait:** Publish MCP events whenever there is a subscribed client, even while a tool invocation is waiting for the same reply.
4. **Never lose accepted replies before their retention deadline:** Persist before signaling a waiter or subscriber; delete/mark complete only on an explicit processing acknowledgment.
5. **Do not manage the agent lifecycle:** No agent process launching, stopping, session scraping, or periodic polling of agents.
6. **No invented guarantees:** Standard MCP notifications do **not** guarantee that a host will start an LLM turn. A wait tool cannot be invoked by an agent that is no longer running.
7. **Minimal notifications:** Only notify the user about meaningful delivery uncertainty or an unroutable reply, not routine internal activity.
8. **Bounded local storage:** Default 30-day TTL on bridge metadata and uncompleted replies. Telegram retains the user-visible conversation independently; Telegram is **not** a complete replay API for the bridge.
9. **Short chat, files for detail:** Enforce a strict 1,000-character cap on outgoing Telegram text (including `[Session Title]`), rather than splitting, truncating, or flooding the chat. Use `telegram_send_file` for substantial reports, logs, code, or long explanations.

## 2. Scope and non-goals

### MVP includes

- Rust single-binary application for Linux, primarily a local desktop installation.
- One local Streamable HTTP MCP endpoint serving multiple simultaneous clients.
- One Telegram bot and one or more explicitly allowlisted user/chat IDs (optimized for a single private chat).
- Telegram long polling with `getUpdates`, with one outstanding request; no inbound public endpoint.
- Registration of agent sessions and short human-readable titles.
- Short text sending with a strict 1,000-character limit, plus document sending with large-file-safe streaming and outbound message-to-session routing.
- Telegram Reply routing to the original session, including replies to sent documents.
- Durable inbox with at-least-once recoverability, explicit processing state, deduplication, and acknowledgments.
- MCP resource subscription support plus optional finite tool-level waiting, concurrently.
- Temporary `typing` indication when an agent confirms that it **started** processing a reply, capped at 10 minutes.
- Minimal service notices when processing of a Reply remains unconfirmed or the Reply cannot be routed.
- 30-day default TTL, periodic cleanup, configurable limits, safe restart and reconnection handling.
- Clear agent policy, README/setup documentation, tests, and resource-use measurements.

### Deliberate non-goals

- Integrating with the Codex, Claude, or other proprietary agent runtimes.
- Guaranteeing an LLM wakes up merely because the MCP server emits a notification.
- Keeping an HTTP request open indefinitely; `wait` has a bounded tool timeout.
- Running one Telegram bot instance per agent, or one MCP server per agent.
- Webhook/tunnel/reverse-proxy infrastructure for the local MVP.
- Duplicating conversation history or outbound file contents in SQLite.
- Hosting a web UI, using Redis/NATS/PostgreSQL, or building a generic workflow scheduler.
- Retrieving arbitrary historical Telegram messages on demand after bridge records expire.
- Handling unrestricted multi-tenant access or assuming all MCP clients on the same computer are mutually trusted.

**Possible later extensions:** optional incoming-file download tool, public webhook mode, runtime-specific wake adapters, richer attachment processing, cross-host file transfer. Keep extension points small; do not pre-build plugin frameworks.

---

## 3. Architecture

```text
         Agent session A        Agent session B        Agent session C
              |                      |                      |
              +----------------------+----------------------+
                                     |
                         MCP Streamable HTTP
                         127.0.0.1:8765/mcp
                                     |
                +----------------------------------------+
                |          Telegram MCP Bridge           |
                |                                        |
                |   MCP tools and resource subscriptions |
                |   Session and message route registry   |
                |   Durable reply inbox + acknowledgments|
                |   Optional asynchronous reply waiters  |
                |   Typing indicator / service notices   |
                |   Telegram HTTP client                 |
                |              SQLite                    |
                +----------------------------------------+
                                     |
                      Telegram Bot API (HTTPS)
                         getUpdates long poll
                         sendMessage/sendDocument
                         sendChatAction/getFile
                                     |
                           User in Telegram
```

**Process model:** One OS process. Prefer Tokio's current-thread runtime where it is viable. Use a single shared HTTP client with connection reuse. SQLite should be embedded (`rusqlite`); short queries only, without blocking the async event loop with heavy database work. If benchmarking shows blocking, isolate SQLite in one dedicated worker rather than introducing an unrelated database service. Avoid unbounded threads/tasks and unbounded buffers.

**MCP transport:** Streamable HTTP on loopback (`127.0.0.1` by default). Do not use `stdio` as the primary deployment mode: typical `stdio` MCP clients would launch a new process per agent, defeating the single-process memory goal. A legacy `stdio` shim is out of scope.

**Telegram transport:** `getUpdates` with a configurable ~45-second server-side long-poll timeout. This is *not* high-frequency polling and uses almost no CPU in idle. Telegram webhooks would avoid polling but require a reachable HTTPS endpoint or tunnel for a machine behind NAT; that complexity is deliberately excluded from the local MVP.

### Rust dependencies (preferred, verify current compatibility)

| Concern | Preferred choice | Notes |
|---|---|---|
| MCP | Official `rmcp` Rust SDK | Support modern `2026-07-28` semantics and compatible older clients if practical. |
| Async | `tokio` | Start with current-thread executor; only enable necessary features. |
| Local HTTP | `axum` or SDK's minimal Tower integration | Avoid duplicate HTTP-server stacks. |
| Telegram HTTP / TLS | `reqwest` + rustls | Use Bot API directly; no large Telegram framework required. |
| SQLite | `rusqlite` | One embedded file; small, indexed schema. |
| JSON, types | `serde`, `serde_json` | Only what the API requires. |
| Config | TOML + environment variables | Never place bot token in committed config. |
| Logs | `tracing` or small equivalent | Structured and quiet by default. |

Use a minimal dependency graph. Re-evaluate exact crate versions and `rmcp` feature flags at implementation time; avoid unnecessary SDK features (OAuth client, prompts, sampling, task engine, etc.).

---

## 4. Session identity and Telegram presentation

### 4.1 Session identity

**Do not confuse** MCP transport sessions/connections with an agent's conversation/thread identity. The current MCP protocol does not provide a universal host conversation title and thread ID in every tool call.

On first use, an agent explicitly registers:

- `external_session_id`: stable runtime/thread ID **if available**; otherwise an agent-generated, persisted UUID.
- `title`: human-readable session name, preferably taken from the host's real session title; explicit fallback if unavailable.
- The bridge returns an opaque `session_key` and an inbox resource URI. Subsequent tool calls carry `session_key` explicitly.

Persist mapping between `external_session_id`, `session_key`, and title. Registration is idempotent for the same registered identity. Different sessions with identical titles **must remain isolated**. No reliance on in-memory TCP connections, process IDs, or a model's display name as an identity.

**Title handling:** Normalize whitespace and strip line breaks/control characters; truncate to at most **50 user-visible Unicode grapheme clusters** (not bytes); provide a short fallback such as `Agent Session` when blank. Format as `[Title]`. Avoid injecting markup from the title; plain Telegram text is acceptable. Session renaming updates future messages, not historical Telegram messages or their routing.

### 4.2 Telegram output

Text example:

```text
[Refactor Portfolio API]
Completed the implementation. All tests passed.
```

Question example:

```text
[Fix ClickHouse Indexing]
Should I proceed with option A or option B?
```

Document example (Telegram document with caption):

```text
architecture-report.pdf
Caption:
[Refactor Portfolio API]
Architecture review and implementation report.
```

All bot-generated agent messages and documents are registered as `(telegram_chat_id, telegram_message_id) → session_key`. A Reply to a *file* is routed exactly as a Reply to text.

**Mandatory outbound text policy (agent-facing and server-enforced):**

- **Maximum 1,000 Unicode scalar values** per final outgoing Telegram text message (`Rust: message.chars().count()`), counting the entire formatted message: brackets, normalized session title, separator newline(s), and body. This is the explicit bridge-side definition of a *character*, not a UTF-8 byte or grapheme cluster.
- Apply the same **1,000-character final-caption limit** to documents (including their `[Session Title]` caption prefix), in addition to Telegram's separate, smaller native caption limit.
- Apply these checks to `telegram_send`, `telegram_ask`, `telegram_send_file` captions, and automatically generated service notices. Incoming **user** Replies are not subject to this *outbound* limit.
- Exactly 1,000 scalar values pass the **bridge** limit; 1,001 fail. Independently enforce Telegram's native limits. Unicode emoji and surrogate pairs can be counted differently across APIs: for document captions, additionally validate a conservative 1,024 UTF-16-code-unit bound (or a more accurate Telegram-verified equivalent). Thus a 1,000-scalar caption with many astral emojis may still be rejected to avoid a Telegram API failure. Test ASCII, Cyrillic, combining marks, ZWJ emoji, and supplementary-plane symbols.
- Validate **after** title normalization and final formatting, **before** invoking Telegram or registering a route.
- If too long, reject with a structured `TEXT_TOO_LONG` error containing `limit=1000`, `actual_characters`, and brief guidance to put the full material in a `.md`, `.txt`, or other suitable file and use `telegram_send_file` with a short caption. Do not send partial content.
- **Never automatically split overlong text into multiple Telegram messages, silently truncate it, or convert it into a file behind the agent's back.** The agent is responsible for preparing a file when necessary.
- Keep routine status updates and questions concise even when under the cap. Reserve file sending for content genuinely needing more space: detailed reports, code, logs, large outputs, etc.

The session title is a display label. **Routing is based on stored IDs, never on the bracketed title or last active agent.**

### 4.3 Reply semantics

- Use Telegram `reply_to_message.message_id` together with the current `chat.id` to locate the originating session.
- Multiple agents may interleave messages in the same chat.
- Multiple replies may target the same original bot message; preserve each independently, in arrival order.
- Reply to any outbound text message or document routes to the same originating session.
- Reject unapproved senders/chats. Do not guess a session for messages not explicitly replying to a known outbound message.
- A Reply to an expired or unknown route does not get reassigned to the most recent session; give one concise unroutable notice.
- Session titles may be identical or change; routes remain stable until their TTL expires.
- A normal standalone message not replying to a bot message is out of scope for agent routing in the MVP.

---

## 5. MCP interface

Use clear JSON Schemas, structured results, and stable error codes. The following is the intended **logical** contract; adopt the SDK's standard naming/serialization where appropriate. Keep tools limited and avoid implementing overlapping methods unless needed for correct semantics.

| Tool | Required behavior |
|---|---|
| `telegram_register` | Register/resume a session, return opaque key, effective title, inbox URI. |
| `telegram_send` | Send concise text (maximum 1,000 formatted characters) and register its Telegram message ID; reject overlong requests. |
| `telegram_send_file` | Send an existing local file as a Telegram document, with a concise title caption (maximum 1,000 formatted characters); register its message ID. |
| `telegram_ask` | Send a question (maximum 1,000 formatted characters); if wait fallback is enabled, wait up to its configured timeout for a routed Reply; otherwise return immediately. **Events remain enabled in either case.** |
| `telegram_inbox` | Retrieve/claim pending replies for this session on startup, resume, or after a notification. Not a periodic polling requirement. |
| `telegram_processing` | Mark beginning/end of processing and control temporary Telegram `typing`; beginning is *not* final acknowledgment. |
| `telegram_ack` | Explicitly confirm successful handling of a reply, allowing removal from the pending inbox. |

**Optional follow-on tool** (not required for basic user-text Reply on a file): `telegram_get_attachment`, to lazily fetch a user-sent photo/document attached to a Reply. See §9.

### 5.1 Suggested requests/results

`telegram_register`:

```json
{
  "external_session_id": "optional-stable-runtime-id",
  "title": "Fix ClickHouse Indexing"
}
```

```json
{
  "session_key": "opaque-session-handle",
  "title": "Fix ClickHouse Indexing",
  "inbox_uri": "telegram-bridge://sessions/opaque-session-handle/inbox"
}
```

`telegram_send`:

```json
{
  "session_key": "opaque-session-handle",
  "text": "Indexing completed successfully."
}
```

`telegram_send_file`:

```json
{
  "session_key": "opaque-session-handle",
  "path": "/home/user/project/reports/architecture-report.pdf",
  "caption": "Architecture review and implementation report."
}
```

`telegram_ask` (wait configured on server):

```json
{
  "session_key": "opaque-session-handle",
  "text": "Should I proceed with option A?"
}
```

Possible result on Reply:

```json
{
  "status": "reply_received",
  "outbound_message_id": 324,
  "reply": {
    "reply_id": "reply-uuid",
    "claim_token": "opaque-claim-token",
    "telegram_message_id": 325,
    "text": "Yes, proceed.",
    "received_at": "2026-10-29T12:35:00Z"
  }
}
```

Possible result on timeout:

```json
{
  "status": "wait_timeout",
  "outbound_message_id": 324,
  "message": "No reply arrived within the configured wait window; later replies remain valid."
}
```

`telegram_inbox` returns bounded, durable pending replies and delivery state (e.g. `reply_id`, text, optional attachment metadata, referenced outbound message ID and timestamp). When claiming a reply, it also returns a `claim_token`. `telegram_processing({session_key, reply_id, claim_token, active: true})` marks that the agent actually started processing; `active: false` stops `typing`. `telegram_ack({session_key, reply_id, claim_token})` marks successful completion. Repeated acknowledgments with the **same valid claim** must be idempotent; stale tokens cannot acknowledge a new claim.

### 5.2 Why explicit registration is necessary

The bridge cannot portably discover the active session name or UUID just because an MCP client connected. A standard MCP server must not pretend to know it. The agent integration policy must supply this information once per session (or resumption). An external stable runtime ID is optional, but without it, continuity depends on the agent preserving the generated session key.

### 5.3 File path semantics

`telegram_send_file` accepts **local filesystem paths** readable by the bridge process. This covers the intended single-machine setup. Paths on a remote agent machine are not magically accessible through MCP. Cross-host binary transfer is a future explicit feature, not a hidden fallback.

When text would exceed the 1,000-character limit, the agent should write a readable `.md` or `.txt` file (or an appropriate binary artifact) in an approved accessible directory and send it using this tool. Provide a short caption; the bridge validates the final caption's 1,000-character limit as described in §4.2.

---

## 6. Events, waiting, and delivery semantics

### 6.1 Standard MCP events: always available to capable clients

Use MCP resource subscriptions, not a proprietary push protocol:

- Resource URI per registered session: `telegram-bridge://sessions/{session_key}/inbox`.
- Support `resources/read` for inbox data. Expose the relevant resource template or discoverability path; keep authorization/session separation explicit.
- Advertise resource subscription support.
- For clients using MCP `2026-07-28`, honor `subscriptions/listen` with the `notifications.resourceSubscriptions` filter when supported, and publish `notifications/resources/updated` for the session inbox resource.
- For negotiated older MCP protocol revisions, implement the SDK-supported compatible mechanism **only if practical**, without changing the vendor-neutral core. Clearly document which clients/versions can subscribe.
- Notifications are **signals that a resource changed**, not the actual durable message queue. The subscriber reads the inbox and explicitly acknowledges processed replies.
- On reconnect, clients must read the inbox, because notifications themselves are not persistent/replayed by the protocol.
- A client must opt in to notifications; the bridge cannot force subscription nor force the client's LLM to act.
- Expose a **stable** `resources/templates/list` entry for `telegram-bridge://sessions/{session_key}/inbox` (and support direct reads of the concrete URI). Do not rely on dynamically adding registered session inboxes to `resources/list`: the 2026-07-28 resource-list contract must not vary by connection or as a side effect of previous requests; authorization may narrow visibility. Do not make subscription capability dependent on a particular agent vendor.
- Bound queues/buffers for subscribed clients. A slow or disconnected subscriber must not block Telegram update processing or cause unbounded RAM use. The SQLite inbox remains the recovery source of truth.

**Hard limitation:** Standard MCP does not require any host to translate `notifications/resources/updated` into a new LLM turn. Automatic wake-up must be documented as **client-dependent, not guaranteed by the MVP**.

### 6.2 Optional `wait` fallback, concurrently with events

Defaults:

- **MCP resource events are always supported and emitted to subscribed clients.** Do not expose an event-vs-wait selector or silently disable notifications while a waiter exists.
- `wait_fallback_enabled = true` (configurable).
- `wait_timeout_seconds = 600` (configurable).

When an agent needs an immediate answer, it may call `telegram_ask`:

1. Send/persist outbound question route.
2. If waiting is enabled, suspend **only this tool call** asynchronously until a matching Reply arrives or the wait deadline passes.
3. Regardless of the waiter, incoming Reply is persisted **and the resource-update event is emitted** to all authorized subscribers.
4. The matching waiter may receive the same reply, using the common inbox claim/ack system (below).
5. A timeout returns `wait_timeout` **without terminating the agent, expiring the route, deleting a reply, or sending a Telegram timeout warning**.
6. A later Reply remains routable until the underlying route's 30-day TTL expires.

This is not an **event OR wait** switch. Both run in parallel.

**Important impossibility:** The bridge cannot detect that an idle model ignored an event and then somehow make the idle model call `wait`. The reliable general-purpose fallback must be *armed while the agent is executing* (for example by its `telegram_ask` call). A server-side deadline/notification never causes a model to run a tool automatically.

**Client timeout caveat:** Many MCP hosts impose a shorter tool-invocation timeout than 600 seconds. Document how to increase per-tool timeout if the host permits it; if not, fall back to nonblocking `telegram_send` plus events and eventual inbox recovery. Do not use protocol-specific runtime adapters merely to evade this limitation.

### 6.3 Durable message states and duplicate prevention

A single Reply can be visible through an event-triggered inbox read and an active `telegram_ask`. **Do not** treat those as independent messages. Use one `reply_id`, one durable record, and a transactionally enforced claim/processing/ack protocol:

```text
Telegram update accepted
          |
        pending  --(event notification; no state change)
          |
          +--(inbox or wait atomically claims it)--> claimed
                                                       |
                         telegram_processing(true) --> processing
                                                       |
                          telegram_ack(success) -----> completed/removed
```

Rules:

- **Persist first, then notify** both event subscribers and async waiters.
- `resources/read` returns a **read-only** inbox snapshot; it must never acquire a claim. `telegram_inbox` (with claim semantics) and a waiting `telegram_ask` use the same atomic claim path. Only one active claim is granted to a reply at a time.
- Each acquired claim returns an unpredictable `claim_token` (or equivalent fencing/version value) together with the stable `reply_id`. The agent must pass the token to `telegram_processing` and `telegram_ack`; reject stale or mismatched tokens so an expired claimant cannot complete another claimant's work.
- If a claim is abandoned **before** processing starts, it becomes claimable again after a short, bounded claim lease; this does **not** time out the agent's work. Never expire a processing claim merely because the user-facing 10-minute typing cap elapsed.
- Once processing is explicitly started, keep the reply recoverable in SQLite until a final `telegram_ack` or 30-day retention expiry. Do not automatically redeliver it to other concurrent consumers while marked `processing`.
- On explicit session resume, allow retrieval/reconciliation of previously `processing` but unacknowledged replies so a crashed agent can decide whether to resume. Always include the same `reply_id` for idempotency.
- A canceled `telegram_ask`, disconnected client, expired wait, or server restart cannot delete an unacknowledged reply.
- Do not mark a reply completed merely when `resources/read`, `telegram_inbox`, or the waiting tool returned it.
- `telegram_ack` is idempotent for an identical valid claim; acknowledgment is per reply ID, session, and claim token, never a blanket clear-inbox operation.
- Exactly-once **processing** across crashes cannot be guaranteed by a generic MCP server; target durable, deduplicated, at-least-once **delivery** and agent-side idempotent handling.

Keep claims very lightweight; one connection-local async notification mechanism (e.g. Tokio `Notify`/`watch`) per active waiter is sufficient. No periodic inbox polling and no background thread per session.

### 6.4 Reply acceptance during inactivity

If no live subscription and no pending waiter exists, the bridge **still persists** the Reply in its session inbox. The user may receive a single notice after the processing-start confirmation grace period (see §8). The reply is obtainable when that agent later resumes and calls `telegram_inbox`.

The MVP must not claim to have delivered a reply **to the LLM** if it only stored it, notified an MCP client, or wrote bytes to an HTTP response.

---

## 7. Telegram `typing` policy

The Telegram `typing` status is bot-wide within a chat; it cannot identify which agent session is active. Its intended meaning is "an agent confirmed it has started working on the received input," not "the bridge received a Telegram update."

- Agent calls `telegram_processing(active=true, reply_id=...)` *when it starts processing* the reply.
- Bridge sends `sendChatAction(action="typing")` immediately and refreshes roughly once every **4 seconds**, only while activity is enabled.
- Automatically **stop refreshing after 600 seconds** (10 minutes) from the **first** processing-start signal for that Reply, regardless of whether the agent is still running. Repeated `active=true` calls for the same Reply must not reset this limit.
- `telegram_processing(active=false)` stops further refreshes immediately. On agent message send, clear this session's activity if appropriate.
- After expiration, **do not interrupt the agent**, change message status, fail a wait, or send a service notification. The agent will eventually send a normal result if it chooses to.
- Track activity only in memory. All activity disappears on process restart; this is acceptable.
- Use a single coordinated timer/loop for active chats; no perpetual four-second wakeup when there is no active typing.
- Overlapping agent sessions share the same visible chat action. Merge active leases to at most one `sendChatAction` call per refresh interval per chat; never try to show which session is typing.
- Telegram itself shows a chat action for **at most about five seconds** and may clear it on a new bot message; do not attempt to create a permanent status.

**Crucial distinction:** `typing_max_duration_seconds = 600` is not `wait_timeout_seconds = 600`. They are independent settings and neither controls the lifetime of agent execution.

---

## 8. Minimal service-notification policy

The bridge is not a monitoring system for agent processes. It has no reliable knowledge of whether a model is sleeping, thinking, blocked, or dead. Notifications should be factual and minimal.

| Trigger | Behavior |
|---|---|
| Normal send / Reply / claim / acknowledgment | Silent. |
| Reply saved, but not confirmed as **started processing** within a configurable 60-second grace period | At most one factual notification for that Reply. Do not notify merely because final acknowledgment is pending while processing is already underway. |
| Reply to unknown/expired route | One concise unroutable notification; do not guess agent. |
| `telegram_ask` exceeds its wait timeout | Return `wait_timeout` to caller; **no Telegram notice**. |
| `typing` reaches 10-minute cap | Stop typing; **no Telegram notice**. |
| Agent runs for hours | Do nothing unless it sends an update. |
| Bridge reconnects after transient Telegram network failure | Retry quietly; log locally. |

Suggested text when the session is known:

```text
[Fix ClickHouse Indexing]
Reply saved, but the agent has not confirmed processing it yet.
```

Suggested text for unknown routes:

```text
Could not identify the target session for this Reply. Please reply to a newer message from that agent.
```

Do not announce "agent is asleep," "delivery failed," or "agent stopped" unless a future runtime adapter can actually establish those facts. Measure the 60-second grace period from durable Reply receipt, and emit an unconfirmed-processing warning **only if no `telegram_processing(active=true)` has been recorded for that Reply**. Once processing has started, neither prolonged work nor delayed final `telegram_ack` should trigger that notice. Persist `notice_sent` so a restart does not send the same warning repeatedly. Avoid notices for unsupported bot commands or unrelated chats; ignore them quietly.

Service messages should not generate self-routed Reply loops. Either leave them outside normal message route registration or explicitly give them a safe, documented routing behavior.

---

## 9. File handling

### 9.1 Agent → Telegram (MVP requirement)

- Use Telegram `sendDocument` for arbitrary file types to preserve files in their original form.
- Caption starts with `[Session Title]` and includes an optional **short** agent-provided description; validate its entire formatted length against the 1,000-character cap.
- Store the returned Telegram message ID in the route table. Reply to that document must route to its originating session.
- Read and stream from the local path rather than loading the entire file into RAM.
- Validate that the path exists, resolves within configured allowed root directories, is a regular file, and is readable. Avoid symlink traversal and path-escaping TOCTOU problems as far as practical.
- For the cloud Telegram Bot API, enforce the currently documented **50 MB outbound document limit** (check current API behavior; do not assume it is permanent).
- Do not copy outbound files into SQLite or persistent bridge storage. The user has the actual file in Telegram; the source remains wherever the agent created it.
- If upload fails definitively, return a clear error; do not record an unsent Telegram message route. If the HTTP response is lost after Telegram may have accepted the upload, report an **uncertain delivery** rather than blindly retrying and risk duplicate files.
- Sending multiple files means multiple Telegram documents, each titled and registered separately.

### 9.2 User → agent attachments (design for later, lightweight optional implementation)

A user may Reply to an agent's message with text, a document, or a photo. For text, support is mandatory. Preserve inbound attachment metadata (`file_id`, name, MIME type, size when present) alongside the Reply; do **not** immediately download the attachment.

If implemented, `telegram_get_attachment({session_key, reply_id, destination})` downloads it lazily through `getFile`, with size and path limits, into a restricted temporary/approved destination. Do not persist binary contents in SQLite. Do not load whole files in memory. Clean up only files owned by the bridge, not files owned by the user/agent. Sanitize filenames.

Telegram's cloud Bot API presently documents **20 MB** as the `getFile` download limit. If an attachment exceeds that or cannot be fetched, return a clear unsupported/too-large error and retain its metadata as appropriate; do not claim successful delivery of its bytes.

Because this feature adds filesystem and security surface, the optional download tool may be implemented in a second phase without blocking the text Reply + outbound document MVP, but its storage model should not need a schema rewrite.

### 9.3 Telegram size limits

As of this specification, Telegram Bot API documents:

- `sendMessage`: text 1–4096 characters after entities parsing.
- `sendDocument`: cloud upload up to 50 MB.
- Document caption: up to 1024 characters after entities parsing (verify the platform's Unicode counting at implementation time; the bridge separately checks its own Unicode-scalar limit).
- `getFile` download: up to 20 MB.

**The bridge intentionally imposes a 1,000-Unicode-scalar maximum on each outgoing text message and document caption, inclusive of its session prefix.** Telegram's larger text-message limit must not be treated as permission to send long chat messages; Telegram's separate caption limit may be stricter for supplementary Unicode. Do not auto-split or auto-truncate; report `TEXT_TOO_LONG` (or a distinct platform-length error where appropriate) and direct the agent to file sending. The 1,000-scalar bridge cap is an MVP policy constant, not an additional configuration setting. Enforce both bridge and platform limits and recheck the current official API documentation while implementing.

---

## 10. Minimal durable SQLite design

Store only what the bridge must know to route, recover, and deduplicate. The actual conversation is visible in Telegram, so storing outbound text bodies, full update payloads, or outbound file bytes is unnecessary.

### 10.1 Proposed logical tables

```text
sessions
  session_key PK (opaque random handle)
  external_session_id UNIQUE NULLABLE
  title
  created_at
  last_activity_at
  expires_at

message_routes
  telegram_chat_id
  telegram_message_id
  session_key FK
  kind (text | document | service-if-routed)
  created_at
  expires_at
  PK (telegram_chat_id, telegram_message_id)

replies
  reply_id PK
  telegram_update_id UNIQUE
  telegram_chat_id
  telegram_message_id
  reply_to_bot_message_id
  session_key FK
  text_or_caption NULLABLE
  attachment_metadata_json NULLABLE
  state (pending | claimed | processing)
  claimed_at NULLABLE
  claim_token NULLABLE (rotate on every new claim)
  processing_started_at NULLABLE
  created_at
  expires_at
  notice_sent

telegram_cursor
  singleton_id PK
  last_durably_processed_update_id
```

Schema may be simpler where justified. Preserve indexes for `(telegram_chat_id, telegram_message_id)`, `(session_key, state, created_at)`, and retention cleanup. Do not make session route references cascade-delete unacknowledged replies unexpectedly; explicitly define cleanup order and behavior.

### 10.2 TTL defaults

| Record | Default TTL policy |
|---|---|
| `message_routes` | 30 days **from original bot message creation**, independent of session activity. |
| `sessions` | 30 days since last bridge activity; each successful send/received Reply/registration refreshes it. |
| Unacknowledged `replies` | 30 days **from receipt**, including processing-but-not-finally-acked records. |
| Acknowledged/completed replies | Remove promptly (keep only any minimal dedupe state needed for correctness). |
| Outbound text/file body | Never persisted. |
| `typing` state | RAM only, maximum 10 minutes. |
| `telegram_cursor` | Durable indefinitely as one tiny record. |

Consequences must be documented: Reply to a bot message after its route expires may be **unroutable**, even though the message is still visible in Telegram. A pending reply that remains unprocessed for 30 days may expire. This is an explicit bounded-storage policy; do not promise indefinite delivery.

### 10.3 Update ingestion and exactly-once persistence

- Accept only allowlisted Telegram chats/users.
- Process updates in `update_id` order with one active `getUpdates` consumer.
- Persist an accepted Reply and advance the local processed-update cursor **atomically** before requesting a new Telegram offset that confirms it.
- After restart, resume from the last durable cursor. Telegram may redeliver already processed updates; unique update IDs + transactionally stored cursor prevent duplicate replies.
- Do **not** advance the offset beyond an update until its processing is durably resolved (accepted, explicitly ignored, or safely classified). On transient failure, retry without dropping subsequent data.
- Long polling and `getUpdates` offsets do not create a general chat history API. Telegram retains unconsumed updates **for at most 24 hours**; if the bridge is offline longer, some updates may be irretrievably missed. Document this clearly.

### 10.4 Cleanup and disk bounds

- Run cleanup once at startup and roughly **once every 24 hours** thereafter; do not allocate one timer per record.
- Delete expired routes/replies/sessions in bounded batches; preserve referential consistency.
- Configure SQLite for low overhead, a reasonable busy timeout, and small bounded caches. Choose rollback journal versus WAL based on measured concurrency and correctness; do not add Redis or another persistence layer.
- Consider SQLite `auto_vacuum = INCREMENTAL` **when initializing the database**, and occasional bounded `incremental_vacuum`/checkpoint operations; note that deleting rows alone may not reduce on-disk file size.
- Keep logs bounded through systemd/journald or configurable rotation and avoid logging full user messages or secrets by default.
- Enforce a configurable **maximum retained inbox item count and maximum payload size**; if exceeded, report capacity pressure rather than silently deleting unacknowledged messages earlier than their promised TTL. Choose conservative defaults after tests.

---

## 11. Failure handling and limits of guarantees

| Scenario | Required response |
|---|---|
| MCP client has no subscriptions | Sending works; registered replies persist; wait fallback (if armed) and inbox recovery remain available. |
| MCP client receives notification but does not create LLM turn | Reply remains in inbox; at most one delayed notice if processing never starts; no claim of automatic wake-up. |
| MCP client also has active `telegram_ask` | Event still emitted; one durable reply; common claim path prevents two independent claims. |
| 10-minute wait expires | Only the tool's wait completes with timeout; no agent interruption, message deletion, or Telegram timeout notice. |
| 10-minute typing duration expires | Stop refreshing; no agent interruption and no notification. |
| Agent works for several hours | No lifecycle heartbeat required; reply remains pending/processing until final ack or retention expiry. |
| Agent/MCP client disconnects mid-wait | Cancel in-memory waiter; keep durable route/replies; release pre-processing claim safely. |
| Bridge restarts | Existing routes/inbox/cursor survive; `typing` and waiters do not; resume long polling and recover inbox. |
| Telegram transient network error / rate limit | Bounded exponential backoff with jitter, honor Telegram `retry_after` if present, remain quiet to user. |
| Telegram accepts send but response is lost | Cannot guarantee exactly-once outgoing send; return uncertainty and avoid unsafe automatic repeat. |
| Telegram Reply comes after route TTL | Do not guess recipient; optionally send one unroutable notice. |
| Multiple same-named sessions | Session keys isolate all routes. |
| Invalid sender/chat | Discard or ignore without notifying unauthorized user; do not leak session details. |
| Incoming update backlog >24 hours while bridge offline | State limitation plainly in documentation; cannot reconstruct from Telegram automatically. |
| Outgoing text/question/caption exceeds 1,000 formatted Unicode characters | No Telegram request or route registration; return `TEXT_TOO_LONG` with count and suggest file delivery. |
| Missing or unreadable file / exceeds limit | No upload attempt; return a structured error. |
| No `external_session_id` available | Accept registered UUID/handle; note that agent must preserve it to resume same session. |
| Expired session with pending responses | Retention cleanup must follow explicit policy and not silently orphan still-valid replies. |

**Idempotency model:** Aim for at-least-once delivery with deduplicated `reply_id` and explicit completion acknowledgments, not unconditional exactly-once LLM actions. The model or client might crash after performing side effects but before acknowledging; consumers must treat repeated `reply_id` as potentially already processed.

---

## 12. Security requirements

- Bind MCP HTTP to `127.0.0.1` (or an explicitly configured loopback address) by default. Never default to `0.0.0.0`.
- Protect the local MCP endpoint from other local/untrusted callers: at minimum validate expected host/origin and offer a bearer token; use a generated token file with restrictive permissions if clients support custom headers. Verify current MCP Streamable HTTP security guidance, including DNS rebinding and Origin checks.
- Keep Telegram bot token in `TELEGRAM_BOT_TOKEN` (or another protected secret source), never checked-in TOML or logs.
- Require an explicit allowlist of Telegram user IDs and chat IDs. No public bot use by default.
- Enforce file-root allowlists for agent-specified paths; do not allow reading secrets outside approved directories or following symlinks outside the root.
- Treat all Telegram messages, captions, filenames, and file contents as **untrusted input** from the perspective of the agent. The bridge transports data; it does not execute shell commands from replies.
- Use authenticated, opaque, unpredictable session handles; prevent one session from accidentally consuming another session's replies. A local shared bearer token alone does not create strong per-agent isolation; document the assumed trusted-local-client model.
- Limit request sizes, incoming Telegram payload parsing, message lengths (including the stricter 1,000-character outbound cap), attachment sizes, and concurrency. Enforce bounds instead of buffering unlimited data.
- Do not leak full file paths, session keys, or sensitive message bodies into user-facing service notifications.
- Prefer plain Telegram text for escaping safety; if formatting is added, escape title/body content rigorously.

---

## 13. Configuration

Suggested `config.toml` (all settings represent defaults and may be simplified without losing requirements):

```toml
[server]
listen = "127.0.0.1:8765"
mcp_path = "/mcp"
# Optional, recommended when client supports authorization headers:
# auth_token_env = "TELEGRAM_MCP_AUTH_TOKEN"

[telegram]
bot_token_env = "TELEGRAM_BOT_TOKEN"
chat_id = 123456789
allowed_user_ids = [123456789]
transport = "long_poll"
long_poll_timeout_seconds = 45

[delivery]
# Resource-change events remain enabled for every subscribed MCP client.
wait_fallback_enabled = true
wait_timeout_seconds = 600
processing_start_grace_seconds = 60

[activity]
typing_enabled = true
typing_refresh_seconds = 4
typing_max_duration_seconds = 600

[storage]
path = "~/.local/state/telegram-mcp-bridge/bridge.sqlite"
ttl_days = 30
cleanup_interval_hours = 24

[notifications]
notify_unconfirmed_processing = true
notify_unroutable = true

[files]
# Explicitly configure a directory readable by the local bridge process.
allowed_roots = ["/home/your-user/agent-exports"]
max_outbound_mb = 50
# Incoming download tool can be disabled in the MVP.
incoming_downloads_enabled = false
```

**Security defaults:** Make users explicitly specify approved Telegram ID(s) and allowed file roots. Resolve `~` paths safely rather than relying on shell expansion in TOML. Document a safe `systemd --user` environment-file option for secrets. Do not silently delete existing Telegram webhooks or pending updates during setup: if a webhook is configured, explain the conflict and provide an explicit safe `deleteWebhook(drop_pending_updates=false)` setup step.

**Host-specific timeouts:** The client may need an MCP `tools/call` timeout greater than `wait_timeout_seconds`; this is a client setting and cannot be forced by the server. Do not hard-code Codex, Claude, or another provider in the application's configuration.

---

## 14. Minimal agent operating policy (for inclusion in AGENTS.md / equivalent)

```markdown
# Telegram MCP Interaction Policy

- Register or resume the current agent session before using Telegram tools.
- Provide a stable session ID and accurate title when available; the bridge will
  normalize the title and prefix all messages/files with `[Session Title]`.
- Keep Telegram chat messages brief. Every outgoing text or question MUST fit within
  1,000 Unicode characters **including** the `[Session Title]` header and newline.
  Do not split a long report into a series of Telegram messages.
- Use `telegram_send` for short status updates and `telegram_ask` for concise questions.
  If a report, code sample, log, or explanation would exceed the limit, write it
  to a readable `.md`, `.txt`, or other appropriate file in an allowed directory
  and use `telegram_send_file` with a short session-labeled caption instead.
  Never truncate important content to fit the text limit.
- To request a user decision, use `telegram_ask` when bounded waiting is appropriate.
  MCP events remain active even while the tool waits.
- On session start/resume, or after an inbox-change notification, inspect
  `telegram_inbox`. Do not perform periodic inbox polling.
- Treat every reply as identified by `reply_id`. Do not execute the same reply twice.
- As soon as you begin working on a received reply, call
  `telegram_processing(active=true, reply_id=..., claim_token=...)`.
- Call `telegram_processing(active=false, reply_id=..., claim_token=...)` when that work ends.
- Call `telegram_ack(reply_id=..., claim_token=...)` after you have successfully handled the reply.
  Preserve `reply_id` and the current claim token throughout processing; do not acknowledge stale claims.
- If a waiting call times out, do not interpret silence as permission. Continue
  other safe work if possible. A later Reply remains valid while its route exists.
- Do not manually prefix session titles or send service warning messages;
  the bridge handles those automatically.
- Do not assume an MCP resource notification will automatically wake an idle LLM;
  host support varies. If the host cannot do this, use the optional wait mechanism
  while the session is running, or recover replies on the next session resume.
```

This is a **policy**, not a replacement for protocol-level support. Agents may not perfectly follow it; the bridge should remain safe if activity/ack calls are missed.

---

## 15. Implementation phases

Keep phases small and independently testable; do **not** treat provider-specific wake support as a prerequisite.

### Phase 1 — Skeleton and Telegram routing

1. Build native Rust daemon with configuration, restricted local MCP transport, Telegram long poll, and SQLite.
2. Implement session registration and title formatting.
3. Implement `telegram_send` and `telegram_send_file`, with stored message-to-session routes and the mandatory 1,000-character outgoing text/caption limit (no auto-chunking).
4. Parse allowlisted Telegram replies, persist them transactionally, and route by `chat_id + reply_to_message.message_id`.
5. Verify two interleaved agents and Replies to both text and documents.

### Phase 2 — Standard MCP event and durable inbox

1. Expose per-session inbox resource and `resources/read`.
2. Implement modern MCP subscription delivery and resource-update notifications, with compatible behavior for supported protocol versions.
3. Implement `telegram_inbox`, atomic claim, processing state, and `telegram_ack`.
4. Ensure reconnection/restart recovery, duplicate update suppression, and concurrent consumer safety.

### Phase 3 — Wait fallback, typing, notices

1. Add optional `telegram_ask` wait behavior with server-configured deadline.
2. Confirm notifications **still fire with an active waiter** and a Reply is not processed twice.
3. Add `telegram_processing` and 10-minute capped typing refresh.
4. Add a 60-second delayed once-per-reply **unconfirmed processing** notice (never fire when processing was confirmed), plus unknown-route notice.
5. Implement cleanup, bounded resource consumption, and safe file sending.

### Phase 4 — Release quality

1. Integration tests against mocked Telegram HTTP API and MCP clients.
2. Document and validate local setup, secret handling, Linux `systemd --user` example, reconnection, and supported client capabilities.
3. Benchmark idle RSS/CPU, outbound file upload RSS, startup time, and binary size.
4. Optionally add lazy incoming attachment download **only if it does not compromise the lean MVP**.

---

## 16. Acceptance criteria

### Functional acceptance

- [ ] A single Rust process supports at least **three concurrent agent sessions** on one Telegram bot.
- [ ] Every agent-originated text, question, and document is prefixed/captioned with a `[Title]` of at most **50 visible characters**.
- [ ] Every outgoing text/question/document caption is **at most 1,000 Unicode scalar values in its final formatted form**, including title prefix and newline; exactly 1,000 ASCII scalars are accepted, 1,001 are rejected, and document captions also respect Telegram's native length limit with supplementary Unicode characters.
- [ ] The bridge never splits or truncates an oversized message, or silently generates a file: it returns `TEXT_TOO_LONG` with an accurate count and guidance to use `telegram_send_file`.
- [ ] Agents are instructed to keep chat replies concise and deliver substantial information in a file with a short caption; user-originated Reply text is not restricted by this outbound policy.
- [ ] Two sessions with the **same title** still route replies independently.
- [ ] Reply to an old, but non-expired, text message or file routes to its original session regardless of subsequent agent activity.
- [ ] Replies to unknown/expired messages are **never** delivered to the wrong session.
- [ ] Different replies to the same bot message are represented by distinct reply IDs.
- [ ] MCP subscription clients can receive a resource-update event when a Reply arrives; `resources/read`/inbox can retrieve it.
- [ ] With `wait_fallback_enabled=true`, an outstanding `telegram_ask` can receive the Reply without any agent polling.
- [ ] With an outstanding wait **and** an active resource subscription, the bridge emits the event and wakes the waiter without creating duplicate independent reply records.
- [ ] With `wait_fallback_enabled=false`, notification and inbox delivery continue to function.
- [ ] A timed-out `telegram_ask` leaves subsequent Replies valid; it does not cancel/kill the agent or delete a route.
- [ ] Offline or disconnected agents' replies remain in SQLite for recovery (within TTL).
- [ ] Claims, processing starts, and final acknowledgments are idempotent and concurrency safe; stale `claim_token` values cannot start processing or acknowledge a newer claim.
- [ ] On confirmed start of handling, Telegram `typing` appears, is refreshed while active, and stops after a configurable **10-minute maximum** even if the agent works longer.
- [ ] No `typing` refresh and no periodic agent inbox checks occur in idle.
- [ ] User receives at most **one** factual notice per Reply whose **processing start remains unconfirmed** beyond the configured grace period, and one for an unroutable Reply. A long-running, confirmed-processing agent triggers no warning.
- [ ] Telegram document upload is streamed, respects size/path restrictions, includes the session caption, and is Reply-routable.

### Persistence and recovery

- [ ] Session/route/reply TTLs default to 30 days with documented expiry semantics.
- [ ] No local copy of outbound chat history or outbound file contents is retained.
- [ ] Crash after SQLite commit but before next Telegram poll does not duplicate an accepted reply on restart.
- [ ] Crash or cancellation of a wait does not discard its unacknowledged Reply.
- [ ] Resource subscription reconnection recovers missed state from SQLite, without pretending that MCP notifications are replayed.
- [ ] Cleanup removes expired data without unbounded timers, accidental cascade loss, or uncontrolled disk growth.
- [ ] The README explains Telegram's at-most-24-hour retention of unconsumed bot updates.

### Performance and operational acceptance

- [ ] One daemon; **no Node.js**, broker, external DB, Docker requirement, agent process management, or background process per agent.
- [ ] The default local setup does not require a publicly reachable HTTPS endpoint, tunnel, or webhook.
- [ ] Idle CPU is effectively zero except normal long-poll/reconnect/timer overhead.
- [ ] Establish and document a **best-effort idle RSS target below ~30 MB** on a specified Linux x86_64 release build, not a fictitious cross-platform guarantee. Record measured values and explain deviations.
- [ ] Sending a near-limit file does not load its whole contents in memory; record peak RSS.
- [ ] Build a stripped release binary where practical; report binary size and dependency profile.
- [ ] All secrets and user file roots are protected; MCP endpoint is loopback-only by default.
- [ ] Include unit and integration tests for title truncation, Unicode text length at 1,000/1,001 scalar values (including header, Cyrillic, ZWJ emoji, surrogate pairs, and multiline text), native Telegram caption bounds, oversized question/caption rejection without partial sends, interleaved sessions, reply routing, event/wait claim races and stale fencing tokens, missing processing acknowledgment vs long-running confirmed processing, TTL, missed acks, restart, large-file streaming, unauthorized updates, and invalid paths.

---

## 17. Definition of Done / expected deliverables

Deliver:

1. Working Rust source with a clean, idiomatic, minimal crate layout.
2. `Cargo.lock`, reproducible release build instructions, and any required config sample.
3. SQLite migration/init logic, cleanup behavior, and restart-safe Telegram cursor handling.
4. MCP tools/resources/subscription implementation, with current protocol features tested.
5. Telegram client with one long poll, strict 1,000-character outgoing text/caption validation, and safe document handling.
6. Unit + integration tests (mock Telegram API; avoid sending real user messages in CI).
7. README covering setup, BotFather token, Telegram chat/user allowlist, MCP URL, secrets, permissions, agent registration, wait-timeout client caveat, and service notices.
8. A short `AGENTS.md` or copyable operating-policy snippet from §14.
9. Example `systemd --user` unit and protected environment-file approach (optional to run, not required to build).
10. A concise benchmark summary with build target, RSS, idle CPU, peak file-upload RSS, and binary size.

Prefer **small, tested modules** over a large architecture framework. If a requirement conflicts with an actual client/API limitation, document the conflict explicitly and preserve the rest of the behavior rather than shipping a fake guarantee.

---

## 18. External specifications / references

Implementation must verify current upstream versions and behavior rather than treating the examples above as copied, compile-ready SDK code.

- MCP specification, resources and subscriptions: <https://modelcontextprotocol.io/specification/2026-07-28/server/resources>
- MCP transport-neutral `subscriptions/listen`: <https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/subscriptions>
- MCP specification, Streamable HTTP transports: <https://modelcontextprotocol.io/specification/2026-07-28/basic/transports>
- MCP 2026-07-28 changes and compatibility: <https://blog.modelcontextprotocol.io/posts/2026-07-28/>
- Official MCP Rust SDK (`rmcp`): <https://github.com/modelcontextprotocol/rust-sdk>
- Telegram Bot API (`getUpdates`, `sendMessage`, `sendDocument`, `sendChatAction`, `getFile`): <https://core.telegram.org/bots/api>
- SQLite auto-vacuum and storage behavior: <https://www.sqlite.org/pragma.html#pragma_auto_vacuum>

**Final architectural rule:** Build a **portable, event-capable MCP bridge** with an **optional pre-armed wait fallback**, backed by a durable 30-day inbox. Do not promise to wake arbitrary LLM hosts, do not poll agents, and do not introduce provider-specific runtime adapters into the MVP.

---

## 19. Requirements traceability (implementation self-check)

| User requirement | Primary sections |
|---|---|
| One compact Rust native service, low idle RAM/CPU, local-only | §§1–3, 13, 16 |
| Universal MCP, one Telegram bot for multiple agents, no vendor adapters | §§1–3, 5–6 |
| `[Session Title]` up to 50 visible characters on text and files | §4, §9 |
| Maximum 1,000 outgoing text characters including header; use files for details | §§4.2, 5, 9, 14, 16 |
| Reply to any agent text/document routes to original session | §§4.3, 6, 9–10 |
| MCP resource notifications remain active with or without a pending wait | §§5–6, 16 |
| Optional bounded async `wait`, no interruption of agent when it expires | §§6.2, 11, 13–14 |
| No mandatory agent-side periodic inbox polling; no promise of universal auto-wake | §§2, 6, 14 |
| `typing` only after agent confirms handling, for at most 10 minutes | §7, §13 |
| Factual minimal notices for unconfirmed processing/unroutable Replies | §8, §13 |
| Durable 30-day TTL, cleanup and restart-safe routing | §§6, 10–11 |
| File delivery without local copies and bounded memory | §9, §12 |
| Secure local operation, tests, deliverables and success criteria | §§12, 15–17 |

This checklist maps the **agreed scope**, not a guarantee that arbitrary MCP hosts will wake an idle LLM. Implementations must report tested client capabilities separately.
