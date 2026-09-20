# Feature: Performance-first lazy message loading
**Date:** 2026-09-20
**Branch:** feature/gmail-imap-integration
**Stack:** Rust 2024, GTK4/libadwaita, Tokio, async-imap, WebKitGTK
**Status:** In progress (automated)

## Problem
Whitford currently downloads, parses, clones, serializes, and renders every complete RFC 5322 message during inbox sync. That makes a 100–500 message retention choice misleading and creates avoidable network, CPU, memory, disk-write, and GTK rendering costs. Users need a fast list immediately, complete content when they open a message, and transparent control over locally retained data.

## Solution
Sync the selected number of lightweight summaries using IMAP header fields rather than complete bodies. Fetch one complete body with `BODY.PEEK[]` only when selected, show an explicit reader loading/error state, persist successfully opened bodies separately, and expose cache disk usage plus a safe Clear Cache action. Keep every blocking/network operation on the worker thread and preserve the read-only IMAP and hardened HTML boundaries.

## User Story
As a Whitford user, I want hundreds of message summaries to appear quickly and full content to load only on demand so that the client stays responsive and predictable on a lightweight Wayland desktop.

## Scope: IN
- Sync 50/100/250/500 lightweight INBOX summaries according to the existing retention setting.
- Fetch a complete message only when opened, with loading, retry, stale-result, offline, and missing-message handling.
- Cache opened bodies separately and reuse them across selection and restart.
- Show local cache byte usage and add a confirmed Clear Cache action that preserves authorization and preferences.
- Avoid full-list rerenders for body-only state transitions where practical.
- Migrate/ignore the current v1 whole-message cache safely without exposing or deleting credentials.

## Scope: OUT
- Separate attachment download/save UI, compose/reply, mutations, folders, pagination beyond the selected newest-N window.
- A database dependency; bounded private JSON files are sufficient for the MVP.
- Flatpak verification and live GUI automation.

## Existing Code to Touch
- `src/gmail.rs`: split summary fetch from single-UID complete-body fetch.
- `src/message.rs`: split bounded header summary mapping from full MIME body mapping.
- `src/model.rs`: represent body availability without placeholder content.
- `src/cache.rs`: summary index, per-message body cache, usage, pruning, clearing, private atomic writes.
- `src/worker.rs`: independent body requests and non-blocking cache maintenance alongside account sync.
- `src/state.rs`: body request IDs/state, retry, stale result rejection, clear-cache flow.
- `src/ui/{build,mod,render,actions}.rs`: loading/error reader and cache management UI.
- `README.md` and test manifest: behavior, privacy, and acceptance coverage.

## Edge Cases to Handle
- Selection changes while a body request is in flight; stale completion must not replace the current reader.
- Reopening a cached message must not use network or recreate body-loading state.
- Missing/expunged UID, UIDVALIDITY mismatch, timeout, expired authorization, corrupt cache, and empty inbox.
- Clearing during a body request must prevent the completed body from being retained or shown unexpectedly.
- Retention changes must prune summaries and orphaned body files.
- Cache size and cache IO failures must have honest UI without blocking message reading.

## Test Scenarios
- Summary query contains header fields and never `BODY.PEEK[]`; selected count drives newest UID discovery.
- Single-message query uses the exact validated UID and `BODY.PEEK[]` without setting Seen.
- Selection emits one body request, cached selection emits none, stale results are ignored, failure is retryable.
- Summary merge preserves cached bodies for matching IDs; retention and clear remove orphaned bodies.
- Cache usage reports index plus body bytes; files/directories remain mode 600/700.
- 500-message sync path performs bounded summary parsing and no full MIME body parsing.

## Assumptions
- Automated mode continues from the user’s earlier instruction.
- Performance takes priority over showing snippet previews before a body has ever been opened; list rows may omit preview text until body content exists.
- The current feature branch is retained because this is the next increment of the same unpushed Gmail feature.
- Cache bodies are private but not encrypted at rest; tokens remain only in Secret Service.

## Context
- The project is a single native Rust desktop repository with a pure reducer, a dedicated Tokio worker thread, and GTK-only main-thread mutation.
- Current bottlenecks are full `BODY.PEEK[]` list sync, whole-cache JSON rewrites, full message cloning, and rebuilding message rows/WebViews on every render.
- Existing HTML hardening in `src/ui/email_view.rs` is preserved unchanged.
- Verification commands are `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release`.

## Scope

**Tier:** single-layer

**Reasoning:** One native desktop application and repository, but the feature crosses its IMAP, persistence, reducer, and GTK boundaries and adds a user-facing cache-management surface.

**Subagents to spawn:** product review lenses, technical planner, test runner, and review passes. Rust implementation stays with the orchestrator because the bundled stack architects do not cover Rust/GTK.

**Skills to invoke:** autofeature orchestration; security and performance review are folded into the native review because no standalone security-review/simplify skill is available.

## Effort Plan

**Profile:** forced:medium (user requested low/faster; autofeature floor is medium)
**Rules fired:** none; existing OAuth behavior is reused rather than redesigned.

| Task | Effort | Why |
|------|--------|-----|
| Context scan | medium | floor/base |
| Product review lenses | medium | floor/base |
| Plan | medium | forced profile |
| Implementation | session | native Rust orchestrator work |
| Test runner | medium | floor/base |
| Review passes | medium | forced profile |

**Escalations during run:** none.

## Product Review

The feature is the right next increment for Whitford's lightweight-native promise, but the review found several flow requirements that must be part of the build rather than deferred. There is no product challenge requiring user intervention; the findings sharpen the implementation contract.

### In-scope findings
- **Critical:** body requests need an operation lane independent from sync/auth/disconnect, with disconnect/clear invalidation preventing late cache writes.
- **High:** sync membership must be authoritative for the newest-N result so expunged mail, empty inboxes, and UIDVALIDITY rollover do not retain stale rows or bodies.
- **High:** the Attachments filter must use lightweight `BODYSTRUCTURE`; unknown must never silently mean no attachment.
- **High:** list highlight and deliberate reader-open intent must be distinct so startup and filtering do not download bodies automatically.
- **High:** reader-local Retry/Reconnect/Missing outcomes must be separate from mailbox-level Retry.
- **High:** increasing retention must trigger a summary refresh; decreasing it must prune summaries and orphaned bodies.
- **High:** cache wording must distinguish retained summaries from opened bodies available offline. Clear Cache will remove downloaded bodies while retaining the summary index, authorization, and preference; it must invalidate in-flight writes and report reclaimed space.
- **Medium:** list and reader rendering need revision guards so body transitions do not rebuild 500 rows or recreate an unchanged WebView.
- **Medium:** expose body-cache bytes and count; enforce a bounded body-cache byte budget with least-recently-used eviction while never truncating a displayed message.

### Explicit tradeoffs
- Unopened rows are a compact sender/subject/date list; search is labelled and implemented as sender/subject search until a body is loaded. No misleading “No preview” filler is rendered.
- Full readable MIME text/HTML parts are fetched without truncation. The current IMAP library requires a complete RFC 5322 fetch when a message is opened, so attachment payloads inside that message are transferred too; list sync still uses `BODYSTRUCTURE` only and transfers no attachment payloads.
- Performance acceptance covers query shape, no full-body parsing during list sync, revision-gated rendering, immediate cached-body reuse, and representative local benchmarks. Network wall-clock budgets are recorded manually because Gmail latency is not deterministic in unit tests.

### Fast follows
- A separate opt-in “Download for offline reading” policy.
- Attachment content download and saving.
- Persistent IMAP connection/prefetch only if measured open latency warrants the added complexity.

## Implementation Plan

### Scope challenge

The smallest coherent change is to keep the existing reducer, Tokio worker, OAuth/Secret Service flow, IMAP connection helper, MIME parser, hardened WebKit view, and XDG atomic-write helpers, while splitting the current whole-message value into summary and body values. No database, persistent IMAP session, speculative prefetch, attachment payload fetch, or new service/module is required. The change necessarily crosses the nine files named in the brief plus documentation/tests; that exceeds the usual eight-file complexity warning, but these are existing boundaries rather than new abstractions. Creating a parallel repository/service would add more state and failure modes.

Defer persistent IMAP connections and token refresh-on-body-open until first-open latency is measured. For this increment, retain the most recent verified access token in zeroizing worker memory after connect/restore/refresh; a body fetch after expiry reports the existing authorization failure and offers Reconnect. Also defer preview/snippet fetching: sender/subject/date search remains explicit, and list rows render no fake preview.

One scope correction is required for correctness: summary sync is an authoritative replacement for the newest `limit` UIDs, not the current merge-with-old-cache behavior. Otherwise expunged messages, UIDVALIDITY rollover, and retention increases leave stale rows and bodies indefinitely. Clear Cache removes opened-body files only and keeps summary metadata, authorization, and preferences, matching the Product Review wording.

### What already exists

- `src/gmail.rs` already establishes read-only XOAUTH2 sessions with `EXAMINE`, bounded sequence-to-UID discovery, timeouts, and skipped-response classification. Generalize the limit and split the fetch query; do not add another IMAP client.
- `src/message.rs` already owns bounded header/MIME normalization and HTML-to-readable-text conversion. Split it into pure summary and body mappers and retain the existing hardening/caps.
- `src/cache.rs` already resolves XDG paths and performs private atomic JSON writes. Reuse one generalized atomic writer for the summary index, body manifest, and body files.
- `src/worker.rs` already owns all network/filesystem/keyring work off the GTK thread and uses typed commands/events. Add a separately cancellable body lane rather than a second worker thread.
- `src/state.rs` already rejects stale account-operation events using IDs. Apply the same rule independently to body/cache operations and add revision counters.
- `src/ui/email_view.rs` remains unchanged; a loaded `MessageBody` continues through the same ephemeral, hardened WebKit boundary.

### Architecture and data flow

```text
Refresh/connect
  retention limit -> EXAMINE -> newest sequence FETCH(UID)
  -> one UID FETCH(summary query) -> Vec<RawMessageSummary>
  -> map_summaries -> authoritative MailboxSnapshot<MessageSummary>
  -> atomically write mailbox-v2.json -> GTK event -> list_revision++

Deliberate open/retry
  MessageId -> validated (UIDVALIDITY, UID) -> state body request ID + cache generation
  -> body worker lane -> body-cache lookup
       hit: read one body file -> touch small LRU manifest -> BodyLoaded(Cache)
       miss: exact UID FETCH(BODY.PEEK[]) -> map_body -> epoch check
             -> atomic body file + manifest/prune -> BodyLoaded(Network)
  -> reducer accepts only current request + generation + selected/opened ID
  -> reader_revision++ (list_revision unchanged) -> render reader only

Clear Cache
  confirmation -> generation++ in reducer -> worker raises shared epoch, aborts/joins body task
  -> remove body directory + recreate private manifest -> CacheCleared
  -> clear in-memory bodies/body request; summary list and preferences remain
```

The account-operation lane (`Restore`, `Connect`, `Refresh`, `Disconnect`) keeps its existing supersession semantics. The body lane has at most one task: a new deliberate open cancels the prior body task without cancelling sync/auth; sync does not cancel a body read; disconnect and Clear Cache invalidate and abort it. Cache maintenance remains on the worker thread. Do not hold a GTK object, `RefCell` borrow, raw MIME buffer, or WebKit object across either channel.

### Exact model changes

- Replace `Message` with serializable `MessageSummary { id, folder_id, sender, email, initials, subject, received_at_unix, unread, starred, attachment_state, used_fallback }`. Remove `body`, `html_body`, and body-derived `preview`. Use `AttachmentState::{Known(Vec<Attachment>), Unknown}` so missing/malformed `BODYSTRUCTURE` is not treated as “no attachments”; the Attachments filter includes `Unknown` (honest false positives are preferable to silent false negatives) and rows can label the status unavailable.
- Add serializable `MessageBody { text: String, html: Option<String>, attachments: Vec<Attachment>, used_fallback: bool }`. Store it in reducer memory as `Arc<MessageBody>` so snapshot/render handoff never clones full body/HTML strings. The full MIME mapper may refine attachment metadata from the complete message.
- Add `MessageId::gmail_parts() -> Option<(u32, u32)>`, accepting exactly `gmail:<nonzero u32>:<nonzero u32>` with no trailing components. Only these parsed integers reach an IMAP command.
- Change `MailboxSnapshot.messages` to `Vec<MessageSummary>`. `SyncMetadata.fallback_count` describes summary-header fallbacks only; body fallback is reader-local.
- Add reducer-only `ReaderState::{Closed, NotLoaded { id }, Loading { id, request_id }, Loaded { id, body: Arc<MessageBody> }, Failed { id, kind, retryable }, Missing { id }}`. Keep list highlight (`selected_message_id`) separate from deliberate reader state so startup normalization/filtering never downloads mail. A click or keyboard next/previous is deliberate and opens the selected item; internal normalize alone is not.
- Add `CacheUsage { body_bytes: u64, body_count: usize, available: bool }`, `list_revision: u64`, and `reader_revision: u64` to `ViewSnapshot`. Cache I/O failure sets `available = false` without hiding a successfully fetched body.

### Exact IMAP changes

- Replace the global fixed limit with `fetch_inbox(email, access_token, limit)` after validating `limit` against `RETENTION_OPTIONS`. Make `newest_sequence_set(message_count, limit)` and `discovered_uids(expected, limit, ...)` bounded by that value (maximum 500).
- Set `SUMMARY_FETCH_QUERY` exactly to `(UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE MESSAGE-ID)])`. It must not contain `BODY.PEEK[]`, `BODY[TEXT]`, or an unrestricted header/body fetch. Read header bytes with `Fetch::header()` and traverse `Fetch::bodystructure()` only for bounded attachment metadata (maximum 20 entries and existing string caps).
- Add `fetch_body(email, access_token, uid_validity, uid)`. It opens a read-only session, `EXAMINE INBOX`, rejects a different mailbox `UIDVALIDITY` as `GmailError::MailboxChanged`, and executes exactly `UID FETCH <u32> (UID BODY.PEEK[])`. Accept exactly one response whose UID matches and whose `Fetch::body()` exists; zero responses become `GmailError::MessageMissing`, duplicates/mismatches/bodyless responses are protocol errors. Always best-effort `LOGOUT`; never use `SELECT`, `STORE`, `COPY`, `MOVE`, `EXPUNGE`, `APPEND`, or a non-PEEK body section.
- Rename the existing raw type to `RawMessageSummary { uid_validity, uid, flags, internal_date_unix, rfc822_size, header, attachment_state }`; add `RawMessageBody { uid_validity, uid, raw }`. `map_summary` parses only the bounded header; `map_body` alone invokes full MIME parsing.
- Keep the current 30-second overall timeout for summary sync and apply a distinct 30-second timeout to one body fetch. Do not retry IMAP automatically: Retry is reader-local and user initiated, preventing duplicate traffic during provider/rate-limit failures.

### Exact worker protocol

- Add opaque `BodyRequestId(u64)` and `CacheOperationId(u64)` types; do not reuse the account `active_operation` slot.
- Add commands `FetchBody { request_id, generation, account_email, message_id, uid_validity, uid }`, `ClearBodyCache { operation_id, generation }`, and change `SetCacheLimit` to carry an operation ID so failures are acknowledged. Debug output may print IDs/counts but never email, UID, subject, body, paths, or tokens.
- Add events `BodyLoaded { request_id, generation, message_id, body: Arc<MessageBody>, source: BodySource, usage }`, `BodyFailed { request_id, generation, message_id, failure: BodyFailure }`, `CacheCleared { operation_id, generation, reclaimed_bytes }`, `CacheMaintenanceFailed { operation_id, operation, usage_available: false }`, and `CacheUsageChanged(CacheUsage)`. `BodyFailure` exhaustively distinguishes `Offline`, `TimedOut`, `AuthorizationExpired`, `MailboxChanged`, `Missing`, `Protocol`, and `CacheRead`; a cache-read error falls through to network when authorized and is logged, rather than making mail unreadable.
- On successful authorization/refresh, place the access token plus verified account email in a zeroizing in-memory worker auth slot. A body command must match that account. Clear it on disconnect/shutdown/auth expiry. Body fetch never reads or exposes the refresh token itself.
- Maintain `Arc<AtomicU64>` cache generation. Clear increments/stores the generation before aborting and awaiting the body task, then clears files. A body task checks the generation immediately before persistence and event send. Thus an old completion is either written before clear (and then deleted) or rejected after clear; it cannot repopulate the cache.
- Cache hits complete without IMAP and reopening an in-memory `Loaded` body emits no worker command at all. Selecting a different cached-on-disk body may briefly enter Loading while the worker reads one file, but never opens the network.

### Cache files, migration, and LRU budget

- Write summary schema v2 to `${XDG_CACHE_HOME}/whitford/mailbox-v2.json`; it contains account email, sync metadata, and summaries only. Ignore `mailbox-v1.json` on load (no deserialization into the new model), leave it untouched until successful v2 sync, then remove it best-effort. Disconnect may remove both. Never touch Secret Service or preferences during migration/Clear Cache.
- Store bodies under `${XDG_CACHE_HOME}/whitford/bodies-v1/`: `manifest.json` plus one `<uidvalidity>-<uid>.json` per body. A body file contains `version`, exact `MessageId`, and `MessageBody`; the small manifest contains `{id, file_name, bytes, last_accessed_unix}`. Validate manifest filenames as generated basenames (no separators), file IDs against the request, and JSON versions before use.
- Keep directory mode `0700` and every JSON/temp file `0600`. Use same-directory temp + flush + `sync_all` + rename. Commit the body file before the manifest; an orphan is safe and pruned during the next reconciliation. Never let a manifest entry point outside `bodies-v1`.
- Set `BODY_CACHE_BUDGET_BYTES = 128 * 1024 * 1024`, counting actual body-file lengths plus `manifest.json` and `mailbox-v2.json` in displayed usage. After a successful write or retention change, remove least-recently-used entries by `last_accessed_unix` until body files are within the body budget. Exclude the just-loaded/currently displayed ID from that prune pass; if that single body exceeds the budget, keep it as the sole entry and report its real size rather than truncate it.
- Update only the small manifest on a disk-cache hit. Reconcile missing/corrupt entries, temp files, bodies not present in the authoritative summary set, UIDVALIDITY/account changes, and over-budget entries in one O(n log n) pass with `n <= 500`; body JSON is not opened during ordinary usage calculation. Retention decreases prune orphan bodies; increases trigger a summary refresh instead of inventing older rows.
- Clear Cache removes `bodies-v1` and recreates an empty private manifest, then reports reclaimed bytes. Summary-index errors are recoverable as a cache miss; body write/prune/usage errors are surfaced but never discard readable in-memory mail.

### State and UI revision gating

- `SelectMessage`/next/previous updates list highlight and deliberately opens the message. If the same ID is already `Loaded`, reuse it with no effect. Otherwise allocate a body request, set reader Loading, and emit `FetchBody`. Filter/search normalization may change highlight but closes the reader or preserves it only if still visible; it never emits a fetch.
- Accept body events only when `(request_id, generation, message_id)` matches current reader state and the opened ID is still current. Missing/stale events do nothing. Clear, disconnect, mailbox replacement/UIDVALIDITY change, and a new open invalidate the tuple before sending commands.
- Reader Retry allocates a new body request only for its failed current ID. Offline shows “Not downloaded — reconnect to read”; missing shows “Message is no longer in Inbox” and suggests Refresh; auth failure offers Reconnect; cache-write failure shows the fetched content plus a toast that it will not be available offline.
- Add a confirmed `win.clear-cache` action, cache bytes/count label, and clear progress/feedback in the folder pane. Confirmation explicitly says opened bodies are removed while message summaries, account authorization, and the Keep locally preference remain.
- Add `last_list_revision` and `last_reader_revision` cells to `Ui`/`WeakUi`. `render_messages` runs only when list revision changes; `render_reader` runs only when reader revision changes. Body Loading/Loaded/Failed and cache-usage-only events must not clear/recreate 500 rows. A list-only change must not recreate an unchanged WebView. Continue rendering cheap action/banner/sync controls independently.
- Change search placeholder/accessibility copy to “Search sender or subject”. Remove preview filler. Show a small spinner/skeleton for Loading, a reader-local retry action for retryable failure, and the existing hardened HTML/plain renderer only for Loaded.

### Files to modify

- `src/model.rs`: summary/body/attachment-state/cache-usage types and strict Gmail ID parsing.
- `src/gmail.rs`: parameterized summary fetch, exact body fetch, UIDVALIDITY/missing errors, BODYSTRUCTURE normalization, query-contract tests.
- `src/message.rs`: pure bounded `map_summary`/`map_body` paths; prove full MIME parsing is unreachable from summary mapping.
- `src/cache.rs`: v2 authoritative summaries, separate body files/manifest, private atomic I/O, reconciliation, usage, LRU prune, clear, and v1 migration.
- `src/worker.rs`: in-memory auth slot, independent body task lane, cache epoch/commands/events, explicit error mapping and structured timings.
- `src/state.rs` and `src/state/tests.rs`: reader/cache state machines, operation IDs, deliberate-open behavior, stale gating, revision counters, and reducer tests.
- `src/ui/build.rs`, `src/ui/mod.rs`, `src/ui/render.rs`, `src/ui/actions.rs`: cache controls/confirmation, reader states, honest search/filter copy, and revision-gated rendering. Preserve `src/ui/email_view.rs` unchanged.
- `README.md` and `.autofeature/tests/gmail-imap-integration-2026-09-20.md`: lazy-loading/cache privacy behavior and automated/manual acceptance.

### Files to create

- None. Keeping cache policy in `cache.rs` and body behavior in the existing model/worker/state boundaries is simpler and directly unit-testable. Split `cache.rs` later only if implementation exceeds a maintainable size after tests.

### Implementation order

1. Change model types and split the pure summary/body mapping functions; update fixtures before adding I/O.
2. Parameterize newest-N discovery, implement/query-test summary fetch, then implement/query-test exact single-UID body fetch.
3. Replace cache v1 reads with v2 summary storage; add body manifest/files, reconciliation, usage, clear, and deterministic LRU functions using injectable roots/time for tests.
4. Add worker auth slot, independent body lane, generation barrier, cache acknowledgements, and exhaustive error mapping.
5. Extend reducer with deliberate reader open/retry/clear flows and revision counters; complete stale/empty/error tests before GTK changes.
6. Build cache UI and reader states, then add UI revision guards and search/filter copy.
7. Update README/test manifest; run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release`, followed by native manual first-open/cache-hit/restart/clear/offline checks.

### Unit test plan (happy / nil / empty / error)

- `MessageId::gmail_parts` (`src/model.rs`): valid maximum/boundary IDs round-trip; empty/missing/zero/non-numeric/overflow/trailing components return `None`; no input can become command text.
- `map_summary` / `attachment_state_from_bodystructure` (`src/message.rs`): decoded headers and nested attachments map with caps; nil date/email and absent structure become safe optionals/`Unknown`; empty header gets bounded fallbacks; malformed header/structure never panics and marks fallback/unknown. A sentinel large MIME body is not accepted by the summary API.
- `map_body` (`src/message.rs`): plain, HTML, multipart, and attachments map fully; absent readable part gets the existing fallback; empty body is explicit fallback; malformed MIME remains bounded and safe.
- UID discovery/query builders (`src/gmail.rs`): each retention option returns newest sorted unique UIDs and exact allowed query; zero-message mailbox skips UID fetch; empty response counts skipped; invalid limit/UID is rejected. Assert summary transcript excludes full body and mutation verbs, while body transcript contains one numeric UID and `BODY.PEEK[]` only.
- Summary/body response classifiers (`src/gmail.rs`): expected records succeed; nil UID/header/bodystructure is classified honestly; empty exact-body result is Missing; duplicate, UID mismatch, bodyless response, UIDVALIDITY mismatch, and protocol/timeout errors map distinctly.
- Summary cache (`src/cache.rs`): v2 round-trip replaces membership and truncates; missing file is `None`; empty mailbox persists/loads; corrupt/wrong-version/I/O errors become cache miss plus logged context without deleting preferences. v1 is ignored and removed only after successful v2 write.
- Body cache/LRU (`src/cache.rs`): write/read/touch round-trip, cached empty-but-valid body round-trips, absent entry is a miss, corrupt ID/path/JSON is quarantined as a miss, injected write/rename/remove/permissions failures are returned. Deterministic fixtures prove exact byte accounting, orphan pruning, oldest-first eviction, current-body protection, and a single over-budget body.
- Worker controller (`src/worker.rs`): cache hit emits Loaded without network, miss fetches once, nil auth fails explicitly, empty/missing remote maps Missing, and network/cache errors follow the rescue map. New body requests supersede only body work; sync does not; clear/disconnect invalidate and prevent late persistence/events.
- Reducer reader flow (`src/state/tests.rs`): deliberate uncached open emits once; already-loaded reopen emits none; startup/filter normalization emits none; Loading -> Loaded is reader-only; nil selection closes safely; empty mailbox has no request; retry is local; stale request/generation/ID events are ignored; clear during flight cannot show/retain completion; retention increase requests refresh and decrease prunes/normalizes.
- Revision helpers/UI-independent snapshot tests (`src/state/tests.rs`): search/filter/sync increments list revision; selection/body changes reader revision; body/cache usage never increments list revision; unchanged reader ID/body never increments reader revision. GTK/WebKit rendering remains native-manual because no display-independent harness exists.

### Error and rescue map

| Codepath | Failure | Rescue | User sees |
|---|---|---|---|
| summary IMAP sync | offline/timeout/provider/protocol | retain prior v2 summaries; existing manual Retry | stale/offline banner with last sync |
| summary membership | empty INBOX or UIDVALIDITY change | atomically replace with empty/new generation; prune old bodies | empty inbox, never stale old rows |
| body cache lookup | missing | fetch exact UID from IMAP | brief reader loading state |
| body cache lookup | corrupt/unreadable | log ID + operation (no content/path), remove/quarantine entry best-effort, try network | body if network succeeds; otherwise retryable cache/network message |
| body IMAP fetch | offline/timeout | retain summary and any prior loaded body; reader-local Retry | “Not downloaded” with Retry/Reconnect as applicable |
| body IMAP fetch | auth rejected/no live token | invalidate worker auth; no retry loop | Reconnect action |
| body IMAP fetch | UIDVALIDITY changed | reject body, trigger/offer summary Refresh | mailbox changed message |
| body IMAP fetch | UID absent/expunged | do not cache; mark reader Missing | message no longer in Inbox |
| body parse | malformed/no readable part | bounded safe fallback, never raw markup | fallback-content notice |
| body cache write/prune | permission/full disk/rename/remove | still deliver fetched body in memory; mark usage unavailable; retry maintenance only on next write/clear | readable message plus “not saved offline” toast |
| retention persistence | invalid value/I/O | reject/restore prior setting; no silent dropdown success | cache-setting error toast |
| Clear Cache | abort/remove/recreate failure | generation remains advanced, never accept old completion; report partial failure and recompute usage when possible | cache could not be fully cleared; Retry Clear |
| worker panic/channel close | either lane unavailable | leave spinner immediately; retain summaries/loaded body | existing restart guidance |

No catch-all may silently convert an I/O failure to success. Best-effort logout/temp cleanup may ignore its return only after the primary result is fixed and a contextual warning is emitted.

### Shadow paths

| Data flow | Happy | Nil | Empty | Error |
|---|---|---|---|---|
| retention -> UID summary fetch -> snapshot | newest N summaries replace cache | missing UID/header fields skipped/fallback-counted | zero EXISTS writes empty authoritative snapshot | retain old snapshot and expose sync failure |
| BODYSTRUCTURE -> attachment state | bounded metadata is Known | absent structure is Unknown | valid no-part structure is Known(empty) | malformed is Unknown + contextual warning |
| deliberate ID -> body cache/network -> reader | cached/network body loads | no selection/invalid Gmail ID emits nothing | remote zero rows becomes Missing | reader-local typed failure and Retry/Reconnect |
| fetched body -> atomic cache -> LRU | file+manifest committed and usage updated | no cache root is created privately | empty valid text persists; zero entries is zero usage | body still displayed; offline retention warning |
| Clear Cache -> generation barrier -> filesystem | bodies removed and reclaimed bytes shown | no body directory succeeds idempotently | empty cache reports zero reclaimed | partial failure is visible; old events remain invalid |
| revisions -> GTK render | only dirty surface rebuilds | absent reader renders neutral state | empty list renders empty state once | failed reader rebuild does not churn list/WebView |

Every cell above receives a unit test at its pure classifier/reducer/cache boundary; live Gmail timing and GTK rendering receive named manual acceptance rather than mocked success claims.

### Performance and observability checklist

- [ ] A 500-message sync performs two bounded IMAP commands after `EXAMINE`, transfers only selected headers/BODYSTRUCTURE, invokes `map_body` zero times, and keeps no full raw messages after mapping.
- [ ] UID membership/classification is O(n log n) with `n <= 500`; mapping/filtering is O(n); cache reconciliation/LRU is at most O(n log n); there is no per-row network or filesystem query.
- [ ] Body open performs one manifest lookup and at most one exact-UID network fetch; memory holds summaries plus only currently/recently loaded reducer bodies, bounded by the same 128 MiB policy (evict non-current in-memory bodies alongside disk LRU).
- [ ] No filesystem, MIME parse, OAuth, DNS/TLS, IMAP, or cache-size traversal runs on the GTK main thread.
- [ ] Body-only and usage-only events leave `list_revision` stable; list-only events do not recreate an unchanged WebView. Add debug assertions/tests around both invariants.
- [ ] Emit structured `tracing` spans for `summary_sync`, `body_open`, `body_cache_lookup`, `body_cache_write`, `body_cache_prune`, and `cache_clear`, recording duration, requested limit, result category, source (cache/network), counts, and bytes only.
- [ ] Never log account email, access/refresh token, OAuth URL, UID/message ID, sender, subject, body, attachment filename, search text, or full cache paths. Errors carry operation/category and safe counts sufficient to diagnose.
- [ ] Log cache corruption, write/prune failure, stale-event rejection (debug), UIDVALIDITY mismatch, timeout, and worker task panic once with context; no silent catch blocks or per-message success spam.
- [ ] Record representative local release-build benchmarks for mapping 500 summaries, serializing the v2 index, body-cache hit, and pruning 500 manifest entries. Record live first-open and cache-hit latency manually; do not gate tests on Gmail wall-clock time.
- [ ] Verify actual cache usage against `du`/file metadata, modes `0700/0600`, atomic temp cleanup, 128 MiB pruning, no Seen mutation, and no v1 credential/preference deletion.

### NOT in scope

- Persistent/pipelined IMAP sessions, background prefetch, pagination, full-text search before open, encrypted-at-rest bodies, or a database: each adds complexity without being required for fast summaries and exact on-demand reads.
- Attachment payload download, message mutation, compose/reply, and multi-folder/multi-account cache partitioning.
- Flatpak or automated GTK/WebKit UI verification; native manual acceptance remains explicit.

### User Challenge

None. The brief and Product Review resolve the only ambiguous behavior: Clear Cache removes opened bodies but retains summaries, authorization, and preferences; the 128 MiB LRU budget is a reversible implementation default and can be tuned after measurement.
