# Feature: Gmail OAuth and IMAP Integration
**Date:** 2026-09-20
**Branch:** feature/gmail-imap-integration
**Stack:** Rust 1.98 · GTK 4.22 · libadwaita 1.9 · native Wayland
**Status:** Draft

## Problem

At the start of this feature, Whitford was a fixture-only desktop shell. The implemented developer preview now connects a Gmail account, retains authorization in Secret Service, and reads the newest 50 INBOX messages without mail mutations. Live browser, Gmail, keyring, Wayland, and Flatpak behavior remains manual acceptance rather than an automated claim.

## Solution

Build one complete, read-only Gmail vertical slice inside Whitford: load the installed-app OAuth client configuration from a local user configuration path, authorize through the system browser using Authorization Code + PKCE + state and a loopback callback, store only the refresh token in Freedesktop Secret Service, refresh access tokens when needed, authenticate to `imap.gmail.com:993` with XOAUTH2, fetch a bounded inbox summary, parse safe display fields, and replace the fixtures in the existing adaptive UI. Keep all network and secret operations off the GTK main thread and expose honest connecting, syncing, authentication, offline, and error states.

## User Story

As a Whitford user, I want to connect my Gmail account and see my real inbox so that the native shell becomes useful without giving Whitford my Google password.

## Scope: IN

- One Gmail account connected through Google's installed desktop OAuth flow.
- Authorization Code flow with PKCE S256, cryptographic state validation, loopback callback timeout, and system-browser launch.
- Scope limited to what Gmail IMAP requires: `https://mail.google.com/`, plus identity scopes needed to identify the account.
- Refresh token stored in Freedesktop Secret Service; access tokens remain memory-only.
- TLS Gmail IMAP connection on port 993 with XOAUTH2.
- Read-only synchronization of a bounded set of recent INBOX headers/body previews into existing folders/message/reader UI.
- Background worker/channel boundary so GTK never blocks on network or keyring work.
- Connect, reconnect, refresh, disconnect, loading, offline, expired/revoked authorization, configuration, protocol, and malformed-message states.
- Unit tests for OAuth construction/callback validation, token response parsing, IMAP response mapping, MIME/display fallbacks, and state transitions; network boundaries injectable/fakeable.
- Flatpak network and Secret Service permissions plus setup documentation.
- Strong ignore rules preventing local OAuth credential/token artifacts from entering Git.

## Scope: OUT

- Sending mail/SMTP, drafts, compose persistence, archive/delete mutations, and mark-read synchronization.
- Multiple accounts or non-Gmail providers.
- Full mailbox/folder synchronization, offline cache/database, attachment downloading, and push/IDLE updates.
- Google production verification and public credential distribution; this remains a local testing client.
- Persistent remote-content permission; HTML rendering uses a locked-down ephemeral WebKit session and requires a per-message opt-in for remote images.

## Context

- `Cargo.toml`: Rust 2024 crate currently depends only on GTK4 and libadwaita; all protocol/auth dependencies are new.
- `src/model.rs`: fixture models use static string references and must become owned, server-compatible values.
- `src/state.rs`: pure reducer and projections provide the seam for account/sync events and mailbox replacement.
- `src/ui/mod.rs`: currently constructs fixtures directly; becomes the bootstrap/coordinator boundary.
- `src/ui/actions.rs`: synchronous callbacks and synthetic surface previews become connect/refresh/disconnect dispatch points.
- `src/ui/build.rs`, `src/ui/render.rs`, `src/ui/widgets.rs`: hard-coded account and fixture rendering require connected/disconnected/error presentation.
- `src/main.rs`: application lifecycle entry point for service initialization.
- `build-aux/dev.whitford.Whitford.yml`: lacks network and Secret Service permissions.
- `README.md`: source of truth for build/test/manual verification; currently states fixture-only limitations.
- `.autofeature/patterns.md`: absent; existing small-module/pure-state style is the local convention.
- Existing automated coverage: 14 display-independent unit tests; no integration or GTK E2E harness.
- OAuth credential currently exists only outside the repo in `~/Downloads`; it must never be committed.
- Git has no remote. This run can produce a verified local branch and commits but cannot push or open a PR.

## Existing Code to Touch

- `Cargo.toml` / `Cargo.lock`: add narrowly selected OAuth, TLS/HTTP, IMAP, MIME, async/channel, serialization, browser, and Secret Service dependencies.
- `.gitignore`: exclude all local credential/config/token artifacts.
- `src/model.rs`: own live message data and introduce account/mailbox types without coupling protocol objects to UI state.
- `src/state.rs`: model connection/sync lifecycle and mailbox replacement while preserving pure reducer behavior.
- `src/ui/mod.rs`: coordinate worker events into GTK main-context state updates.
- `src/ui/actions.rs`: connect, sync, and disconnect commands.
- `src/ui/build.rs` / `src/ui/render.rs` / `src/ui/widgets.rs`: onboarding and real account status/content.
- New focused modules under `src/`: configuration, OAuth, secrets, Gmail/IMAP, message mapping, and worker/service boundary.
- `build-aux/dev.whitford.Whitford.yml`: add required sandbox access.
- `README.md`: document OAuth credential placement, Google test-user setup, runtime requirements, and manual test steps.

## Edge Cases to Handle

- Missing, unreadable, malformed, wrong-project, or wrong-client-type OAuth configuration.
- Loopback port failure, callback timeout, denial, state mismatch, missing code, browser-launch failure, and token-exchange errors.
- No refresh token returned on repeat consent, revoked/expired refresh token, unavailable or locked Secret Service, and explicit disconnect cleanup.
- DNS/TLS/timeout/IMAP authentication failures and transient offline operation.
- Empty inbox, non-UTF8/RFC 2047 headers, multipart mail, HTML-only mail, missing From/Subject/Date/Message-ID, and oversized bodies.
- Stale worker events arriving after disconnect or a newer sync.
- GTK window close while background work is in progress.
- Secrets or message contents reaching logs/error strings.

## Test Scenarios

- Fresh start without credential: disconnected UI explains the exact setup path without crashing.
- Valid credential and no saved account: connect opens system browser, validates callback, securely stores refresh token, authenticates with XOAUTH2, and shows inbox.
- Restart with saved refresh token: Whitford refreshes in the background and loads the inbox without prompting.
- Revoked token: saved secret is removed or invalidated safely and UI returns to reconnect state.
- Offline/timeout: existing messages remain visible where possible and status offers retry.
- Empty inbox and malformed messages render deterministic empty/fallback states.
- Disconnect removes the stored refresh token and clears account/mail data.
- Unit/integration fakes prove no network or secret dependency is required for the test suite.
- Live manual acceptance uses the already-created Whitford Linux OAuth client and confirms read-only Gmail IMAP login.

## Open Questions and Assumptions

- Automated-mode assumption: this first slice is intentionally read-only; mutation controls remain local/deferred and must not imply server success.
- Automated-mode assumption: the OAuth client JSON is user-supplied at `$XDG_CONFIG_HOME/whitford/google-oauth.json`, never embedded in source control.
- Automated-mode assumption: `org.freedesktop.secrets` is the required secure-store backend; lack of a keyring is a surfaced configuration error, not a plaintext fallback.
- Automated-mode assumption: fetch the newest 50 INBOX messages initially, with conservative per-message preview limits.
- The current `feature/wayland-mail-shell` branch is the implementation base because `main` contains only the original documentation baseline.

## Scope

**Tier:** single-layer

**Reasoning:** The change spans domain, protocol, worker, and GTK presentation modules, but all behavior lives in one native Rust desktop application and introduces no separate API, data store, or sibling repository.

**Subagents to spawn:**
- Product review lenses (pre-build)
- Technical Plan
- Rust/GTK architect in design and implementation modes
- Test runner
- Critical security, informational, testing, design, and DX review passes

**Skills to invoke:**
- Explore/context scan (completed)
- Security review fallback (auth/token-specific review; no packaged `security-review` skill is installed)
- Simplification fallback (review-driven; no packaged `simplify` skill is installed)
- Frontend-design skipped: this extends the approved shell rather than introducing a new visual system, and no `frontend-design` skill is installed

## Effort Plan

**Profile:** balanced
**Rules fired:** E2, E4

| Task | Effort | Why |
|------|--------|-----|
| Context scan | medium | base |
| Product review lenses | medium | base |
| Plan | high | E2 — OAuth, refresh tokens, secrets, and authentication |
| Rust/GTK architect (design) | high | E2 — owns auth/token and async integration design; E4 — prevent local Archive from claiming a Gmail mutation |
| Rust/GTK implementer | medium | base |
| Test runner | medium | base |
| Critical/security review | high | E2 — authentication and secret handling |
| Informational/testing/design/DX reviews | medium | base |

**Escalations during run:**
- E-fail: Rust/GTK implementation review retry at medium — adversarial review found OAuth callback timeout/fragmentation, cancellation-versus-persistence, recovery-state, and missing negative-test gaps after the first green build.
- E-fail: design/DX repair retry at high — the first repair exposed user-facing status, onboarding, stale-data, partial-message, accessibility, and developer-setup gaps.

## Product Review

The proposed slice can turn Whitford into a credible Gmail developer preview, but it must not present local-only actions, unsynchronized folders, or a 50-message snapshot as complete Gmail behavior. The critical requirement is to neutralize Archive/Delete behavior for live mail. The build also needs honest folder and freshness states, a complete disconnect path, clear OAuth permission disclosure, and end-to-end acceptance beyond protocol login.

**Mapped:** 11 existing surfaces and 6 user journeys.
**Verified:** 6 high-severity claims checked against code; 2 over-broad claims were refuted or narrowed.
**Counts:** 1 critical · 5 high · 4 medium · 0 low.

### In-scope findings

1. **🔴 Archive and Delete can falsely report success without changing Gmail — confirmed.** When live Gmail data is active, disable server mutations and their accelerators. They must not remove a message locally or report server success.
2. **🟡 Unsynchronized folders look usable and silently empty — confirmed.** INBOX is the only enabled live mailbox in this slice; other folders must be hidden or clearly unavailable rather than rendering as empty Gmail folders.
3. **🟡 The 50-message snapshot and its freshness appear account-wide — confirmed.** Disclose the bound, limit search/no-results language to loaded messages, and display the actual last successful sync time.
4. **🟡 Disconnect lacks a complete recovery loop — confirmed.** Add a discoverable action, confirmation, progress, secret/mail cleanup, and retryable cleanup failure handling. Ignore stale worker events after disconnect.
5. **🟡 Bring-your-own OAuth makes this a developer preview.** Name it honestly and surface exact credential/test-user setup in onboarding and documentation.
6. **🟡 The OAuth grant is broader than the read-only product behavior.** Explain that Gmail IMAP requires `https://mail.google.com/`, distinguish local disconnect from Google revocation, and never describe the granted scope itself as read-only.
7. **🟢 In-session offline states should retain visible messages.** Preserve loaded mail with a stale/offline banner and Retry; an offline cold-start cache remains out of scope.
8. **🟢 Browser authorization needs cancel/reopen/timeout recovery.** Late callbacks from cancelled or superseded attempts must not mutate current state.
9. **🟢 Acceptance must cover the complete live journey.** Verify first connection, reading, restart without reauthorization, manual refresh, transient failure, revoked authorization, and disconnect.

### Fast follows

- **Production-ready Google OAuth onboarding:** complete Google verification and distribute a Whitford-owned client before general-user release.
- **Paginated inbox with bounded local cache:** make older mail discoverable and useful across restarts.
- **Server-backed mail actions and mailbox synchronization:** safely re-enable archive, read-state, star, delete, and additional folders.

## Implementation Plan

### Step 0 — Scope Challenge

#### What already exists and should be extended

- `src/model.rs` already owns the UI-facing folder/message vocabulary and validation. Convert its borrowed fixture fields to owned values, make `MessageId` an owned stable ID, and keep protocol types out of this module. Do not add a second “live message” model.
- `src/state.rs` is already the single pure reducer/projection layer for selection, search, filters, status, and mailbox contents. Extend `AppState::dispatch` to accept account/service events and return typed effects; do not introduce a GTK-side shadow state machine.
- `src/ui/mod.rs` already centralizes dispatch and rendering and is therefore the correct coordinator for executing reducer effects, receiving worker events on the GLib main context, and rejecting all widget access from the worker.
- `src/ui/actions.rs` already owns window actions and accelerators. Replace demo surface actions with Connect, Cancel/Reopen authorization, Refresh, and confirmed Disconnect actions. Remove the Delete archive accelerator and the local-success archive reducer path for the live mailbox.
- `src/ui/build.rs`, `render.rs`, and `widgets.rs` already provide the adaptive shell and reusable status panels. Extend those widgets with onboarding/account/sync/error variants instead of building a separate connection window.
- Existing pure state/model tests and the `src/state/tests.rs` convention remain the base test harness. Network, Secret Service, browser, and GTK display availability must not be prerequisites for `cargo test`.
- GTK/GIO already provides portal-aware URI launching via `gio::AppInfo::launch_default_for_uri`; use it instead of adding an `open`/shell-command dependency.

#### Smallest complete slice

The minimum useful slice is one sequential account pipeline: resolve and validate one local Google desktop-client JSON file; restore one refresh token or run one PKCE loopback authorization; obtain an access token and verified email identity; read the newest 50 INBOX messages through read-only IMAP; map bounded MIME bytes to owned display values; render them in the existing shell; allow manual refresh; and delete the refresh token on confirmed disconnect. Anything that does not make that path secure, honest, recoverable, or testable is deferred.

The following are deliberately not prerequisites: a database/cache, pagination, background polling/IDLE, additional folders, attachment actions, SMTP, or any server mutation. The application may retain the last successful result only in memory during the current process so a transient refresh failure can show stale mail with an offline banner. A cold start has no mail until Gmail responds.

#### Complexity and module-boundary challenge

The change necessarily touches more than eight existing files and crosses more than two I/O boundaries, so the methodology's complexity trigger fires. Reducing it to one generic “mail service” file would hide security boundaries rather than remove complexity. Keep one runtime coordinator (`worker`) and five narrow modules (`config`, `oauth`, `secrets`, `gmail`, and `message`) with these rules:

- only `worker` sequences operations and owns cancellation;
- only `config`, `oauth`, `secrets`, and `gmail` perform their named I/O;
- only `message` turns untrusted RFC/MIME bytes into bounded UI values;
- `model` and `state` remain protocol-agnostic and pure;
- each new module stays near or below 300 lines of production code; split tests into a sibling `tests.rs` only if a module would exceed that threshold;
- do not create a dependency-injection framework or a service locator. Private traits with generic fake implementations are sufficient.

This is the smallest responsible boundary set for OAuth plus keyring plus IMAP. Further consolidation is rejected because it would make secret handling, cancellation, and unit tests less explicit.

#### Current Rust/GTK choices

- Use one dedicated OS thread containing a Tokio current-thread runtime. The runtime owns OAuth HTTP, loopback TCP, Secret Service D-Bus, TLS, and IMAP futures. It gives cancellation and timeouts without allowing GTK objects onto a worker or creating a second application-wide async runtime.
- Use `tokio::sync::mpsc::unbounded_channel` for low-volume typed commands/events. The GTK side awaits the event receiver with `glib::spawn_future_local`; only `Send + 'static` domain values cross the channel. No GTK/libadwaita type crosses it.
- Use `oauth2` 5.x for Authorization Code, cryptographic CSRF state, and PKCE S256; `reqwest` 0.12 with rustls and redirects disabled for token/userinfo HTTP; `secret-service` 5.x with `rt-tokio-crypto-rust`; `async-imap` 0.11 with its Tokio runtime feature; `tokio-rustls` plus WebPKI roots for IMAPS; and `mail-parser` 0.11 for RFC 5322/MIME/RFC 2047 and HTML-to-plain-text fallback. These releases support Rust 1.98. Add `serde`/`serde_json`, `futures-util`, `zeroize`, `tracing`, and `tracing-subscriber` only for the concrete uses described below.
- Prefer rustls to OpenSSL/native-tls so the native and Flatpak builds share a predictable TLS implementation. Never disable certificate or hostname verification.
- Prefer native `async fn` in private generic traits over `async-trait`; dynamic dispatch is not needed. Prefer standard `Result`, enums, `SystemTime`, atomics/generation counters, and RAII over custom frameworks.
- Use bounded reads, `tokio::time::timeout`, and task abortion. Do not use `thread::sleep`, a polling loop on the GTK thread, nested GLib main loops, or `block_on` from GTK callbacks.
- Bind OAuth only to `127.0.0.1:0`, derive the redirect URI from the bound socket before opening the browser, and accept only the expected callback path from a loopback peer. Do not bind wildcard interfaces.

#### Unit-testability challenge

Keep parsing and state transitions as pure functions. Define small private boundaries for `ConfigSource`, `BrowserLauncher`, `OAuthTransport`, `CredentialStore`, `Clock`, and `InboxSource`; the real worker wires them together and tests use deterministic fakes. In particular, do not make `mail-parser`, `async_imap::Fetch`, `reqwest::Response`, or `secret_service::Item` part of a reducer/UI contract. Adapt them at the boundary to `RawFetchedMessage`, `AccountIdentity`, `MailboxSnapshot`, and typed failure categories first.

### Step 1 — Architecture Review

#### Data flow

```text
 GTK/GLib main context                         dedicated worker OS thread
 ─────────────────────                        ──────────────────────────
 action/button
      │
      ▼
 AppState::dispatch(Action) ── Update.effects ──► Command { operation_id, kind }
      │                                              │
      │ render snapshot                              ├─ config file (read + validate)
      ▼                                              ├─ Secret Service (refresh token only)
 widgets                                             ├─ loopback listener + OAuth HTTPS
                                                     ├─ OIDC userinfo (identity)
 gio browser launch ◄── AuthorizationUrl event ──────┤
      │                                              └─ rustls IMAP/XOAUTH2
 browser → 127.0.0.1 callback                              │
                                                          ▼
                                              bounded RawFetchedMessage
                                                          │
                                              pure RFC/MIME mapper
                                                          │
 Event { operation_id, safe payload } ◄────────── MailboxSnapshot / FailureKind
      │
      ▼
 AppState::dispatch(WorkerEvent)
      │ reject stale operation_id
      ▼
 owned ViewSnapshot → render existing adaptive panes
```

The startup path sends `Restore`. No stored token yields disconnected onboarding; a stored token yields refresh → userinfo → IMAP. Connect binds the listener before emitting an authorization URL. Refresh reuses the stored refresh token. Disconnect first cancels the current task, then deletes the Secret Service item; only the successful deletion event clears identity and mail.

#### Explicit contracts

- `OAuthClientConfig`: validated `client_id`, optional desktop client secret, exact HTTPS Google authorization/token endpoints, and allowed loopback redirect form. Parsing rejects non-`installed` JSON, the wrong `project_id` (`whitford-email` for this developer-preview build), empty IDs, non-Google hosts, insecure endpoints, and oversized files. It implements a redacted `Debug` or no `Debug`.
- `AccountIdentity`: verified email returned by Google's HTTPS userinfo endpoint plus stable provider (`Gmail`). No access/refresh token and no unverified ID-token parsing enters the domain model.
- `MailboxSnapshot`: `Vec<Message>`, `SyncMetadata { completed_at, loaded_count, requested_limit: 50, fallback_count, skipped_count }`, and the single `Inbox` folder. Empty is a valid successful snapshot.
- `RawFetchedMessage`: UID, UIDVALIDITY, flags, optional internal date, and the complete `BODY.PEEK[]` response. It exists only between the Gmail adapter and pure message mapper and is dropped immediately after mapping.
- `MessageId`: owned opaque `gmail:{uidvalidity}:{uid}` value; never use array position or a possibly missing `Message-ID` header as identity.
- `WorkerCommand`: `Restore`, `Connect`, `Refresh`, `Disconnect`, `Cancel`, `ReopenAuthorization`, and `Shutdown`, with every user-visible operation carrying a monotonically increasing `OperationId` issued by the reducer.
- `WorkerEvent`: phase changes, authorization URL/deadline, successful identity/snapshot, no stored account, successful disconnect, and `Failure { kind, retryable, preserve_mail }`. Events contain static/sanitized user copy and typed categories, never raw library errors, URLs containing state/code, tokens, callback queries, or raw message bytes.
- `Update`: render flags, optional typed `UserNotice`, and a list of typed effects. It replaces the current `Copy` transition because service/browser effects own values. The UI executes effects after releasing the `RefCell` borrow.
- `SessionState`: one enum representing `Disconnected`, `Authorizing`, `Syncing`, `Ready`, `Offline`, `AuthRequired`, `Disconnecting`, `ConfigurationError`, and retryable `ServiceError`. It carries the active operation ID and only valid data for that phase. Mail contents remain a separate in-memory snapshot so Offline can retain the last successful list.

#### Main-thread and worker responsibilities

GTK/GLib main context only:

- allocate/mutate GTK and libadwaita objects, present dialogs/toasts, render snapshots, and launch the system browser through GIO;
- issue operation IDs, translate actions to worker commands, and ignore every event whose operation ID is no longer active;
- retain the last successful sanitized `MailboxSnapshot` in `AppState` for the current process;
- disable or hide every server-mutation control and remove its accelerator in live mode.

Worker thread only:

- read configuration; connect/unlock/search/create/delete Secret Service items; own all token values; bind/read loopback TCP; call token and userinfo endpoints; resolve/connect/TLS/authenticate/query IMAP; parse bounded MIME input; and send safe events;
- run at most one foreground account operation at a time and apply an explicit timeout to every external wait;
- drop/zeroize access, refresh, authorization-code, and XOAUTH2 buffers as soon as the operation completes or is cancelled.

No main-thread callback may call filesystem, D-Bus, DNS, HTTP, TLS, IMAP, MIME parsing, or thread join. Window destruction sends `Shutdown` without blocking; the worker aborts its current task, closes the loopback listener, drops secrets, closes event senders, and exits.

#### Cancellation and stale events

- The reducer increments `OperationId` for Restore, Connect, Refresh, Cancel, Disconnect, and any retry that supersedes work. `AppState` is the authority for the active ID.
- The worker controller uses `tokio::select!` over commands and the active `JoinHandle`. A superseding command aborts the handle before starting the replacement. `Cancel` returns to disconnected/onboarding; `Disconnect` aborts then performs secret deletion as its own current operation.
- The GLib receiver checks `operation_id == active_operation_id` before applying an event. Late callback, HTTP, keyring, IMAP, or channel events are therefore harmless even if an underlying library completes after cancellation.
- Reopen Authorization asks the worker to re-emit the existing URL for the same active attempt; it does not create new state/PKCE values. Cancel or timeout destroys the listener and URL. The authorization URL/state must not appear in derived `Debug` output or logs.
- Browser launch failure immediately dispatches `BrowserLaunchFailed`, cancels that generation, and offers Retry. A late callback for the cancelled generation is ignored.
- Closing the last window invalidates the generation before `Shutdown`, so no event can resurrect UI state.

#### Authentication and data-access boundaries

- Request only `https://mail.google.com/`, `openid`, and `email`. Onboarding must state that Google requires the broad mail scope for IMAP even though Whitford's implemented behavior is read-only.
- Add Google parameters needed for an offline grant (`access_type=offline`) and explicit consent when a new refresh token is required. If a first connection returns no refresh token, fail safely and explain how to retry consent; never persist an access token as a fallback.
- Store one refresh token in the default Freedesktop Secret Service collection under fixed application/provider attributes so replacement remains single-account. The label may identify Whitford/Gmail, but logs may not expose item labels, attributes, or email. No plaintext, file, environment-variable, settings, or in-memory-across-restart fallback is allowed.
- On refresh `invalid_grant`, attempt to remove the saved token and suppress automatic retry loops. Confirmed removal requires Connect; failed removal requires Disconnect cleanup first. Retain already loaded messages only for the current session. Network/5xx failures must not delete a valid token.
- Fetch the account email from Google's userinfo endpoint over verified TLS using the memory-only access token; require a non-empty verified email before building XOAUTH2.
- Gmail access is hard-coded to `imap.gmail.com:993`; TLS hostname and trust-chain validation are mandatory. Authenticate with XOAUTH2, then `EXAMINE INBOX` (not `SELECT`), sequence-fetch UIDs for only the newest at-most-50 messages, and issue one batched `UID FETCH` using complete `BODY.PEEK[]`. Do not issue unbounded search, `STORE`, `COPY`, `MOVE`, `EXPUNGE`, or non-PEEK body fetches.
- Map and sort the returned batch once; missing/malformed individual messages receive deterministic safe fallbacks or are counted as skipped. One malformed message must not discard the other 49. Raw MIME, token, and protocol objects never reach `AppState`.

#### Product-review requirements folded into the architecture

- Remove `ArchiveSelected` behavior for live mail, remove the Delete accelerator, and disable/hide Archive, Delete, Mark Read, Star, Label, Compose/Reply send, and attachment-download mutations as appropriate. No control may remove a live message locally or display success. Read-only copy is visible in onboarding/account status.
- Expose only INBOX. Do not render fixture Drafts/Sent/Archive/Trash/Starred as empty Gmail folders.
- Show `Newest 50 messages` and `Search loaded messages`; no-results copy explicitly means no match in the loaded set. Render actual `completed_at`, loaded/fallback/skipped counts, and stale/offline state.
- Disconnect is a confirmation flow with progress. It explains “remove Whitford's saved authorization” versus revoking access in Google Account settings, and it clears mail/identity only after deletion succeeds. A deletion failure retains state and presents Retry plus Google-revocation guidance.
- Onboarding calls the feature a developer preview and gives the exact credential location/test-user requirement. Browser authorization offers Cancel, Reopen Browser, and a visible timeout/retry path.

### Step 2 — Code Quality Review

- Keep a single error taxonomy (`FailureKind`) with boundary-specific source errors converted once. User copy is selected from the category and is never created by interpolating a raw error. Preserve source chains internally only where they are guaranteed redacted; logs use category fields rather than `Debug`-printing opaque third-party errors.
- Use descriptive phase names (`LoadingConfiguration`, `WaitingForBrowser`, `ExchangingCode`, `OpeningKeyring`, `RefreshingToken`, `ConnectingImap`, `FetchingInbox`, `Disconnecting`) rather than one overloaded `Loading` flag.
- Make security constants explicit and colocated: expected project ID, Google endpoint allowlist, callback path, callback timeout, HTTP timeout, IMAP timeout, message limit, raw byte limit, display-body limit, and preview limit.
- Introduce no `unwrap`/`expect` in production I/O code, no catch-all that silently continues, no detached task, and no `eprintln!("{error:?}")` containing third-party payloads. A malformed individual message is an explicit partial-success count, not a silent drop.
- Keep OAuth URL/callback parsing, XDG path resolution, config validation, XOAUTH2 encoding, MIME mapping, fallback construction, reducer transitions, freshness copy, and UID selection pure. I/O adapters should be thin.
- Reuse one HTTP client with redirects disabled and finite connect/request timeouts. Reuse the same redaction/error conversion helpers across token and userinfo calls. Do not duplicate status/error copy across `render.rs` and `widgets.rs`; project it once from `ViewSnapshot`.
- Prefer enums over booleans for session/phase/failure state. Remove obsolete `Surface` demo state and fixture-only archive paths rather than maintaining mutually inconsistent modes in production. Keep deterministic fixtures under `#[cfg(test)]` as builders for reducer tests.
- Cap sender/subject/header fields and the 280-character list preview on Unicode-safe boundaries, but preserve the complete reader body. Real `text/html` parts render in a locked-down WebKit view; `text/plain` remains a GTK label. When both alternatives exist, derive searchable/preview text from HTML so generated plain-text tracking destinations do not overwhelm the list.
- Use custom/redacted `Debug` implementations or omit `Debug` for token/config/auth-attempt containers. Wrap secret strings/buffers in zeroizing containers, while recognizing this is defense in depth rather than a guarantee against all allocator copies.

### Step 3 — Unit Test Plan

Tests remain display-independent and run with `cargo test`. Real Google, Secret Service, browser, network, and Flatpak checks are manual acceptance only.

`resolve_config_path` / `parse_oauth_config`: `src/config.rs`

- Happy: absolute XDG config directory plus valid installed-client JSON produces the exact path and validated endpoints/client data.
- Nil: absent `XDG_CONFIG_HOME` falls back to `$HOME/.config`; both absent return `ConfigDirectoryUnavailable` without panic.
- Empty/boundary: empty, whitespace, oversized, wrong top-level client type, missing fields, empty redirect list, wrong project, HTTP/custom endpoints, and invalid JSON map to precise configuration categories.
- Error: permission denied, directory instead of file, and read failure produce setup-facing errors without including file contents.

`begin_authorization` / `parse_callback_request`: `src/oauth.rs`

- Happy: binds loopback first; URL contains exact scopes, random CSRF state, S256 challenge, offline parameters, callback URI, and no token; matching callback returns one code and a success response.
- Nil: callback without code/error/state is rejected as malformed.
- Empty/boundary: blank code/state, duplicate query fields, wrong method/path/host, non-loopback peer, denial, oversized request, state mismatch, and timeout are distinct failures.
- Error: bind, accept/read/write, browser-launch notification, token HTTP, JSON, OAuth error response, redirect, timeout, and cancellation are surfaced and never panic.

`exchange_code` / `refresh_access_token` / `fetch_identity`: `src/oauth.rs`

- Happy: fake HTTP returns access+refresh on connect, access on refresh, and a verified Gmail identity; requests use the configured redirect/PKCE verifier and bearer auth only over allowed HTTPS endpoints.
- Nil: first grant without refresh token and userinfo without email/verification are rejected; refresh may legally retain the already stored refresh token when no replacement is returned.
- Empty/boundary: zero/expired lifetime, blank tokens, non-Gmail-shaped but valid email, malformed JSON, 429, 4xx, 5xx, and unexpected redirect classify correctly.
- Error: timeout/TLS/DNS and `invalid_grant` differ so only the latter enters AuthRequired and attempts credential cleanup.

`SecretServiceStore::{load,replace,delete}` plus worker-facing fake: `src/secrets.rs`

- Happy: zero items returns None; one unlocked item returns a zeroizing refresh token; replace uses fixed attributes and `replace=true`; delete removes the matching item.
- Nil: missing default collection/item is a typed unavailable/not-found outcome, never plaintext fallback.
- Empty/boundary: empty secret, invalid UTF-8, locked collection, duplicate matching items, and successful idempotent delete of zero items are explicit.
- Error: D-Bus unavailable, prompt cancelled, unlock denied, create/read/delete failure map to retryable keyring errors; fake-store tests prove connect refuses to continue after persistence failure and disconnect does not clear UI after deletion failure.

`xoauth2_response` / `select_newest_uids` / `map_imap_fetches`: `src/gmail.rs`

- Happy: XOAUTH2 bytes have the required control-A framing; unordered UIDs choose the newest 50; a batched response maps UID, UIDVALIDITY, unread/starred flags, and bounded bytes.
- Nil: absent UID or UIDVALIDITY is skipped and counted; no search results returns an empty successful mailbox.
- Empty/boundary: exactly 0/1/50/51 UIDs, duplicate UIDs, missing body, out-of-order responses, and Gmail challenge text are handled deterministically.
- Error: DNS/TCP/TLS/hostname/authentication/EXAMINE/SEARCH/FETCH/stream timeout/logout failure map to stable categories. Logout failure after a successful complete fetch is logged as sanitized cleanup context and does not erase valid results.

`map_message` and text/fallback helpers: `src/message.rs`

- Happy: RFC 2047 sender/subject, multipart alternative, quoted-printable/base64, plain text, HTML-only-to-text, flags, date, and attachment metadata become bounded owned `Message` values.
- Nil: missing From, Subject, Date, Message-ID, text body, initials, and attachment details each use documented fallbacks; UID remains the stable ID.
- Empty/boundary: empty inbox, empty headers/body, non-UTF-8, malformed MIME, nested multipart, huge headers, body cut mid-encoding, script/style HTML, NUL/control characters, emoji/multibyte truncation, and overlong fields cannot panic or leak markup.
- Error: parser returns no message/usable body yields a fallback row or explicit skipped count; one bad record does not fail the batch.

`worker::run` / orchestration flows with fake boundaries: `src/worker.rs`

- Happy: Restore with saved token, first Connect, manual Refresh, Reopen Authorization, Cancel, and Disconnect emit ordered phase/final events with the same operation ID.
- Nil: Restore with no secret emits `NoStoredAccount`; Refresh while disconnected and Reopen without an active attempt are safe no-ops/user notices.
- Empty/boundary: empty mailbox is success; replacement command aborts prior work; channel receiver drop and Shutdown terminate; repeated Refresh is coalesced/superseded.
- Error: each fake boundary fails in turn and emits the right retryable/preserve-mail classification; no token/raw payload appears in event `Debug`; aborted tasks cannot emit an accepted terminal event.

`AppState::dispatch`, selection, search, and event generation: `src/state.rs` and `src/state/tests.rs`

- Happy: onboarding → authorizing → syncing → ready; ready → refreshing → ready; ready → confirmed disconnect → disconnected; mailbox replacement repairs selection.
- Nil: no selected message/account/snapshot is safe; empty mailbox becomes `EmptyInbox`, not `NoSearchResults`.
- Empty/boundary: only INBOX is exposed; loaded-set search wording and newest-50 metadata are present; in-session offline retains messages; cold-start failure does not show fixtures; reducer operation ID rollover uses checked/wrapping behavior without reaccepting stale work.
- Error: stale/out-of-order events, browser failure, auth denial/timeout, revoked grant, partial parse, offline sync, keyring failure, and disconnect cleanup failure land in distinct recoverable states.
- Mutation guard: Archive/Delete/Mark Read/Star/Label actions and Delete accelerator cannot mutate messages or produce success; folder navigation cannot select an unavailable folder.

Pure UI projections: `src/ui/render.rs` / `src/ui/widgets.rs` (test extracted copy/projection helpers, not GTK object internals)

- Happy: account, exact last-sync time, loaded limit/count, read-only disclosure, and enabled Retry/Disconnect actions project correctly.
- Nil/empty: missing account/time and empty inbox/onboarding render deterministic copy.
- Error: configuration, keyring, browser, offline, revoked-auth, protocol, partial-message, and disconnect-cleanup failures project actionable non-sensitive copy.

### Step 3b — Error & Rescue Map

| Boundary/codepath | What can fail | Typed category | Rescue/action | User sees |
|---|---|---|---|---|
| Resolve XDG config path | missing/non-Unicode environment, relative XDG path | `ConfigDirectoryUnavailable` | do not guess or read repo paths; provide native/Flatpak paths | exact setup-path guidance |
| Read client JSON | missing, permission denied, directory, I/O, over size limit | `ConfigMissing` / `ConfigUnreadable` / `ConfigTooLarge` | remain disconnected; Retry after correction | developer-preview setup instructions |
| Parse/validate client JSON | malformed, wrong client type/project, empty ID, unsafe/wrong endpoint | `ConfigInvalid` / `ConfigWrongProject` | reject before browser/network; never echo JSON | field-level corrective guidance without values |
| Bind loopback listener | address/socket exhaustion or permission | `LoopbackBind` | cancel attempt; new Retry selects another ephemeral port | “Could not start secure sign-in callback” |
| Accept/read callback | timeout, cancellation, oversized/malformed HTTP, wrong method/path/peer | `AuthorizationTimedOut` / `CallbackInvalid` | close listener; offer Retry/Reopen while valid | timeout or invalid-response recovery copy |
| Validate OAuth callback | denial, missing code, blank/duplicate state, mismatch | `AuthorizationDenied` / `StateMismatch` | invalidate attempt/PKCE; never exchange | denied or “response could not be verified” |
| Launch system browser through GIO | no handler, portal denied, launch error | `BrowserLaunchFailed` | cancel generation; show Retry and copyable/open-again action without logging URL | “Could not open your browser” |
| Token exchange HTTP | DNS/TLS/timeout, redirect, 4xx/429/5xx, malformed JSON | `Network` / `TokenRejected` / `RateLimited` / `ProviderUnavailable` / `Protocol` | bounded retry only on explicit user action; retain no first-grant token on failure | actionable retry/provider message |
| First token response | no/empty refresh token | `RefreshTokenMissing` | drop access token; retry Connect with explicit consent | consent-specific reconnect guidance |
| Refresh token HTTP | transient network/5xx/429 | same transient categories | retain stored token and in-session mail; show Retry | stale/offline banner with last successful sync |
| Refresh token HTTP | `invalid_grant`/revoked | `AuthorizationExpired` after confirmed delete; otherwise `DisconnectFailed` | attempt secret delete, stop auto retry, require Connect or cleanup retry | authorization expired/reconnect, or explicit cleanup-required warning |
| Userinfo HTTP/parse | transport, malformed JSON, absent/unverified email | `IdentityUnavailable` / `IdentityInvalid` | do not build XOAUTH2 or store a first grant without a valid identity | “Could not verify this Gmail account” |
| Connect to Secret Service | session bus/service unavailable | `KeyringUnavailable` | no plaintext fallback; Retry after service starts | keyring requirement and Retry |
| Unlock/search item | locked, prompt cancelled, duplicate items, access denied | `KeyringLocked` / `KeyringAmbiguous` | leave state intact; offer Retry/Forget recovery | specific keyring recovery, no labels/attributes |
| Read stored secret | missing, empty, invalid UTF-8, read failure | `NoStoredAccount` / `StoredCredentialInvalid` | missing → onboarding; invalid → allow confirmed cleanup | reconnect/forget saved authorization |
| Replace stored secret | unlock/create/replace failure | `CredentialSaveFailed` | abort connection and zeroize tokens; do not claim connected | “Authorization could not be saved securely” |
| Delete stored secret | prompt/delete/D-Bus failure | `DisconnectFailed` | keep account/mail state, stop active network, offer Retry and Google revocation link | “Whitford could not remove its saved authorization” |
| DNS/TCP to Gmail | offline/refused/timeout | `Offline` / `ImapUnavailable` | preserve last in-session snapshot; explicit Retry; cold start stays empty | stale/offline banner or connection error |
| TLS handshake | trust/hostname/protocol failure | `TlsFailed` | never downgrade/disable validation; no automatic retry loop | secure-connection failure |
| XOAUTH2 authentication | rejected/challenge | `ImapAuthenticationFailed` | distinguish after refreshed token; require reconnect, preserving in-session mail | authorization/reconnect message |
| `EXAMINE INBOX` | mailbox absent/permission/protocol | `InboxUnavailable` | no fallback folders; preserve old snapshot | INBOX unavailable with Retry |
| UID SEARCH/FETCH stream | timeout, BYE, parse, partial stream, oversized response | `ImapProtocol` / `SyncTimedOut` | discard incomplete new batch and preserve prior snapshot | sync failed; prior mail marked stale |
| Individual MIME/header parse | malformed/truncated/non-UTF8/missing fields | `MessageFallback` / counted skip | bounded fallbacks; continue other records | row fallback and aggregate “some messages limited” note |
| Batch has no messages | valid empty result | not an error | commit empty snapshot | deliberate empty-INBOX state |
| Worker task panic | unexpected panic/join error | `WorkerFailed` | controller emits one sanitized terminal failure and remains able to retry/restart worker if safe | “Background mail service stopped” |
| Command send fails | worker exited/channel closed | `WorkerUnavailable` | transition to service error; never freeze spinner | retry/restart-app guidance |
| Event receiver/UI gone | window closed/channel closed | shutdown condition | abort task, close listener/socket, zeroize/drop secrets, exit thread | nothing; application is closing |
| Stale event | result arrives after newer operation/disconnect | `StaleEvent` diagnostic only | reducer drops before mutation; count at trace/debug without payload | nothing; current state remains authoritative |

Every row above gets either a unit test at the adapter/reducer boundary or a manual platform check where the real OS integration cannot be hermetic. There is no allowed path with neither rescue nor user feedback.

### Step 3c — Shadow Path Testing

| Data flow | Happy | Nil | Empty | Error | Required assertion |
|---|---|---|---|---|---|
| Environment → config path → file bytes | XDG path resolves | XDG absent uses HOME | empty file is invalid | read denied/missing | no repo/Downloads fallback and exact setup copy |
| JSON → validated OAuth config | installed client accepted | missing optional secret accepted | blank ID/redirect rejected | malformed/wrong project/unsafe endpoint | no browser/network work after rejection |
| Connect → listener → browser → callback | matching code/state | no code/state rejected | blank/duplicate params rejected | bind/launch/deny/timeout/mismatch | listener closes and generation invalidates |
| Code → token → secure persistence | refresh token saved | missing refresh rejected | blank token rejected | HTTP/keyring failure | no connected state and all token buffers dropped |
| Saved refresh → access token → identity | verified email returned | no saved secret → onboarding | blank/invalid secret → cleanup route | offline/invalid_grant/userinfo failure | transient errors keep secret; invalid grant does not loop |
| Access token + email → TLS/XOAUTH2 IMAP | authenticated read-only session | missing identity/token cannot call IMAP | empty INBOX succeeds | TLS/auth/protocol/timeout | only EXAMINE/SEARCH/PEEK commands and old snapshot retained on failure |
| UID set → newest bounded fetch | newest 50 fetched once | absent UID skipped | zero UIDs → empty snapshot | partial stream discarded | no N+1 fetch and requested limit disclosed |
| Raw bytes → MIME → `Message` | decoded plain display data | missing fields use fallback | empty/truncated body safe | malformed record counted, batch continues | no panic/HTML/script/control leakage; caps enforced |
| Worker event → reducer → view | matching ID commits snapshot | no snapshot renders onboarding/empty | empty mailbox distinct | stale/failure event | stale never mutates; failure has meaningful copy |
| Refresh with existing snapshot | new snapshot replaces atomically | no previous snapshot shows full loading | successful empty result clears list | transient failure retains old snapshot | last-sync remains the last success and banner says stale |
| Confirm Disconnect → secret delete → clear | deletion succeeds then clears | no item is idempotent success | empty secret item still removed | keyring/delete failure | failure retains identity/mail and offers Retry |
| Window close → Shutdown | task/listener cancelled | worker already gone is harmless | empty channels terminate | event send races with close | no join/block on GTK and no accepted late event |

No implementation step is complete until all four cells for its data flow have both an explicit behavior and test coverage (unit fake or named manual acceptance).

### Step 3d — Observability Checklist

- Initialize `tracing_subscriber` once in `main` with environment filtering, but default to concise info/warn output. Use structured events at operation start/end, phase change, cancellation, partial-result count, and categorized failure.
- Allowed fields: operation ID, phase, elapsed milliseconds, retryable/preserve-mail booleans, loaded/fallback/skipped counts, configured limit, and stable error category. Even in debug builds, do not enable HTTP/IMAP wire logging.
- Forbidden in every log, panic, toast, `Debug`, or error string: client secret, client ID if avoidable, authorization URL, PKCE verifier/challenge, CSRF state, callback query, authorization code, access/refresh token, XOAUTH2 bytes, HTTP Authorization header, Secret Service secret/item attributes, account email, sender/recipient, subject, Message-ID, UID, header/body/MIME bytes, attachment name, search query, or rendered message content.
- Token/config/authorization-attempt wrappers omit or redact `Debug`. `WorkerCommand`/`WorkerEvent` custom `Debug` implementations print variants and operation IDs only. Never log third-party errors wholesale unless their redaction has been proven; map to `FailureKind` first.
- User-visible errors are static/category-driven and actionable. Raw OS/network/provider text remains internal and is not interpolated because it can echo URLs, server challenges, paths, or content.
- Log one sanitized error at the boundary that decides recovery, not at every propagation layer. Cancellation and expected stale-event drops are debug/trace, not warnings. Missing config/no saved account are normal state, not errors.
- Record duration for config, keyring, authorization wait, token, userinfo, TLS/IMAP, fetch/map, and disconnect cleanup. Do not record payload sizes per message; aggregate total bounded bytes only if needed.
- Tests capture tracing output for representative token, callback, email, subject, body, and search-query canaries and assert none appears. Also assert formatted errors/events are redacted.
- A worker panic, closed command channel, or event receiver loss is never silent: emit a sanitized category if a UI still exists, then terminate cleanly. No empty catch/ignored `Result`; cleanup-only logout/browser-response write failures may be downgraded only with a sanitized trace and documented reason.

### Step 4 — Performance Review

- Bound work at every layer where it does not alter user-visible message content: one account, one mailbox, newest 50 UIDs, one batched fetch, 280-character preview, bounded headers/fields, and finite callback/HTTP/IMAP timeouts. Reader bodies are complete.
- Use UID SEARCH plus one UID FETCH set; do not put an IMAP round trip inside a message loop. Sorting at most 50 records is `O(n log n)` and filtering/search remains `O(n)`. Deduplicate UIDs with a set before fetch/mapping.
- Parse/map off the GTK thread and commit a complete `MailboxSnapshot` atomically. Never stream partial rows into widgets. Drop raw fetch/MIME buffers immediately after mapping and do not retain token responses or access tokens beyond the current operation.
- One foreground worker operation avoids duplicate refresh/sync traffic. Refresh while already syncing either disables the action or supersedes/coalesces the prior generation; it never queues unbounded work.
- Rebuilding the GTK list is acceptable at the fixed limit of 50. Do not add diffing/virtualization in this slice; revisit before pagination or larger mailboxes.
- The in-session stale snapshot is the only retained mail copy. Expected upper-bound payload memory is a few MiB plus owned display strings; there is no database, disk cache, attachment payload, or background polling.
- Configure HTTP connection/request timeouts and wrap token, userinfo, callback, and each IMAP phase in timeouts. Cancellation drops sockets/futures; the main thread never waits for thread shutdown.

### Files to create

- `src/config.rs`: XDG path resolution, bounded file read, Google installed-client JSON parsing/allowlist validation, and redacted config errors/tests.
- `src/oauth.rs`: loopback authorization attempt, PKCE/state URL construction, callback parser/response, token refresh/exchange, verified userinfo lookup, timeouts, and pure/fakeable transport tests.
- `src/secrets.rs`: narrow Freedesktop Secret Service adapter for one fixed Whitford Gmail refresh-token item, zeroizing value wrapper, error mapping, and fake contract tests.
- `src/gmail.rs`: rustls connection, XOAUTH2 framing, read-only IMAP session, newest-UID selection, one bounded PEEK fetch, protocol-to-raw adaptation, and adapter tests.
- `src/message.rs`: pure bounded RFC/MIME-to-domain mapping, HTML-to-text and missing-field fallbacks, partial-result accounting, and adversarial parser tests.
- `src/worker.rs`: worker handle, typed commands/events, Tokio current-thread controller, operation cancellation, orchestration over generic private boundary traits, shutdown, and fake end-to-end tests.
- `.autofeature/tests/gmail-imap-integration-2026-09-20.md`: acceptance manifest for the complete live developer-preview journey and its recovery paths.

### Files to modify

- `Cargo.toml` / `Cargo.lock`: add only the pinned-compatible dependency set described in Step 0 with default features disabled where needed; keep rustls as the single TLS strategy.
- `.gitignore`: add targeted recursive patterns for `google-oauth*.json`, Google `client_secret*.json`, token/credential exports, `.env*`, and local secret directories while keeping redacted test fixtures trackable by explicit negation if needed.
- `src/lib.rs`: export the new internal modules needed by the binary/tests; do not expose secret-bearing types publicly.
- `src/model.rs`: replace static string IDs/fields with owned protocol-neutral values, define account/sync/mailbox metadata, restrict production folders to INBOX, and move fixture builders behind `#[cfg(test)]`.
- `src/state.rs`: replace `Surface` with the explicit session state, add worker/user actions and effects, operation-ID stale rejection, atomic mailbox replacement, in-session offline preservation, disconnect cleanup semantics, and loaded-set/freshness projections.
- `src/state/tests.rs`: retain existing search/selection/navigation coverage with owned test data; replace fixture archive-success tests with mutation guards and add the complete account/sync/error/stale-event transition matrix.
- `src/ui/mod.rs`: own the worker handle/event receiver, execute reducer effects after releasing state borrows, receive events via `glib::spawn_future_local`, route browser failures, and send non-blocking Shutdown on window destruction.
- `src/ui/actions.rs`: install Connect/Cancel/Reopen/Refresh/Disconnect confirmation actions; remove demo actions and the Delete/archive accelerator; keep all real mutations disabled and honest.
- `src/ui/build.rs`: replace the hard-coded account block with onboarding/account controls, add sync/error banners and confirmation UI, retain adaptive structure, and provide widget handles/actions needed by render state.
- `src/ui/render.rs`: render account/session phases, actual last successful sync, newest-50 and loaded-search disclosures, stale/partial states, disabled read-only controls, and only the INBOX folder.
- `src/ui/widgets.rs`: add reusable onboarding/status/error/action rows and pure copy projections; update message/reader widgets for owned optional fields and capped plaintext.
- `src/main.rs`: initialize redacted structured logging, construct/present the UI as before, and replace raw debug startup output with a sanitized category.
- `src/style.css`: add only the states required for onboarding, progress, warning/offline banner, read-only disclosure, disabled actions, and confirmation feedback using existing tokens.
- `build-aux/dev.whitford.Whitford.yml`: add `--share=network`, narrowly scoped read-only OAuth config access (`--filesystem=xdg-config/whitford:ro`), and `--talk-name=org.freedesktop.secrets`; continue browser launch through the desktop portal/GIO and document that the manifest still needs a real Flatpak build check.
- `README.md`: replace fixture-only claims; document native and Flatpak credential paths, exact JSON name, Google test-user/developer-preview status, scope disclosure, Secret Service/runtime requirements, read-only/50-message limitations, revocation versus local disconnect, commands, and complete manual acceptance steps.

### Implementation order

1. Update ignore rules first, then add/pin dependencies and module declarations. Verify the real OAuth JSON and all token artifacts remain outside Git before any test/run.
2. Convert `model` to owned protocol-neutral values and adapt existing fixtures/tests; keep the current test suite green before adding I/O.
3. Expand the pure reducer to `SessionState`, operation IDs, typed effects/events, mailbox replacement, stale-event rejection, offline retention, and mutation guards; complete its transition tests.
4. Implement and test configuration resolution/parsing/endpoint allowlisting.
5. Implement and test pure OAuth URL/callback logic, then the bounded loopback and reqwest adapters with fake HTTP.
6. Implement and test the Secret Service adapter and fake credential-store contract, including idempotent deletion and failure retention.
7. Implement and test MIME/display mapping from protocol-neutral raw messages, including all malformed/HTML-only/bounded cases.
8. Implement and test Gmail TLS/XOAUTH2/read-only EXAMINE/SEARCH/batched-PEEK adaptation behind a fakeable inbox boundary.
9. Implement the worker controller/orchestration, timeout/cancellation/shutdown/redaction, and fake end-to-end tests before connecting GTK.
10. Wire typed effects/events into `ui/mod.rs`, replace actions and hard-coded account/status widgets, expose only INBOX, render honest limit/freshness/error states, and disable every mutation/accelerator.
11. Add structured logging/redaction tests, Flatpak permissions, README setup/security/acceptance documentation, and the test manifest.
12. Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release`; then manually verify native Wayland first connect, message reading, restart restore, refresh, transient failure, revoked authorization, cancel/reopen/timeout, empty/partial mailbox, and disconnect. A Flatpak result must remain explicitly unverified unless `flatpak-builder` and the GNOME runtime are installed and exercised.

### Unit test files

- `src/config.rs` (`#[cfg(test)]`): XDG/file/config parsing and endpoint validation.
- `src/oauth.rs` (`#[cfg(test)]`): authorization construction, callback/state validation, fake HTTP token/userinfo classification, timeout/cancel behavior.
- `src/secrets.rs` (`#[cfg(test)]`): fake store contract and error mapping; no live keyring dependency.
- `src/gmail.rs` (`#[cfg(test)]`): XOAUTH2 framing, UID bounding/batching, raw fetch adaptation, IMAP error classification; no live Gmail dependency.
- `src/message.rs` (`#[cfg(test)]`): RFC/MIME decoding, HTML-to-text, caps, fallbacks, malformed and partial messages.
- `src/worker.rs` (`#[cfg(test)]`): full orchestration with deterministic fake config/auth/store/inbox/browser/clock boundaries, cancellation, channel close, and redaction.
- `src/state/tests.rs`: pure session/sync/reducer/search/selection/stale-event/disconnect/mutation behavior.
- Pure projection tests colocated in `src/ui/render.rs` or `src/ui/widgets.rs`: static copy and action availability without constructing GTK widgets.

### Deferred / NOT in scope

- Pagination, older-message loading, full-text server search, local database/cache, and offline cold start: require a persistence/sync model beyond this bounded slice.
- SMTP, compose/send/reply, drafts, archive/delete, read/unread, star, label, and folder mutations: require server-backed commands, conflict/error semantics, and dedicated acceptance coverage.
- Additional Gmail folders, labels, multiple accounts, and non-Gmail providers: would change identity, secret-keying, navigation, and sync architecture.
- IDLE/push, periodic background refresh, notifications, and retry daemons: avoid hidden traffic and lifecycle complexity until one-shot sync is proven.
- Attachment actions remain disabled. Complete messages are retrieved, HTML renders in an ephemeral locked-down WebKit session, and remote images remain blocked until the user opts in for that message.
- Public OAuth credentials and Google production verification: this build remains an explicitly documented bring-your-own-client developer preview.
- Google-side token revocation as part of local Disconnect: local secret removal is implemented and clearly distinguished; documentation links to Google Account access revocation.

### User Challenges

None. Automated-mode assumptions in the brief resolve the product choices without a fundamental architecture change. The plan challenges breadth by retaining only the secure, read-only, one-account/newest-50 vertical slice and deferring all cache, pagination, mutation, SMTP, and multi-account work.

## Rust/GTK Architecture Design

This section is the implementation contract. It narrows a few choices in the plan where the current crate APIs or Flatpak behavior require a more precise design.

### Dependency contract

Keep the existing `gtk = 0.11.4` and `libadwaita = 0.9.2` declarations and their GTK 4.22/libadwaita 1.9 feature gates. Add the following direct dependencies; these versions resolve on Rust 1.98.1 and avoid a second HTTP or TLS stack:

```toml
async-imap = { version = "0.11.3", default-features = false, features = ["runtime-tokio"] }
futures-util = { version = "0.3.34", default-features = false, features = ["std", "async-await"] }
mail-parser = { version = "0.11.9", default-features = false, features = ["full_encoding"] }
oauth2 = { version = "5.0.0", default-features = false, features = ["reqwest", "rustls-tls", "timing-resistant-secret-traits"] }
reqwest = { version = "0.12.28", default-features = false, features = ["json", "rustls-tls-webpki-roots"] }
secret-service = { version = "5.2.0", default-features = false, features = ["rt-tokio-crypto-rust"] }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
tokio = { version = "1.53.1", features = ["rt", "net", "io-util", "sync", "time"] }
tokio-rustls = { version = "0.26.5", default-features = false, features = ["ring", "tls12"] }
tracing = { version = "0.1.44", default-features = false, features = ["std"] }
tracing-subscriber = { version = "0.3.23", default-features = false, features = ["fmt", "ansi"] }
unicode-segmentation = "1.13.3"
url = "2.5.8"
webpki-roots = "1.0.9"
zeroize = "1.9.0"
```

`Cargo.lock` is the exact transitive pin. Do not add `native-tls`, OpenSSL, `async-native-tls`, `async-trait`, an `open` crate, a second `reqwest` major, or a direct `glib`/`gio` version; use `gtk::glib` and `gtk::gio` so the GTK object graph stays on its existing 0.22 family. Do not add `base64`: `async-imap::Client::authenticate` base64-encodes the raw `Authenticator::Response`, so the application supplies the control-A-framed XOAUTH2 bytes and must not encode them twice.

`async-imap` 0.11.3 remains the choice over a replacement: it is an actively released Tokio-capable client, exposes `EXAMINE`, `uid_search`, streaming `uid_fetch`, and a custom SASL `Authenticator`, and accepts an application-provided stream. It does not provide the desired rustls connector, so construct `TcpStream` + `tokio_rustls::TlsConnector` and pass the resulting stream to `async_imap::Client::new`. Explicitly consume and validate the server greeting before calling `authenticate`; `Client::new` does not do that. Build the IMAP `rustls::ClientConfig` through the `tokio_rustls::rustls` re-export with the ring provider, TLS 1.2/1.3, and `webpki_roots::TLS_SERVER_ROOTS`. Never install a permissive verifier.

Use one reusable `reqwest::Client` configured with `redirect::Policy::none()`, `use_rustls_tls()`, a 10-second connect timeout, and a 20-second request timeout. `oauth2` 5.0 accepts `&reqwest::Client` as its async HTTP client and uses the same reqwest 0.12 line. Use `secret-service` with `EncryptionType::Dh`, never `Plain`. `mail-parser` with `full_encoding` owns RFC 2047/MIME/legacy charset decoding; use `mail_parser::decoders::html::html_to_text` only when no plain-text part exists. Bound input before parsing.

Native Arch needs only the existing compiler/pkg-config/GTK/libadwaita packages plus a running session-bus Secret Service implementation such as GNOME Keyring, KWallet, or KeePassXC. The selected network, TLS, D-Bus, and MIME crates do not require system OpenSSL or libsecret. The Flatpak SDK must contain vendored Cargo sources generated for the new lockfile because the manifest builds with `--offline`; adding dependencies without regenerating those sources is not a valid Flatpak result.

### Module and type contract

All new production items are `pub(crate)` unless used only inside their module. No protocol, HTTP, D-Bus, OAuth, GTK, or borrowed parser type may appear in `model`, `state`, `WorkerCommand`, or `WorkerEvent`.

`model.rs` owns these protocol-neutral values:

```rust
pub(crate) enum FolderId { Inbox }
pub(crate) struct MessageId(String); // "gmail:<uidvalidity>:<uid>"
pub(crate) enum MailProvider { Gmail }
pub(crate) struct AccountIdentity { provider: MailProvider, email: String }
pub(crate) struct Attachment { name: String, media_type: Option<String>, octets: Option<u64> }
pub(crate) struct Message {
    id: MessageId,
    folder_id: FolderId,
    sender: String,
    email: Option<String>,
    initials: Option<String>,
    subject: String,
    preview: Option<String>,
    received_at_unix: Option<i64>,
    body: String,
    unread: bool,
    starred: bool,
    attachments: Vec<Attachment>,
    used_fallback: bool,
}
pub(crate) struct SyncMetadata {
    completed_at: SystemTime,
    requested_limit: usize, // always 50 in this slice
    loaded_count: usize,
    fallback_count: usize,
    skipped_count: usize,
}
pub(crate) struct MailboxSnapshot { messages: Vec<Message>, metadata: SyncMetadata }
```

Keep fixed folder names/icons as projection constants rather than allocating server folders. `MessageId` and message fields are owned and cloned only into the at-most-50-row `ViewSnapshot`; they are not `Copy`. Attachment payloads never enter `Attachment`. Store timestamps as Unix seconds/SystemTime and format them in a pure view helper; do not retain `chrono`, `mail_parser`, or GLib time values in the model.

`gmail.rs` owns the sole protocol adapter value:

```rust
struct RawFetchedMessage {
    uid_validity: u32,
    uid: u32,
    flags: MessageFlags,
    internal_date_unix: Option<i64>,
    rfc822_size: Option<u32>,
    raw: Vec<u8>,             // <= 65_536 bytes
}
struct MessageFlags { seen: bool, flagged: bool }
```

`oauth.rs` owns `SecretString(Zeroizing<String>)`, `SecretBytes(Zeroizing<Vec<u8>>)`, `AuthorizationUrl(Zeroizing<String>)`, `OAuthClientConfig`, `AuthorizedGrant`, and the live authorization-attempt values. None derives `Clone` unless the flow requires one bounded copy, and all have redacted or omitted `Debug`. `OAuthClientConfig` keeps `client_id`, optional installed-app `client_secret`, and parsed endpoints private. OAuth crate token wrappers must be converted promptly into zeroizing application wrappers; acknowledge that third-party parsing and allocator moves mean zeroization is defense in depth, not proof that no copy ever existed.

The worker boundary is:

```rust
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct OperationId(u64);

pub(crate) enum WorkerCommand {
    Restore { id: OperationId },
    Connect { id: OperationId },
    Refresh { id: OperationId },
    Disconnect { id: OperationId },
    Cancel { id: OperationId },       // a new generation that supersedes the old one
    Shutdown,
}

pub(crate) enum WorkerEvent {
    Phase { id: OperationId, phase: WorkerPhase },
    AuthorizationRequired { id: OperationId, url: AuthorizationUrl, deadline: SystemTime },
    AccountPersisted { id: OperationId, account: AccountIdentity },
    NoStoredAccount { id: OperationId },
    SyncComplete { id: OperationId, account: AccountIdentity, snapshot: MailboxSnapshot },
    Disconnected { id: OperationId },
    Cancelled { id: OperationId },
    Failed { id: OperationId, failure: ServiceFailure },
}
```

`WorkerPhase` uses the phase names already listed in the plan. `ServiceFailure` contains only `FailureKind`, `retryable`, and `preserve_mail`; user copy is selected later. `WorkerCommand`, `WorkerEvent`, `AuthorizationUrl`, and `ServiceFailure` need manual redacted `Debug` implementations. Every value crossing the thread boundary is owned, `Send + 'static`, and contains no GTK object or parser borrow.

`WorkerHandle::start` creates a dedicated `std::thread` and returns a `tokio::sync::mpsc::UnboundedSender<WorkerCommand>` plus `UnboundedReceiver<WorkerEvent>`. Use unbounded channels deliberately: GTK callbacks can always send `Shutdown`/`Cancel` without blocking or losing them to a full queue, while the reducer disables duplicate actions and each operation emits only a small fixed number of events with one aggregate snapshot. Never emit per-message events. A send failure becomes `WorkerUnavailable`; it is not ignored.

Inside the thread, build a Tokio current-thread runtime with I/O and time enabled. The controller owns at most one `tokio::task::JoinHandle`, selects between the command receiver and that task, aborts and awaits the old handle before starting a superseding operation, and converts a non-cancelled join failure into `WorkerFailed`. `Shutdown` aborts/awaits the active task, performs any required bounded Secret Service cleanup, drops the event sender, and exits. The GTK close handler sends `Shutdown` but never joins. The application lifecycle owner retains the OS `JoinHandle` and definitively joins it after `application.run()` returns, so GTK close remains nonblocking while process exit cannot detach in-progress cleanup. UI actions retain only cloned command senders and weak UI references; the strong UI holder clears on window destruction to break the application/window ownership path.

Use `tokio::sync::mpsc` for the GLib handoff rather than the removed/changed GLib channel APIs. On the GTK main thread call `gtk::glib::MainContext::default().spawn_local(async move { while let Some(event) = rx.recv().await { ... } })`. Tokio mpsc receiving does not require entering a Tokio runtime. The future holds only the receiver plus `WeakUi`; widget access occurs only after upgrading on the main context.

Private test seams are generic traits, not a service locator:

```rust
trait ConfigSource: Send + Sync + 'static { /* load -> impl Future + Send */ }
trait OAuthGateway: Send + Sync + 'static { /* interactive_grant, refresh, identity */ }
trait CredentialStore: Send + Sync + 'static { /* load, replace, delete */ }
trait InboxSource: Send + Sync + 'static { /* fetch_inbox */ }
trait Clock: Send + Sync + 'static { fn now(&self) -> SystemTime; }
```

Each async method returns `impl Future<Output = Result<OwnedValue, BoundaryFailure>> + Send + '_`; production dependencies are moved into the worker thread and tests monomorphize deterministic fakes. Do not use trait objects or `async-trait`. Browser launch is intentionally not one of the worker traits: it is a GLib-local effect whose success/failure is injected back into the reducer in tests.

### OAuth and browser contract

Deserialize only this bounded Google file shape (maximum 64 KiB). Open the path once, validate that opened handle is a regular file, and read at most 64 KiB plus one byte from the same handle; reject the extra byte without a metadata/read race:

```rust
struct GoogleClientFile { installed: GoogleInstalledClient }
struct GoogleInstalledClient {
    client_id: String,
    project_id: String,
    auth_uri: String,
    token_uri: String,
    client_secret: Option<String>,
    redirect_uris: Vec<String>,
}
```

Reject unknown top-level client types, `project_id != "whitford-email"`, blank/oversized fields, a client ID not ending in `.apps.googleusercontent.com`, and a redirect list with no HTTP loopback entry. Endpoint validation is an exact allowlist after parsing with `url::Url`: authorization is either `https://accounts.google.com/o/oauth2/auth` or `https://accounts.google.com/o/oauth2/v2/auth`; token is `https://oauth2.googleapis.com/token`; userinfo is not configurable and is `https://openidconnect.googleapis.com/v1/userinfo`. Allowed URLs have no userinfo, non-default port, query, or fragment. IMAP is likewise a code constant, `imap.gmail.com:993`. Never accept merely “HTTPS on a Google-looking host.”

Resolve `google-oauth.json` through XDG rules: an absolute `XDG_CONFIG_HOME`, otherwise an absolute `HOME` plus `.config`, then `whitford/google-oauth.json`; relative/non-Unicode/missing roots fail closed. In Flatpak, use the sandbox XDG path (`~/.var/app/dev.whitford.Whitford/config/whitford/google-oauth.json`) rather than exposing host `xdg-config`. This corrects the earlier manifest proposal: do not add `--filesystem=xdg-config/whitford:ro`.

For Connect, bind `tokio::net::TcpListener` to `127.0.0.1:0` before constructing the authorization request. The redirect is exactly `http://127.0.0.1:<bound-port>/oauth/callback`. Generate `CsrfToken::new_random()` and `PkceCodeChallenge::new_random_sha256()` per attempt. Request exactly `https://mail.google.com/`, `openid`, and `email`, plus `access_type=offline` and `prompt=consent`. Do not request `profile`, do not discover arbitrary endpoints, and do not parse the ID token for identity.

The callback deadline is 180 seconds. Accept only an IPv4 loopback peer, `GET`, the exact callback path, an exact `Host: 127.0.0.1:<port>`, a request header block no larger than 8 KiB, and exactly one of `code` or `error` plus exactly one `state`; duplicate/blank fields are invalid. Wrong paths receive a static 404 and may continue waiting; a callback on the right path with a state mismatch is terminal. Responses are small static success/error HTML and never echo parameters. `url::form_urlencoded` handles decoding. Close the listener on completion, cancellation, or timeout.

The worker emits `AuthorizationRequired` after the listener exists. `ui/mod.rs` stores the redacted URL in a non-`Debug`, operation-keyed `AuthorizationPrompt` outside `AppState`, then launches it with `gtk::gio::AppInfo::launch_default_for_uri_future`, not the synchronous launcher. This is portal-aware and gives an actionable async error in Flatpak. `Reopen Browser` relaunches that same cached URL while the same operation and deadline remain active; it does not regenerate state/PKCE or ask the worker. Cancel clears the cached prompt and sends a new-generation `Cancel`. A launch failure also clears the prompt, dispatches `BrowserLaunchFailed`, and cancels the worker attempt. The cache is cleared by any terminal/superseding event.

Exchange the code with the exact redirect URI and PKCE verifier through `oauth2`. A successful first grant must contain non-empty access and refresh tokens. Fetch userinfo with bearer authentication, require non-empty `sub`, non-empty `email`, and `email_verified == true`, then discard `sub`; Workspace addresses are valid and must not be restricted to `@gmail.com`. The first-grant commit order is fixed: token exchange -> verified userinfo -> persist refresh token -> emit `AccountPersisted` -> IMAP sync. Never access IMAP or report connected before secure persistence succeeds. Cancellation is offered only while waiting for the browser; after callback, Disconnect is the cancelling cleanup path. If the app closes after persistence, restart Restore recovers the account.

Restore/Refresh loads the saved refresh token, requests a fresh access token for every sync, and does not cache access tokens between operations. If Google returns a replacement refresh token, persist it before IMAP and fail the operation if replacement cannot be committed. `invalid_grant` stops retry and attempts credential deletion: confirmed deletion enters AuthRequired, while failed deletion requires Disconnect cleanup. Transient transport/429/5xx failures keep the existing credential. Access tokens, codes, verifier, state, XOAUTH2 bytes, and token response objects are dropped/zeroized at the earliest boundary.

### Secret Service contract

Use the default collection only. Connect with `EncryptionType::Dh`; unlock the collection/item through the crate prompt-capable `unlock()` calls when locked. Search within the default collection by this complete, fixed attribute set:

```text
xdg:schema     = dev.whitford.OAuthRefreshToken
application    = dev.whitford.Whitford
provider       = gmail
kind           = oauth-refresh-token
schema-version = 1
```

Attributes and labels are not secret, so do not place email/client/token material in them. The label is exactly `Whitford Gmail authorization`; the content type is `text/plain; charset=utf-8`. `load` accepts zero matches as `None`, exactly one as the credential, and treats multiple matches as `KeyringAmbiguous` rather than guessing. Reject empty, non-UTF-8, or over-8-KiB secrets. `replace` calls `Collection::create_item(..., replace = true, ...)` with the complete attribute set. `delete` deletes every exact match so it can recover from historical duplicates and treats zero matches as success.

On first grant, verify identity before storing, then wait for `create_item` and any prompt to finish before emitting `AccountPersisted`. On Disconnect, abort network work, delete the item(s), and only then emit `Disconnected`; failure retains account/mail state. On revoked `invalid_grant`, confirmed deletion enters AuthRequired; deletion failure is cleanup-required `DisconnectFailed` and can only retry Disconnect. An indeterminate first-save result likewise requires a confirmed delete before the cleanup marker can be cleared; failed cleanup never claims that no token remains. No file, environment, GSettings, process-restart memory, or Secret Service `Plain` fallback is permitted.

### IMAP and mapping contract

Each sync performs this sequence under explicit phase timeouts: TCP connect; rustls handshake for DNS name `imap.gmail.com`; consume/validate greeting; `AUTHENTICATE XOAUTH2`; `EXAMINE INBOX`; one bounded sequence `FETCH <last-at-most-50> (UID)`; one `UID FETCH` for those UIDs; `LOGOUT`. There is no unbounded search. The XOAUTH2 authenticator returns `user=<verified email>\x01auth=Bearer <access token>\x01\x01` for the initial empty challenge and an empty response to any subsequent Gmail error challenge. `async-imap` performs base64. Never log the challenge or authenticator.

Require `Mailbox::uid_validity`. Use the `EXAMINE` message count to address only the newest at-most-50 sequence numbers, sort/deduplicate and bound the returned UIDs, and build a comma-separated numeric UID set from integers only. If the mailbox is empty, return an empty successful snapshot without either FETCH. Count expunges, missing UIDs, duplicates, unilateral results, and unusable body records as skipped. The only body fetch query is:

```text
(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])
```

Collect the `uid_fetch` stream to completion before committing. Accept only requested UIDs, ignore duplicate/unilateral fetches deterministically, require each record's UID and body, and reject a body over 65,536 bytes. Any stream/protocol error discards the candidate batch and preserves the previous snapshot; a malformed individual completed record is fallback/skipped without losing its peers. `BODY.PEEK` plus `EXAMINE` are both mandatory. There is no code path or command constant for `SELECT`, `STORE`, `COPY`, `MOVE`, `EXPUNGE`, APPEND, or non-PEEK body access.

Map `\Seen` to `unread = false`, `\Flagged` to `starred = true`, and combine `uid_validity`/UID into the opaque ID. Sort final messages by UID descending. Immediately map each `Fetch` into owned `RawFetchedMessage`, finish/drop the fetch stream and session, then parse/map the raw records; no `Fetch` borrow crosses the adapter.

`message.rs` parses the complete raw bytes with `MessageParser`. Prefer readable text derived from the first HTML alternative for previews, which preserves link labels without exposing tracking destinations; fall back to the first usable `text/plain` body, then `No readable message body.` Preserve the complete real HTML or plain-text reader body. Normalize NUL/control characters in plain text, cap sender to 160 graphemes, address to 320, subject to 512, preview to 280, attachment names to 255, and attachment metadata to 20 entries. Missing sender/subject/date/body use stable copy (`Unknown sender`, `(No subject)`, no timestamp, and the body fallback). MIME parser objects are dropped after mapping.

### Reducer, UI effects, and read-only guards

`AppState` remains the authority and contains `session: SessionState`, `active_operation: Option<OperationId>`, `next_operation: u64`, `account: Option<AccountIdentity>`, `mailbox: Option<MailboxSnapshot>`, and the existing local selection/search/filter values. `SessionState` is one enum: `Disconnected`, `Authorizing { deadline }`, `Syncing { kind }`, `Ready`, `Offline { failure }`, `AuthRequired { cleanup_failed }`, `Disconnecting`, `ConfigurationError`, and `ServiceError`. The mailbox is separate so Offline/AuthRequired can retain the last in-process snapshot.

Allocate a fresh ID for Restore, Connect, Refresh, Cancel, Disconnect, and Retry with `checked_add`; exhaustion becomes a static `WorkerUnavailable` error rather than wrapping and accepting stale work. Events whose ID is not `active_operation` are ignored before any mutation or notice. `AccountPersisted` sets account identity but not Ready; only `SyncComplete` atomically replaces mail and records the success time. A failed refresh preserves mail and the prior success time. A successful empty sync replaces it with an empty inbox.

Replace `Transition` with:

```rust
pub(crate) struct Update {
    dirty: DirtyRegions,
    notices: Vec<UserNotice>,
    effects: Vec<Effect>,
}
pub(crate) enum Effect {
    SendWorker(WorkerCommand),
    LaunchAuthorization { id: OperationId }, // resolves URL from UI prompt cache
    ClearAuthorization { id: OperationId },
    PresentDisconnectConfirmation,
}
```

Actions include startup/connect/refresh/cancel/reopen-browser/request-confirm-dismiss-disconnect, browser launch result, typed worker events (with the authorization URL intercepted/cached by the coordinator), and the existing selection/search/filter/navigation actions. Execute effects only after dropping the `RefCell<AppState>` borrow. `ViewSnapshot` projects all copy/action-enabled/status text once; widgets do not infer service state.

Read-only protection has three layers and no mode-dependent loophole:

1. Remove `ArchiveSelected` and every mutation variant from `Action`; no reducer branch changes message flags/folders or removes a message.
2. Remove accelerators for Archive/Delete, Compose, Reply, Mark Read, Star, Label, and Download. Install any still-visible `gio::SimpleAction` disabled, with controls disabled or hidden and read-only explanatory copy. Attachment download controls are hidden; metadata remains visible.
3. Expose only `FolderId::Inbox`; never construct fixture Drafts/Sent/Archive/Trash/Starred folders in production. Search/filter/selection are explicitly local and may remain enabled.

Action enablement is projected exactly: Connect only in Disconnected/AuthRequired; a ConfigurationError exposes Retry only, preserving the exact failed operation so startup configuration repair resumes saved-token Restore rather than opening browser consent; Cancel and Reopen only in an unexpired Authorizing attempt; Refresh only with a persisted account and no foreground operation; Disconnect only with a persisted account and not already disconnecting; Retry retains the exact recovery operation (Restore, Refresh, Connect, or Disconnect). Token/userinfo/IMAP authentication rejection enters AuthRequired and recovers with Connect. WorkerUnavailable is non-retryable and gives restart guidance; any command-send failure dispatches it immediately so progress cannot hang. Disconnect uses an `adw::AlertDialog`, enters progress only after confirmation, and preserves identity/mail if secret deletion fails.

### Tests, logging, and platform risks

Pure tests exercise config/URL/callback/XOAUTH2/UID/MIME/reducer functions directly. Generic fake implementations cover every orchestration boundary and record call order, including “userinfo before secret replace,” “secret replace before IMAP,” “no IMAP after persistence failure,” refresh-token rotation before IMAP, cancellation, stale IDs, channel closure, controller shutdown, and application-owned shutdown/join. A scripted `AsyncRead + AsyncWrite` stream tests greeting/SASL/error-challenge/read-only command emission without Gmail. Secret Service tests use a fake store only; GIO launch tests inject success/failure as reducer actions. The implemented suite contains 66 display-independent tests. Real browser, keyring prompt, Google, Wayland, and Flatpak behavior remain named manual acceptance checks.

Initialize `tracing_subscriber` once with an application-only level (`WHITFORD_LOG=error|warn|info|debug|trace`, default `info`); do not accept arbitrary dependency directives from `RUST_LOG`, because they can enable HTTP/IMAP internals. Emit only the allowlisted fields in Step 3d. Redaction tests format every command/event/failure plus canary secrets and content, and assert the canaries are absent.

Known dependency/platform risks are explicit:

- `async-imap` and `mail-parser` are 0.x APIs and do not declare a useful MSRV; keep their direct versions and lockfile reviewed, and compile all targets on Rust 1.98 before merge. The IMAP adapter is the only place their types may occur.
- `async-imap` exposes arbitrary command strings and mutation APIs. Keep the five read-only command literals private and test the emitted transcript.
- `secret-service` uses direct session D-Bus and can prompt; Flatpak therefore needs exactly `--talk-name=org.freedesktop.secrets`. There is no plaintext or portal-secret fallback. Native/manual tests need an actual provider, not merely the crate.
- Flatpak needs `--share=network` both for Google/Gmail and for the host browser to reach the loopback listener. GIO OpenURI uses the portal without another broad bus permission. Do not add host/home/config filesystem access.
- Use ring rather than rustls's default aws-lc provider to avoid an extra CMake-heavy provider in native/Flatpak builds. HTTP and IMAP still have separate client configurations but one rustls implementation and one compiled root set.
- Complete `BODY.PEEK[]` avoids reader truncation. A later efficiency pass should fetch structure/list metadata first and complete body parts on open so attachments are not transferred eagerly.

Primary API references used to lock this design are the `async-imap` 0.11.3 `Client`/`Session` APIs, `oauth2` 5.0's stateful reqwest and redirect guidance, `secret-service` 5.2's async collection/item APIs, GIO's async default-URI launcher, Google's desktop loopback/PKCE and Gmail XOAUTH2 documentation, and the Freedesktop Secret Service attribute/locking specification.

### Architecture user challenge

None. The earlier `--filesystem=xdg-config/whitford:ro` proposal is deliberately removed: Flatpak rewrites `XDG_CONFIG_HOME` to the app sandbox, so placing the developer credential in the documented per-app path is both simpler and less permissive. Reopen Browser is likewise a GTK-local replay of the cached URL rather than a worker command; the worker remains the sole owner of the listener/state/PKCE attempt.
