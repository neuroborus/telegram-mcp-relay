# Local Setup

This guide describes the implemented daemon. The [specification](product/TECHNICAL_SPECIFICATION.md)
also contains proposed settings and illustrative results; use the actual
[example TOML](../config/relay.example.toml), [configuration reference](CONFIGURATION.md) and
discovered tool schemas. Runtime measurements, host unit verification and named-client/live
Telegram interoperability remain unverified. Setup instructions are not acceptance evidence.

## Build and prepare private directories

Use Linux with kernel 5.6+ and permitted `openat2` for outbound files, Rust 1.89+ with Cargo,
and a native C compiler/build tools for bundled SQLite. Build from an identified checkout,
preserving `Cargo.lock`. Dependency acquisition needs registry access or a previously populated
cache. No Node.js, Docker, external database or broker is required at runtime.

```bash
cargo build --locked --release --bin telegram-mcp-relay
```

The release profile strips the binary and uses thin LTO. This command is a build instruction,
not a claim that a release build or measurement passed. For local operation, prepare these
paths as your ordinary user (never run the daemon as root):

```bash
umask 077
install -d -m 700 "$HOME/.config/telegram-mcp-relay" \
  "$HOME/.local/state/telegram-mcp-relay" "$HOME/agent-exports"
```

Use real directories with trusted ancestors, without symlinks. Inspect existing ownership and
permissions first; the daemon never repairs them. Its database parent must be owned and private
(0700); the database and existing sidecars must be regular, single-link, owned, private files
(0600). Keep secrets and the database outside exports. Leave disk headroom for the active
SQLite rollback journal in addition to the default 64 MiB main-file cap; see
[physical limits](ARCHITECTURE.md#physical-disk-maintenance).

## Create a bot and select IDs

1. In Telegram, open the official [BotFather](https://t.me/BotFather), use `/newbot`, choose
   the bot's display name and username, and save its token privately. Revoke a leaked token
   through BotFather. The token authorizes the bot; never put it in TOML, a repository,
   screenshots, shared shell history or diagnostics.
2. Start a private conversation with your bot. For group use, add it only to the intended
   group and ensure it receives Replies to its messages; retain Telegram's privacy settings
   unless your use case requires a deliberate change. Plain unthreaded messages do not select
   an agent: users must use Telegram's **Reply** action on the relevant bot text or document.
3. Obtain the numeric chat ID and sender user ID from your own bot's
   [`getUpdates`](https://core.telegram.org/bots/api#getupdates) response: `message.chat.id`
   and `message.from.id`. A username, phone number or bot ID is not a substitute. Preserve
   the sign of group chat IDs. Use only IDs you have verified; the example's 101/202 are synthetic.

For setup inspection, stop every existing consumer for this bot first. Use a private HTTPS
client that protects the token-bearing Bot API URL (no verbose/request logging or token in
process arguments). Check the API response's `ok` field. Inspect
[`getWebhookInfo`](https://core.telegram.org/bots/api#getwebhookinfo) before `getUpdates`.
If `url` is nonempty, webhooks conflict with long polling: decide explicitly to retire the
previous webhook integration, then perform this operator-only request:

```text
POST https://api.telegram.org/bot<BOT_TOKEN>/deleteWebhook
Content-Type: application/json

{"drop_pending_updates":false}
```

Verify `getWebhookInfo.url` is empty afterward. Do not use `drop_pending_updates=true`.
For ID inspection, call `getUpdates` without an offset (and without negative offsets or
advancing to a higher offset); a higher offset confirms and discards earlier updates.
Protect the response too: it contains private messages. Stop setup inspection before starting
the relay. Run exactly one daemon/consumer per bot and database, shared by all agent sessions.
The relay never deletes a webhook or pending updates automatically. A 409/conflict requires
operator investigation, not another competing poller.

See the [official API](https://core.telegram.org/bots/api) and
[documented API review](ARCHITECTURE.md#telegram-http-client); live behavior is unverified.

## Configure paths and secrets

Copy the templates only when the destination does not already exist; preserve an existing
installation's settings and secrets. From the checkout:

```bash
cp -n config/relay.example.toml "$HOME/.config/telegram-mcp-relay/config.toml"
cp -n deploy/relay.env.example "$HOME/.config/telegram-mcp-relay/relay.env"
chmod 600 "$HOME/.config/telegram-mcp-relay/config.toml" \
  "$HOME/.config/telegram-mcp-relay/relay.env"
```

Edit the private TOML: set `telegram.chat_id`, include it in `telegram.allowed_chat_ids`,
set `telegram.allowed_user_ids` to the authorized human sender IDs, and replace
`files.allowed_roots` with your actual export directory. Authorization requires both a permitted
chat and sender. Uncomment `server.auth_token_env = "TELEGRAM_MCP_AUTH_TOKEN"` to enable bearer
authentication. All agents use the same configured destination; there is no per-send chat override.

Edit the private environment file: uncomment the two assignments, replace the bot placeholder,
and generate a separate random bearer value of at least 32 ASCII bearer characters. For example,
`python3 -c 'import secrets; print(secrets.token_hex(32))'` generates a suitable value; treat its
output as a secret. Never reuse the bot token as the MCP bearer. The template is systemd
`EnvironmentFile` syntax, not TOML or a shell script; do not `source` it. The binary does not
load this file itself. For foreground use, supply the same named variables through a protected
process launcher, or use the secret-file alternative below. Do not type tokens into command lines.

Alternatively, remove each `*_env` selection and choose `telegram.bot_token_file` and
`server.auth_token_file` in TOML, pointing to separate existing protected secret files. Files
must be regular, single-link, effective-user-owned and inaccessible to group/others (0600 or
0400); symlinks fail. With `server.create_auth_token_file = true`, the daemon exclusively creates
a random bearer file if absent in an existing private parent; it never overwrites a file.
For the user-unit example, put a generated bearer file under the writable state directory.
Keep an environment file present for that unit, even if it contains only comments when all
secrets use files. See [secret validation](CONFIGURATION.md) for full bounds.

Paths refer to the **daemon host's filesystem**, not an agent's remote workspace or a container.
TOML expands only initial `~/` using the daemon's HOME; no `$HOME` interpolation or relative
paths. Tool file paths must be absolute beneath approved roots, without symlinks or traversal.
Roots are pinned at startup; restart after intentionally replacing a root. Export only trusted
content: same-user hard links, mounts and in-place writes remain part of the local trust model.
Uploads stream the validated open handle, retain no file copy, and enforce the lower of the
configured limit and 50,000,000 bytes. Keep files stable until the send completes.

Start in the foreground after resolving secrets:

```bash
target/release/telegram-mcp-relay "$HOME/.config/telegram-mcp-relay/config.toml"
```

Startup opens storage/roots, binds loopback and finishes cleanup before serving traffic.
Failures return safe error codes and a nonzero exit; check private ownership, secret sources,
IDs, roots and port conflicts without enabling sensitive HTTP logs.

## Connect MCP clients

Configure one **remote Streamable HTTP** connection to `http://127.0.0.1:8765/mcp`, with
`Authorization: Bearer <MCP_BEARER>` when auth is enabled. Do not configure a stdio command
that launches a daemon per session. Keep the listener on loopback; no public port, tunnel,
reverse proxy, webhook endpoint or broad CORS is needed. Only the daemon needs outbound
Telegram HTTPS/DNS. Native clients may omit Origin; if present, Origin must be HTTP with the
configured numeric authority or `localhost`, including the port. Host has the same authority
restriction. A shared bearer and opaque session handles assume **trusted local clients**;
they do not isolate hostile agents or users with access to your process/files.

The locked SDK is rmcp 3.8.0. Only MCP **2026-07-28** is supported, including discovery and
subscriptions. A capable client must send matching `MCP-Protocol-Version` and `Mcp-Method`
headers and `params._meta` protocolVersion/clientCapabilities metadata; `Mcp-Name` must match
the tool name for calls or URI for resource reads. See
[transport details](ARCHITECTURE.md#protected-mcp-transport) and the
[HTTP fixtures](../src/routes/tests.rs). A minimal discovery body is:

```json
{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}
```

For that body, set `Mcp-Method: server/discover`, `MCP-Protocol-Version: 2026-07-28`,
`Content-Type: application/json` and `Accept: application/json, text/event-stream`, plus bearer
authorization. Older initialize-based revisions and legacy `resources/subscribe` are unsupported.
Do not assume a client works merely because it advertises Streamable HTTP.

| Capability | Evidence and limitation |
| --- | --- |
| Discovery, seven typed tools, private inbox reads | Synthetic rmcp 3.8.0 / MCP 2026-07-28 fixtures in [transport tests](../src/routes/tests.rs) and [daemon scenarios](../src/app/tests/recovery.rs); see the [coverage map](TESTING.md#specification-1619-coverage-map) for evidence boundaries |
| Resource events | POST SSE `subscriptions/listen`, accepted `notifications.resourceSubscriptions` filters and `notifications/resources/updated` are covered by [subscription fixtures](../src/routes/subscriptions.rs); older subscription APIs are unsupported |
| Concurrent asks/inbox/events and reconnect recovery | Synthetic [daemon recovery](../src/app/tests/recovery.rs) and [ask fixtures](../src/routes/ask.rs); no live client certification implied |
| Named agent clients, live Telegram, automatic idle-host wake | Unverified; no named-client or automatic wake guarantee |

Register/resume using a stable external ID and accurate title; save the returned session key
and inbox URI. Subscribe to that concrete canonical URI (no template or query page) using
`params.notifications.resourceSubscriptions` in `subscriptions/listen`, and inspect the accepted
filter. Read/claim on start, resume and notifications, without periodic agent polling. Resources
are read-only and never claim. Events are invalidations, not durable payload delivery: after a
disconnect, restart or slow-listener closure, reconnect and read SQLite-backed state.

`telegram_ask` waits by default up to 600 seconds **after send bookkeeping**. Configure the
client's tool-call timeout above that wait plus send/network/bookkeeping time; file calls can
take up to 600 seconds of network time. The server cannot set a host's timeout. Subscriptions
must permit long-lived streams. Waits and events work together; disabling waits does not disable
events. Timeout, disconnect or cancellation grants no permission and never acknowledges a
reply. Late Replies remain recoverable within retention. Notifications cannot promise to start
an idle LLM turn; use bounded asks while running or recover at the next resume.

Install the [complete agent policy](AGENT_POLICY.md) in the consuming project's instructions.
That document also explains current tokens and explicit processing reconciliation.

## Recovery and retention

Telegram keeps unconsumed updates for **at most 24 hours**. A longer outage can lose input
before the bridge sees it; local retention cannot reconstruct Telegram history. Accepted
replies/cursor survive restart in SQLite and require explicit completion acknowledgments.
Consumers must deduplicate by stable `reply_id`: a crash after an external effect but before
acknowledgment can redeliver already handled work.

The three local TTLs are independent and default to 30 days: routes from original send time,
replies from receipt, sessions from activity. New traffic never extends an old route; a valid
received reply survives its route's expiry. Cleanup preserves session identities needed by
children/receipts. Processing does not extend reply TTL. Startup and default daily cleanup
remove expired records in bounded batches; capacity pressure rejects work without evicting
valid replies. Pre-processing claims default to a 60-second lease; confirmed processing is
distinct from that lease, optional waiting and the maximum 600-second typing activity.

Send success confirms Telegram acceptance and a committed route. `TELEGRAM_DELIVERY_UNCERTAIN`
means delivery may have occurred; `SEND_BOOKKEEPING_FAILED` means Telegram accepted the send
but route persistence failed. Neither permits automatic resending or a fabricated route.
Client cancellation/lost responses can also hide success: admitted sends finish bookkeeping
while the daemon lives. `ASK_WAIT_FAILED` preserves committed outbound IDs and directs inbox
recovery. A definitive API rejection is different; inspect its safe numeric code/retry delay
before deciding on another intentional send. See [delivery](ARCHITECTURE.md#text-delivery).

Incoming Replies expose text/caption and document/photo metadata. Downloading attachment bytes
(`telegram_get_attachment`) remains deferred and is not one of the seven tools. Treat all
incoming text, names, metadata and attachments as untrusted input, not executable instructions.

## Optional systemd user service

Review the [user unit](../deploy/telegram-mcp-relay.service) and
[environment template](../deploy/relay.env.example). They are examples only; this change does
not install, start, enable or configure a service. If choosing this deployment later, place
the built executable at `~/.local/bin/telegram-mcp-relay`, the unit in your user-unit search
path, and the private configuration/environment files at the paths used above. Match every
path before activation; no root privileges are needed. Do not run a foreground copy alongside it.

The unit uses the current user, a 0077 umask, no new privileges/capabilities, private
user/temporary/devices namespaces and a read-only filesystem/home except the existing relay state
directory. Agents write exports outside this service; the daemon only needs read access.
If changing storage or generated-secret paths, review the explicit writable allowlist rather
than broadening access to all of HOME. Keep the configured roots visible; private `/tmp` is
not a shared export directory. Outbound IPv4/IPv6 and Unix sockets remain available for
Telegram/DNS and loopback MCP. Filesystem hardening in a user service requires unprivileged
user namespaces; availability varies by host. Verify this prerequisite without relaxing the
unit's protections to obtain a check pass. See the host's `systemd.exec(5)` documentation.

Host unit verification is an explicit operational acceptance step, still pending:

```bash
systemd-analyze --user verify deploy/telegram-mcp-relay.service
```

Verify on the deployment host before activation; for the current release workflow, this check
belongs to the assigned quiescent HOST boundary. It does not enable/install the unit or prove
runtime access. No resource figures have been measured here. SIGINT/SIGTERM close
admission and drain accepted sends. The example allows 660 seconds before forced termination
to accommodate the 600-second upload deadline and SQLite bookkeeping; reassess that margin
for your host. SIGKILL, crashes or an external stop deadline can interrupt post-send persistence,
so uncertain sends must never be blindly replayed. See [shutdown](ARCHITECTURE.md#runtime-and-ownership).
