# Configuration and Secrets

The library provides `Config::load`, side-effect-free `Config::parse` and explicit
`Config::load_secrets`, with no network I/O. Binary startup wiring is pending; the scaffold
still logs and exits. Start with the [synthetic example](../config/relay.example.toml);
unlisted settings retain [central defaults](../src/config.rs). Unknown fields, inline
credentials and API overrides fail. TOML input is bounded to 64 KiB.

Specify a nonzero destination `telegram.chat_id` inside nonempty `allowed_chat_ids`, positive
`allowed_user_ids`, and 1–64 export roots. IDs are bounded to 52 bits; ID lists to 256 entries.
Binding requires loopback and a nonzero port; `/mcp` is fixed. Paths must be absolute after
initial `~/` expansion against HOME. Relative paths, parent traversal, `~other-user` and `/`
export roots fail. Parsing does not inspect/create roots or storage. SQLite opening is explicit
through `Database::open`; `OutboundFiles::new` explicitly opens and pins existing export roots.
Root paths and every ancestor must be real directories without symlinks. File opening uses
Linux `openat2` (kernel 5.6+); unavailable or sandbox-denied resolution fails closed. Roots
are pinned until the opener is dropped: replacing a configured root pathname does not grant
access to its replacement. Recreate the opener to adopt an operator-changed root.

Bot secrets default to `TELEGRAM_BOT_TOKEN`; choose `telegram.bot_token_env` or `bot_token_file`,
never both. Optional MCP authentication uses `server.auth_token_env` or `auth_token_file`.
Bearer values need at least 32 ASCII bearer-alphabet characters; all credentials are limited
to 4,096 bytes (trailing CR/LF accepted). Debug and errors redact credentials, input and paths.
Unix secret files must be regular, single-link, effective-user-owned, owner-readable and
inaccessible to group/others (normally 0600/0400). Leaf symlinks and symlinked immediate parents
fail; descriptors are checked before bounded reads. Keep ancestors trusted and secret/database
paths outside exports. Protect environment sources with a mode-0600 service environment file.

`server.create_auth_token_file=true` requires an existing private owned parent (normally 0700).
It creates 64 hex characters from 32 OS-random bytes with exclusive mode-0600 creation and a
synced write. Existing files are validated, never overwritten/repaired; failed writes may leave
an incomplete private file requiring operator removal. No parent directories are auto-created.

Wait/grace settings accept 1–86,400 seconds; claims 1–600; independent typing duration 1–600
with refresh 1–5 seconds no longer than that duration. Long polling defaults to 45 (1–50).
Independent session/route/reply TTLs default to 30 days (1–3,650); cleanup to 24 hours (1–168).
Uploads accept 1–50 decimal MB, with an inclusive ceiling of 50,000,000 bytes or the lower
configured limit. Opening rejects relative paths, parent traversal, symlinks (including ones
whose targets remain inside a root), nonregular and unreadable files. Keep exported directories
and their contents trusted; confinement does not prevent same-user hard links, mounted content
or in-place writes. Files with all read bits cleared are rejected even under elevated privileges.
Only sanitized, at-most-128-byte ASCII basenames are prepared for upload; errors omit paths.
Document streaming and its enforcement of the opening size during mutation remain pending.
Final outbound text/captions always have the fixed 1,000-scalar
policy. Positive request/task/subscription/inbox/payload/metadata budgets reject excessive values
and inconsistent capacities. SQLite's physical budget must exceed payload plus metadata budgets;
later components enforce actual usage without early eviction. Host/Origin/authentication and
other runtime enforcement land with their components; this step supplies validated settings.

SQLite initialization requires an existing owned private parent directory (normally 0700)
and trusted ancestors. It creates only the database, with mode 0600; existing files and sidecars
must be owned regular single-link files with owner read/write access and no group/other access.
Parent/leaf symlinks and special files fail; existing permissions are never changed. The main
database file is page-limited by `storage.max_database_bytes`. Session/route admission enforces
`limits.sessions`, `limits.routes` and logical `limits.metadata_bytes`, including in-flight route
reservations. Physical journal accounting and inbox/other metadata admission remain later work.
See [architecture](ARCHITECTURE.md#session-and-route-persistence).
