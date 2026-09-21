# Feature: Unified Mail Actions

**Date:** 2026-09-21  
**Branch:** feature/unified-mail-actions  
**Stack:** Rust 2024 · GTK4/libadwaita · Tokio · Gmail IMAP/SMTP  
**Status:** Draft

## Problem

Whitford is fast at reading cached Gmail, but messages lose essential mail-client behavior when viewed outside Inbox: replies from server search fail, and read, star, labels, archive, and trash are disabled for Starred, All Mail, labels, and server search. A daily mail client must act on the selected message wherever it is found, immediately show the intended local result, recover correctly when offline or Gmail rejects it, and make archive/trash safely reversible.

## Solution

Build a folder-independent message-action foundation around the existing stable Gmail `X-GM-MSGID` identity. Resolve an action's current mailbox UID at execution time, apply a cache-first optimistic update, reconcile against Gmail asynchronously, and provide an accessible Undo window for archive/trash. Source reply metadata from the selected loaded message rather than only the current mailbox collection. Preserve existing virtualized lists and prohibit network/database work on GTK's main thread.

## User Story

As a Whitford user, I want to reply to and manage a message from any folder, label, or search result so that finding an email never strands me in a read-only dead end.

## Scope: IN

- Reply, Reply All, and Forward from Inbox, folders, labels, and server Gmail search results.
- Stable message resolution across view-local copies using Gmail message identity and current-folder locator data.
- Read/unread, star/unstar, label, archive, and trash actions wherever their Gmail semantics are valid.
- Optimistic cache/list updates, authoritative reconciliation, visible retry on failure, and crash-safe pending-operation handling.
- Undo for archive and move-to-Trash; restore-to-Inbox/previous location where Gmail semantics allow.
- Immediate cache-first opening and tests ensuring action dispatch does not block the UI path.
- Keyboard/action accessibility states and feedback for disabled or unsupported operations.

## Scope: OUT

- Background IMAP IDLE/polling, desktop notifications, threading, multi-account, generic IMAP, bulk selection, and full Drafts mailbox. These require this action foundation and remain the next parity phases.
- Permanent deletion/EXPUNGE, to avoid irreversible behavior before a dedicated Trash workflow.

## Existing Code to Touch

- `src/model.rs`: `MessageLocator`, `MessageMutation`, message identity and pending-operation models.
- `src/state.rs`: action routing, selected message lookup, optimistic state and reconciliation.
- `src/cache.rs`: durable cached message lookup and transactional optimistic/reconciliation persistence.
- `src/gmail.rs`: Gmail IMAP mutation target resolution and label mutation semantics.
- `src/worker.rs`: asynchronous mutation/reconciliation commands and results.
- `src/ui/actions.rs`, `src/ui/render.rs`, `src/ui/email_view.rs`: availability, Undo feedback, and reply paths.
- `src/state/tests.rs` and module tests: cross-view, error, offline and performance-regression coverage.

## Edge Cases to Handle

- Search result not present in the current mailbox collection.
- The same Gmail message appears in multiple labels/folders with different UIDs.
- Archive requested for an already non-Inbox message; Trash requested from Trash; label changes in virtual/all-mail views.
- Message changes remotely or disappears between optimistic update and IMAP command.
- Network/auth failure, operation supersession, app restart during an Undo window, and double Undo.
- Cache has body but no valid current locator; opening remains local while action gives a recoverable state.

## Test Scenarios

- Open a cached message from server search and Reply/Reply All/Forward without a fresh mailbox fetch.
- Star/read/label/archive/trash from Inbox, Starred, All Mail, a user label, Trash, and server search, asserting valid/disabled semantics.
- Successful optimistic action, Gmail rejection rollback, retry, remote conflict reconciliation, and restart recovery.
- Archive/Trash Undo restores both Gmail labels and cached/visible state.
- Existing virtualized list behavior and cache-first reader path remain non-blocking.

## Assumptions

- Gmail's `X-GM-MSGID` remains the canonical identity; IMAP UIDs are folder-local execution locators.
- Archive means removing the Inbox label; Trash means applying Gmail's Trash label. Actions with no meaningful Gmail effect are disabled with explanatory feedback.
- This automated run chooses the complete action foundation first; always-on synchronization/notifications follow immediately after it rather than being combined with a risky transport rewrite.

## Open Questions

- None blocking. The implementation will retain only recoverable Trash behavior and defer permanent deletion.

## Scope

**Tier:** cross-stack (single native desktop repo)

**Reasoning:** This changes persistent cache/state, Gmail IMAP worker transport, and GTK action/rendering surfaces. The project is not a web stack, so Rust-domain specialists replace the skill's nominal backend/frontend roles.

**Subagents to spawn:** state/cache design, Gmail/worker design, UI/performance design (all `gpt-5.6-terra`, per user request).

**Skills to invoke:** automated plan, direct Cargo verification, pre-ship review. Product review is skipped because this feature was selected from today's product review.

## Effort Plan

**Profile:** forced:medium (with `gpt-5.6-terra` for every delegated task)
**Rules fired:** E4 — current server-search reply flow is a verified broken core flow

| Task | Model | Effort | Why |
|---|---|---|---|
| State/cache design | gpt-5.6-terra | medium | user-requested model; persistent action state |
| Gmail/worker design | gpt-5.6-terra | medium | user-requested model; protocol integration |
| UI/performance design | gpt-5.6-terra | medium | user-requested model; response and accessibility |
| Implementation | gpt-5.6-terra | medium | user-requested model; code changes |
| Review passes | gpt-5.6-terra | medium | user-requested model |

**Escalations during run:** none

## Implementation Plan

### Data and protocol contract

1. Keep `MessageId` as the only cross-view identity (`X-GM-MSGID`) and keep `MessageLocator` explicitly advisory and mailbox-local. Do not add a canonical message database: retain the existing per-folder cache views and the ephemeral server-search result list. Extend the message/action metadata just enough to express Gmail system-label membership (`in_inbox`, `in_trash`, plus the current user-label list) and to retain an action's source locator and pre-action label snapshot for rollback/Undo. Populate it from the already-fetched `X-GM-LABELS`, preserving the present user-label UI.
2. Replace the Inbox-only transport entry points with `mutate_message` and `reconcile_message`. First select/examine the supplied locator mailbox, verify `UIDVALIDITY`, UID, and `X-GM-MSGID`; on a stale/missing locator, resolve by `UID SEARCH X-GM-MSGID <id>` in a bounded candidate order (supplied mailbox, All Mail, Inbox, Trash, then the known source folder) and verify the fetched identity before changing anything. Never use a UID returned by search without the ID check. Return an authoritative state including system membership, user labels, and the confirmed current locator when one exists.
3. Encode Gmail effects by label semantics, rather than by the displayed view: read/star use flags; archive removes `\\Inbox`; normal label changes add/remove the validated user label; trash applies/moves to the discovered Trash mailbox. Add internal inverse operations for Undo: archive adds `\\Inbox`; trash removes `\\Trash` and restores the captured Inbox membership and user labels where Gmail permits. Treat archive outside Inbox and trash from Trash as no-op/disabled actions with a reason, not protocol failures. Do not expose permanent delete.
4. Add a small, versioned, account-scoped pending-action journal beside the existing cache, written atomically with restrictive permissions. A record contains operation ID, message ID, source locator/folder, forward mutation, inverse/restore snapshot, creation/deadline, and phase (`dispatching`, `confirmed-awaiting-undo`, `undo-dispatching`). Persist before dispatch; update/remove it after every terminal transition. On restore, load unexpired records, reconcile them in the worker, surface a resumable Undo when the forward effect is present, discard expired/rolled-back entries, and show a recoverable failure rather than guessing. Bound journal size and expire records defensively.

### Work sequence

1. Add model types and validation first: system-membership fields, resolved target/state, operation/Undo IDs, restore payload, and journal schema. Keep `MessageMutation`'s public actions small; make compensating mutations internal so UI callers cannot fabricate restore state. Add serialization/backward-compatible defaults for old cached views.
2. Generalize cache helpers from `*_inbox` to folder-independent, transactional view updates. For a confirmed/reconciled state, update every cached view containing the ID (metadata fields and labels), remove it only from views whose membership is no longer true (Inbox/archive; Trash/restore), and retain unrelated label/All Mail rows. The optimistic cache write and journal transition run on the worker's blocking cache lane, never GTK. Continue using atomic file replacement; a failed persistence emits a warning and schedules a reconciliation rather than falsely reporting durable success.
3. Refactor Gmail IMAP selection/resolution and mutation/reconciliation behind the new generic APIs. Use a single authenticated session per operation, timeouts matching the current mutation path, bounded UID searches/fetches, and a final reconciliation after uncertain I/O. Resolve the locator immediately before both forward and Undo commands; this is essential because trash/archive can change mailbox-local UIDs. Map stale UIDVALIDITY, missing message, auth, offline, and malformed label responses to typed outcomes.
4. Extend worker commands/events to carry operation ID, mutation and restored authoritative state/locator; serialize same-message dimensions as today, and serialize an Undo behind its forward operation. Persist/cache-reconcile before emitting the terminal event when possible. On uncertain result, reconcile; if still uncertain, retain the journal record and emit “status could not be verified” with Retry/Refresh rather than rolling the UI back blindly. Add structured, redacted tracing for operation ID, mutation kind, resolver path, latency, outcome, cache/journal result, and reconciliation mismatch (never subjects, addresses, labels, IDs, or tokens).
5. Refactor `AppState` selection into a single selected-message accessor/value that searches server results first, then the loaded folder, and use that value for reader, compose, and mutations. Build reply/forward subject from that selected summary (not `self.mailbox`); retain body/reply context from `ReaderState`. This fixes server-search compose while preserving the existing requirement that the body is loaded locally before reply. Remove the selected-folder/Inbox gate and instead derive per-action availability/reason from session, selected summary membership, folder catalog, current pending dimension, and server-search status.
6. Make optimistic state representation-aware. Apply read/star/label changes to every in-memory representation currently holding the selected ID (loaded folder and loaded server search); archive/trash immediately remove it only from views it should leave and retain enough pre-action placement to restore exactly on definite failure. Reconciliation applies authoritative membership to both representations when still present. Preserve list virtualization by changing vectors in place, bumping only the existing list/reader revisions, and normalizing selection after removals.
7. Add `Action::UndoMessageOperation` and a single accessible transient feedback/banner after a confirmed archive/trash, with a deterministic deadline, keyboard action, screen-reader announcement, disabled/busy state, and clear outcome text. Undo immediately optimistically restores the affected local representation, dispatches the persisted inverse, and reverses/restores on definite failure. A double Undo, expired Undo, superseded message mutation, or missing valid locator is harmless and explains the next recovery action. Ensure actions that are invalid in a virtual view remain visible but disabled with an explanatory tooltip/feedback.
8. Wire GTK actions/rendering last: enable reply/reply-all/forward from a loaded selected search result; compute archive/trash/read/star/label availability from the new snapshot flags rather than `selected_folder_id == Inbox`; render Undo and retry/reconnect feedback without blocking. Keep all action handlers state-only; worker/cache/network work remains off the main thread.

### Verification and rollout guardrails

- Unit-test model validation, old-cache defaults, label-effect/inverse construction, action availability matrix, resolver candidate ordering/identity verification, and journal atomic load/expiry/recovery. Add IMAP transcript/mock tests for stale locator fallback, UIDVALIDITY change, Gmail rejection, network timeout, remote removal, and a UID that resolves to the wrong `X-GM-MSGID`.
- Add state tests for Inbox, Starred, All Mail, user-label, Trash, and server-search selected messages: optimistic forward/rollback/reconcile, duplicate cross-view representation, action supersession, selection/reader normalization, reply/reply-all/forward using a server-search summary, and no fresh mailbox fetch on that compose path. Assert cache-first body opening and action dispatch only enqueue worker effects.
- Add worker/cache integration tests for confirmed and uncertain mutations, all affected cached views, pending journal restart recovery, archive Undo, trash Undo to Inbox and prior user label, double/expired Undo, and cache-write failure. Keep the current Inbox cases as compatibility tests and run full `cargo test` plus clippy/format checks.
- Initially retain the old Inbox-only mutation/reconciliation helpers as private shadow-oracle test paths only: for Inbox actions, compare their derived authoritative state/cache membership with the generalized path in tests and log a redacted mismatch during development. Remove those helpers after parity tests pass; do not dual-send Gmail mutations in production.
- Instrument resolver fallback rate, mutation/Undo success/definite-failure/uncertain rates, reconciliation latency/mismatch, journal recovery count, cache persistence failures, and GTK handler-to-enqueue duration. Treat an increase in UI-path duration, unbounded resolver searches, or journal growth as release blockers.

### Deferred deliberately

- No background IDLE/polling, multi-account, bulk operations, thread-wide actions, permanent delete/EXPUNGE, full Drafts sync, or generic-IMAP abstraction.
- Do not build a global canonical message store or eagerly synchronize every folder. The generalized per-view cache update plus authoritative, bounded resolution is sufficient for this feature; revisit a canonical index only if measured resolver/cache duplication becomes a demonstrated bottleneck.
- Do not promise restoration of Gmail labels unavailable in the pre-action snapshot or remote changes made after the action; reconciliation wins and the UI reports the conflict with Refresh/Retry recovery.
