# Feature: Always-on Mail Sync

**Date:** 2026-09-21  
**Branch:** feature/unified-mail-actions (continuation)  
**Stack:** Rust 2024 · GTK4/libadwaita · Tokio · Gmail IMAP  
**Status:** Draft

## Problem

Whitford only learns about incoming mail when opened or manually refreshed. That makes cached reading fast but makes it unreliable as a daily desktop mail client: new mail does not appear or notify the user until they intervene, and temporary network failures have no bounded reconnect schedule.

## Solution

Add an incremental, Inbox-first background synchronization loop with bounded polling, exponential backoff with jitter, and native desktop notifications. The GTK main loop schedules ticks only; all cache, token, and IMAP work remains in the worker. Sync compares stable Gmail `X-GM-MSGID` identities against a durable prior Inbox snapshot, emits a privacy-safe count/preview notification only for genuinely new unread messages, and never replaces visible state or blocks actions unexpectedly.

## User Story

As a Whitford user, I want new unread Gmail to appear and notify me while the app is open, recovering quietly after temporary network failures, so I can rely on it throughout the day.

## Scope: IN

- App-lifetime background Inbox synchronization after a ready session.
- Bounded polling with exponential reconnect backoff and bounded jitter; no busy loops.
- Cache-first, worker-only Gmail/token/database work.
- Stable-ID new-message detection with an initial-baseline guard, duplicate suppression, and unread-only desktop notifications.
- Native `gio::Notification` delivery through the registered GTK application, with privacy-safe content by default.
- State/UI sync status that distinguishes background activity/errors from a foreground manual refresh.
- Unit/integration tests for scheduling decisions, new-message detection, backoff, reconnect, duplicate events, offline state, and no notification on initial load.

## Scope: OUT

- IMAP IDLE/push transport, background sync when the app is not running, and OS notification-policy configuration.
- Thread aggregation, multi-account, generic IMAP, notification click-to-message navigation, and message content previews.

## Existing Code to Touch

- `src/state.rs`: background tick action, baseline/dedup state, notification effects, refresh isolation.
- `src/worker.rs`: background sync command, timed transport work, backoff event/outcome, cache-first folder synchronization.
- `src/cache.rs`: durable last-seen Inbox identities/notification watermark if needed for restart-safe suppression.
- `src/ui/mod.rs`, `src/ui/render.rs`, `src/main.rs`: GLib timer lifecycle, native notification execution, subtle sync state.
- `src/model.rs`: bounded notification/sync outcome types only if required.

## Edge Cases to Handle

- Startup restore must set a baseline without notifying old unread mail.
- A manual refresh and tick cannot issue competing IMAP work or duplicate notifications.
- Offline, authorization failure, token refresh failure, and worker shutdown stop/retry without UI blocking.
- New mail arrives while a folder/search/body request is in progress.
- Reconnect adds multiple messages at once, duplicate worker events arrive, or IDs disappear after label changes.
- Notification delivery unavailable/disabled by the desktop must not fail mail sync.

## Test Scenarios

- Initial load: no notification; later unseen unread ID: one notification and Inbox update.
- Read-only/seen new mail: no notification.
- Consecutive failures: increasing bounded delay; success resets delay; manual refresh remains immediate.
- Tick during foreground work: coalesces rather than interrupting or restarting it.
- Cached body open and virtualized 500-row list retain existing performance behavior during a sync event.

## Assumptions

- A conservative 60-second polling floor is appropriate until IDLE is introduced; users should not need a setting for the first safe version.
- Notifications show only a count (for example, “3 new unread messages”) to avoid leaking subjects/senders onto the lock screen.
- Current open PR is intentionally extended because this is the immediately following foundation milestone and has not merged.

## Scope

**Tier:** cross-stack (native state/cache, worker transport, GTK lifecycle/notifications)

**Reasoning:** The feature crosses state scheduling, asynchronous worker/cache behavior, and desktop UI integration.

**Subagents:** worker/cache planner, state planner, GTK notification/performance planner — all `gpt-5.6-terra`.

**Skills:** automated planning, direct Cargo verification, pre-ship review. Product review is skipped because this milestone comes directly from the just-completed product roadmap.

## Effort Plan

**Profile:** forced:medium (all delegated work uses `gpt-5.6-terra` by user request)
**Rules fired:** none

| Task | Model | Effort | Why |
|---|---|---|---|
| Worker/cache design | gpt-5.6-terra | medium | asynchronous mail transport |
| State design | gpt-5.6-terra | medium | user-visible scheduling and dedup |
| GTK design | gpt-5.6-terra | medium | lifecycle and native notifications |
| Implementation/review | gpt-5.6-terra | medium | user-requested model |

**Escalations during run:** none

## Implementation Plan

Keep the existing foreground account lifecycle (`Restore`/`Connect`/`Refresh`) intact. Background polling is a separate, narrow path; it must never be expressed as `Refresh`, must not use IMAP IDLE, and must not turn the current `active_operation` into a global scheduler rewrite.

### Contract and ownership

- Add `BackgroundSyncRequestId(u64)`, `WorkerCommand::BackgroundSync { request_id, account_email }`, and two background-only events: `BackgroundSyncComplete { request_id, account_email, snapshot, new_unread_ids }` and `BackgroundSyncFailed { request_id, failure }`. `new_unread_ids` contains canonical `MessageId`s, not headers or preview data. Keep foreground `SyncComplete`/`Failed` unchanged so their authorization/error UI cannot be accidentally triggered by a timer.
- Add `Action::BackgroundSyncTimer { schedule_generation }`, `Effect::ScheduleBackgroundSync { after, schedule_generation }`, `Effect::CancelBackgroundSyncTimer`, and `Effect::NotifyNewUnread { count }`. Effects keep GTK/GIO types out of `state.rs`.
- `AppState` owns a small `BackgroundSyncState`: enabled/paused reason, in-flight request id, next request id, consecutive retryable failures, and schedule generation. It may dispatch only while a session is `Ready`, an account and Inbox snapshot exist, no foreground folder load/account transition is pending, and no background request is in flight. A tick during any of those conditions is coalesced into one later one-shot schedule; it never cancels, restarts, or competes with foreground work.
- Start the first one-shot 60 seconds after a successful foreground `SyncComplete`; reset failures and reschedule after every background success. On a retryable background failure, retain the visible mailbox and schedule `min(60s * 2^failures, 15m)` with a bounded +/-10% random jitter (sampled outside the pure delay calculation, falling back to no jitter if entropy is unavailable). An authorization/configuration failure pauses automatic sync and surfaces only the subtle status/action needed to reconnect; it must not open a browser, clear credentials, or overwrite foreground session state. Disconnect, worker loss, account change, and UI destruction cancel and invalidate the timer.

### Worker and cache path

- Give `BackgroundSync` its own controller task/slot rather than putting it in the `active` account-boundary path. It must acquire the existing `metadata_lane` for the short IMAP/cache transaction, reject/coalesce a second background command while that slot is occupied, and not abort body, search, attachment, send, mutation, or foreground folder tasks. State-side gating prevents normal contention; the worker-side slot is the defensive backstop.
- The worker reads the already verified runtime account/token, refreshes credentials silently when necessary using the existing Secret Service/OAuth helpers, and performs the same bounded Inbox summary fetch/map/cache flow as `sync`. It emits no `Phase`, `AuthorizationRequired`, or foreground `Failed` events. Timeouts and Gmail/OAuth failures map to the existing `ServiceFailure` taxonomy in `BackgroundSyncFailed`; cache writes and token work remain on the worker/blocking cache lane.
- Bump `MAILBOX_VERSION` and add a serde-defaulted, bounded `InboxWatermark` to `StoredMailbox`: `{ initialized: bool, observed_ids: Vec<MessageId> }`. Validate Gmail IDs, uniqueness, and at most `MAX_SUMMARIES_PER_FOLDER` IDs. A missing/migrated watermark is deliberately uninitialized.
- Add one cache commit helper used by both full Inbox sync paths: while holding the existing cache serialization, compare the freshly mapped Inbox IDs with the prior watermark, compute only IDs that are both previously unseen and currently `unread`, replace the Inbox view, then atomically write the new full observed-ID watermark. If the watermark was uninitialized, persist the fresh IDs and return an empty notification set (the startup baseline guard). Persist all observed IDs, not only unread IDs, so a known message later toggled unread cannot be announced as newly delivered. A cache write failure emits no completion/notification and leaves the previous durable watermark in force.
- Preserve the existing retention cap and summary-only fetch. Do not fetch bodies, subjects, senders, or extra folders for notification purposes; use the existing Gmail `X-GM-MSGID` summary identity and Inbox-only fetch query.

### State/UI behavior

- On `BackgroundSyncComplete`, accept only the current request/account, clear in-flight/backoff, replace the Inbox snapshot, and update the visible list only when the Inbox is selected. Preserve selected message/reader/search/folder request state; normalize only as needed for removed rows. Deduplicate `new_unread_ids` against a bounded process-local emitted-ID set before creating exactly one notification effect for the remaining count, then arm the next 60-second timer. This protects against duplicate/out-of-order worker events in addition to the durable watermark.
- On `BackgroundSyncFailed`, accept only the current request, preserve mail and foreground readiness, expose a compact `BackgroundSyncStatus` in `ViewSnapshot` (idle/syncing/backing-off/paused), and schedule only retryable failures. Rendering must distinguish this from the existing foreground sync spinner/error; do not toast each retry.
- `Ui` owns the GLib `SourceId` in a `RefCell<Option<SourceId>>`. `Schedule...` first removes the old source, uses a one-shot `glib::timeout_add_local_once`, and dispatches the generation-tagged action through `WeakUi`; `Cancel...` and window destroy remove it. `NotifyNewUnread` calls `application.send_notification(Some("inbox-new-unread"), &gio::Notification::new("New unread mail"))` with body exactly `"N new unread messages"` (or singular); no sender, subject, body, action target, or message identifier is exposed. Notification errors/unavailability are logged/ignored and never alter sync state.

### Tests and guardrails

- Unit-test the pure state transition table: first success schedules 60s; foreground/busy tick coalesces; only one command is emitted; stale timer/event is ignored; retry delays cap and reset; non-retryable auth failure pauses; disconnect/worker loss cancels; foreground refresh remains immediate and isolated.
- Cache tests cover v3/missing-watermark migration baseline (zero notifications), atomic watermark update, stable-ID dedup across a simulated restart, read new mail exclusion, re-read known mail exclusion, multiple new IDs yielding one count, and malformed/oversized watermark rejection. Add worker tests for one background command/event, metadata-lane serialization, retryable versus terminal failures, and no foreground event variants.
- GTK-facing tests should isolate timer replacement/cancellation and notification-effect execution behind a small adapter or injectable closure; do not require a desktop daemon. Run the existing state/worker/cache suites plus `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
- Performance/error limits: one IMAP Inbox summary fetch per scheduled attempt, no polling below 60 seconds, one outstanding timer and one background task, no unbounded ID collections, no UI-thread file/network/token work, and no cache/list full rebuild beyond the current bounded Inbox snapshot. Log only request IDs/counts/failure kinds—never token, mailbox content, subject, sender, or message IDs.
