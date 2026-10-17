# CLAUDE.md — Flit (Mail Client)

Project memory for Claude Code. Read at the start of every session. Keep edits to this file deliberate.

## What we're building

A minimal, privacy-first desktop email client for macOS (multiplatform later), meant to replace Canary Mail. Plain IMAP/POP3 + SMTP and Gmail, multiple accounts, unified inbox. No feature bloat — a clean, fast client for reading and writing mail.

**Stack:** Tauri v2 (Rust backend + system webview frontend), Svelte 5 + TypeScript + Vite frontend, Rust backend in `src-tauri/`.

## Design goals — the north star. Every decision serves these.

1. **Privacy-first.** Local-first: data lives only in a local SQLite DB. No sync server, no telemetry, no phone-home. Secrets in the OS keychain, all network over TLS.
2. **Open-source.** Permissive dependencies, no proprietary lock-in. (License: decide MIT/Apache-2.0 vs GPL early.)
3. **Blazing fast.** Instant startup from local cache, parallel async account sync, lazy-loaded message bodies, virtualized message list.
4. **Clean UI.** "Nice" here means typography, density, consistent spacing, dark mode, and keyboard control — not effects.

## Hard rules — never violate

- **Email-body rendering is security-critical.** The webview that renders message HTML MUST have JavaScript disabled, MUST block remote resource loading by default (remote images = tracking pixels), and MUST sanitize the HTML. Render bodies in a sandboxed, isolated context (sandboxed iframe / restricted webview), never in the app's main frame. Treat any change to this path as a security change and flag it explicitly. Decided 2026-07-13: the webview itself NEVER loads remote content — remote images the user opts into (policy "always"/"ask", see `mail/remote.rs`) are fetched by the Rust backend (TLS-only, no cookies/Referer, known trackers stripped) and inlined as `data:` URIs. Decided 2026-07-19, the ONE sanctioned exception (user-approved): the compose editor may hold a reply's quoted original — only the `mail::sanitize::sanitize_fragment` output (scripts stripped, remote refs inert, images restricted to `data:`/`cid:` at the editor schema too), and only after the user explicitly expands the parked quote (••• toggle). Raw or unsanitized message HTML still never enters the main frame. Decided 2026-10-17 (user-approved): the experimental on-device translation (`mail/translate.rs`) may replace the text of an already-sanitized body with the local model's translation — post-sanitize, pre-embed, the same spot as the quote fold in `build_srcdoc`. Tags and attributes come only from the sanitizer's output, byte for byte; the model's text goes back escaped, as text, so it can never become markup. The result renders in the same sandboxed iframe under the same CSP, and the reader always sees it labelled as a machine translation with the original one click away.
- **Secrets never hit disk in plaintext.** Account passwords and OAuth tokens go in the macOS Keychain (`keyring` crate). Never write credentials or tokens to plaintext files, logs, or the SQLite DB.
- **All network I/O over TLS.**
- **No telemetry, no analytics, no external calls** other than the user's own mail servers, the OAuth provider of an account that signs in with one (decided 2026-09-28, `auth/oauth.rs`: for Google only `oauth2.googleapis.com` — token exchange, refresh, and revocation when the account is deleted; the sign-in page itself opens in the user's default browser, never inside Flit, and the redirect comes back to a one-shot listener on 127.0.0.1), user-initiated remote-image loading (decided 2026-07-13; `mail/remote.rs`, default remains blocked/ask), and sender-domain avatar lookups (decided 2026-07-31; `mail/avatars.rs`, **default off**). The avatar exception is narrow and stays that way: the lookup key is the sender's **domain, never an address or a hash of one**, results are cached per domain (hits 30 days, misses 7) so a domain is contacted at most once per window, and fetching is batched at list load — never tied to opening a message, which would make it a tracking pixel. Per-address services (Gravatar and friends) are out: they would leak who the user corresponds with, not merely which organisations write to them. Decided 2026-09-14, the fourth and last sanctioned host: **model downloads for the experimental on-device summaries** (`llm/download.rs`, `llm/catalog.rs`). Only when the user clicks Download on a specific catalog model in Settings → Experimental — the feature switch alone contacts nothing. Each catalog entry pins an exact URL (repository revision) and SHA-256 on huggingface.co; the file is fetched once over TLS with no cookies or Referer, verified, and kept in the app data folder. Nothing about the user's mail is ever sent: summaries and translations run entirely on the local model and no message text leaves the machine.

## Development workflow

- **Always follow the `workflow` skill**: test-first, small atomic commits, every commit
  green. Never batch a whole feature into one commit.
- **Use the `stack` skill** for environment setup, running the app, Rust↔Svelte IPC
  patterns, and troubleshooting.
- Propose a plan before non-trivial changes; wait for review before executing.
- **Commits land directly on main** — push after every green commit
  (`git push origin HEAD:main` from a worktree). No feature branches or PRs
  unless explicitly requested.
- Never change a Hard Rule without flagging it explicitly.

| Task | Command |
|---|---|
| Run the desktop app (dev) | `npm run tauri dev` |
| Frontend only in browser | `npm run dev` |
| Build release app | `npm run tauri build` |
| Frontend tests | `npm test` |
| Frontend type/lint check | `npm run check` |
| Rust tests | `cargo test` (run in `src-tauri/`) |
| Engine smoke test against a real model | `FLIT_TEST_MODEL=<path>.gguf cargo test llm::engine -- --ignored` |
| Rust lint | `cargo clippy -- -D warnings` (in `src-tauri/`) |
| Rust format | `cargo fmt` (in `src-tauri/`) |
| Regenerate app icons | `npm run tauri icon -- icon.svg` |
| Regenerate the macOS 26+ Liquid Glass icon | `scripts/build-icon-assets-car.sh` |

The app icon's source of truth is `icon.svg` in the repo root; everything under
`src-tauri/icons/` is generated from it. Edit the SVG, never the PNGs. The
generator also writes `icons/android/` and `icons/ios/` — gitignored, this is a
desktop app.

**macOS 26 (Tahoe) Liquid Glass icon:** `src-tauri/icons/AppIcon.icon/` is a
separate, hand-authored Icon Composer source (foreground glyph only — no
squircle/background, macOS composites that from `icon.json`'s
`automatic-gradient` fill plus its own glass/shadow treatment). It compiles to
`src-tauri/icons/AppIcon.car`, which `tauri.conf.json`'s `bundle.icon` lists
directly — **not** the `.icon` source. Run
`scripts/build-icon-assets-car.sh` after editing `AppIcon.icon/` and commit
the regenerated `.car`.

Why pre-built instead of letting Tauri compile it: Tauri (≥2.11) can compile a
`.icon` source into `Assets.car` during `tauri build` via `actool`, but on
this machine `actool` (invoked through the cargo/tauri-cli process tree)
crashes deterministically with an internal `NSPlaceholderArray`/nil-object
exception in `ibtoold`, its asset-compiler XPC daemon. The exact same
`actool` invocation against the exact same files succeeds every time when run
directly from an interactive shell (which is what the script does — it also
kills any stale `ibtoold` first, another flaky-XPC-daemon symptom). Without
the `.icns` fallback (`CFBundleIconFile`, still generated from `icon.svg`),
pre-Tahoe macOS would show no icon at all.

`npm run tauri` goes through `scripts/tauri.sh`, which loads `.env` (see
`.env.example`) and signs macOS builds — dev binaries and release bundles alike
— with the local `flit-dev` identity, so the Keychain stops re-asking on every
rebuild. Machines without that identity fall back to ad-hoc signing.

The Rust build compiles llama.cpp from source (`llama-cpp-2`, for the experimental
on-device summaries) and therefore needs `cmake` — see the `stack` skill for
installing it without Homebrew.

All of `npm run check` + `npm test` + `cargo test` + `cargo clippy` must pass before any commit.

## Code conventions

- **Simplicity over cleverness.** This is a small app. Prefer the straightforward solution. Do NOT add abstraction (traits, generics, extra layers, "provider frameworks") until there's a concrete present need — usually the third real use, not the first. A little duplication beats the wrong abstraction. If you're about to build a framework, stop and ask.
- **Idiomatic, not inventive.** Write idiomatic Rust and idiomatic Tauri; follow established community conventions. Don't design novel architecture. When unsure of the idiomatic pattern, fetch current docs (see Tooling) instead of guessing.
- **Errors:** return `Result<T, E>`, use `?`. Define a small app error enum with `thiserror`. `#[tauri::command]` functions return a `Result` with a serializable error so the frontend can handle failure. No `.unwrap()`/`.expect()` on fallible runtime paths.
- **Async for all I/O** (IMAP, SMTP, DB). Fetch multiple accounts in parallel, never sequentially.
- **Svelte 5 runes** (`$state`, `$derived`, `$effect`). Do NOT emit Svelte 4 reactive syntax (`$:`). Keep components small.
- **Explain non-obvious decisions.** The maintainer is new to Rust and reviews to learn. When you make a non-trivial ownership/lifetime/async/architecture choice, say why (a short chat note or a `// why:` comment). Comment the *why*, not the *what*.

## Architecture — keep it exactly this simple

**Backend (`src-tauri/src/`):**

- `main.rs` / `lib.rs` — boots Tauri, registers commands, builds shared state.
- `state.rs` — `AppState` managed by Tauri (DB pool, per-account sessions/config). The app's shared state.
- `commands/` — the `#[tauri::command]` functions. The ONLY API surface the frontend calls. Keep them thin: validate input, call a module, return `Result`. No business logic here.
- `mail/` — IMAP/SMTP: connect, fetch headers/bodies, send, flags; MIME parsing.
- `storage/` — SQLite: schema, queries, cache.
- `auth/` — credentials, OAuth2, Keychain access.
- `models.rs` — shared structs (`Account`, `MessageHeader`, `Message`) serialized to the frontend.

**Frontend (`src/`):**

- Three-pane layout: sidebar (accounts + unified inbox) / message list / message body.
- Talks to the backend ONLY via `invoke('command_name', {...})`.
- State in Svelte runes/stores. No business logic in the frontend — it renders and calls commands.
- Mirror command payload types in `src/lib/types.ts` and keep them in sync with `models.rs`.

**Data model (multi-account from day one):**

- Every message row carries `account_id`.
- Unified inbox is not a special table — it's a query across all accounts, merged, sorted by date desc.
- Compose tracks the "From" account: reply → the account it arrived on; new mail → default account + a switcher.

## Non-goals — do not build unless explicitly asked

Snooze, rules/filters, PGP, calendar. Out of scope until the core read / write / multi-account flow is solid. (Full-text search was pulled out of this list and built 2026-07-12 — a local FTS5 index over the cache with Gmail-style operators, `storage/search.rs`; typo tolerance added 2026-09-29: a free-text word that matches nothing is searched as the closest indexed word — OSA distance, 1 typo from 4 letters, 2 from 8, first letter must match, letters only so numbers and codes are never swapped — and the list says which word it used. Send-later was pulled out of this list and built 2026-07-16 — local scheduler in `src-tauri/src/scheduler.rs`; missed sends never auto-send, the user confirms in a catch-up dialog. Threading/conversation view was pulled out and built 2026-07-17 — References/In-Reply-To based, thread keys computed at insert in `storage/messages.rs`, no subject fallback; flat Gmail-style conversation, accordion detail view; trash/junk/drafts views stay flat. IMAP IDLE/push was pulled out and built 2026-07-27 — `src-tauri/src/idle.rs`, opt-in via the `push_enabled` setting, one connection per account watching the inbox, re-IDLE every 29 min, capped backoff; servers without IDLE fall back to polling. The poller stays: under push its interval is the cadence for the folders IDLE cannot watch. On-device summaries were built 2026-09-14 as an Experimental setting — `src-tauri/src/llm/`: a static model catalog pinned by revision + SHA-256, download only on an explicit click, `llama-cpp-2` engine on its own thread with idle unload, message and conversation prompts that treat mail as untrusted material, a hashed summary cache, streamed tokens with cancel. Output is plain text only, never HTML; the buttons appear only while the switch is on and the picked model is on disk.)

## Backend crates (baseline)

`async-imap`, `lettre` (SMTP), `mail-parser` (MIME), `oauth2`, `rusqlite` or `sqlx` (SQLite), `keyring` (Keychain), `tokio`, `thiserror`, `serde`. **Confirm current versions and APIs via Context7 before writing code against any crate — do not rely on training data for crate APIs.**

## Tooling — use these, don't guess APIs

- **Context7 MCP** (when connected): fetch current, version-specific docs for Rust crates and libraries before coding against them. If not connected in the session, verify against docs.rs / official docs instead of training data.
- **Svelte MCP** (when connected): use for Svelte 5 / SvelteKit docs, and run its autofixer on Svelte code before finishing. Never emit Svelte 4 syntax.
- For Tauri, prefer current **Tauri v2** docs — Tauri v1 patterns are common online and wrong for this project.

## Build order — one phase at a time, commit after each

0. **Skeleton** — three empty panes, mock data, prove `invoke()` round-trips.
1. **Account model + storage** — SQLite schema with `account_id`; account CRUD; Keychain for secrets. (Before any real fetching.)
2. **Read path** — one IMAP account: fetch headers → list → click → body in the sandboxed webview (JS off, remote images blocked). Also: connection check in the add-account flow ("Verify & Save" — IMAP TLS connect + LOGIN, SMTP connect; save only on success). Decided 2026-07-08: phase 1 stores accounts unverified.
3. **Multi-account + unified inbox** — parallel fetch, merge, "All Inboxes" view.
4. **Write path** — compose, reply (correct From), send via SMTP.
5. **Gmail OAuth2** — built 2026-09-28: browser sign-in with PKCE + loopback redirect, IMAP/SMTP via XOAUTH2 (not the Gmail REST API). Google Cloud project `flit-mail-client`, "In production" but unverified (100-user cap, "unverified app" screen) until Google's review. The client id/secret come from `.env` / repo secrets at build time (`FLIT_GOOGLE_CLIENT_*`, see `.env.example`); a build without them offers no Google sign-in. A provider is plain data (`auth::oauth::Provider`) — Microsoft is meant to be one more constant.
6. **Actions** — read/unread, delete, archive, synced back to the server.
7. **Polish** — typography, spacing, dark mode, keyboard shortcuts (j/k, Cmd+Enter to send).
