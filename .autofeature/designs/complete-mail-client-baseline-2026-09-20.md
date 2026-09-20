# Feature: Complete Mail Client Baseline
**Date:** 2026-09-20
**Branch:** feature/complete-mail-client-baseline
**Stack:** Rust, GTK4/libadwaita, WebKitGTK, async-imap, lettre
**Status:** In progress

## Problem
Whitford reads and replies to Gmail quickly, but it cannot complete the normal mail loop: users cannot triage messages, compose new mail, browse other mailboxes, use received attachments, or search the full mailbox. Draft restoration and disconnect cleanup also have verified data-lifecycle gaps.

## Solution
Build the missing baseline as bisectable slices: safe multi-draft lifecycle and complete cleanup; stable Gmail identity and folder-aware caching; inbox mutations; standalone compose; folders and labels; lazy received attachments; and explicit server-backed search. Preserve Whitford's privacy-first, performance-first design.

## User Story
As a Hyprland user, I want one lightweight native client that can manage, compose, navigate, download, and search Gmail without returning to Gmail Web.

## Scope: IN
- Multiple isolated local drafts, safe restoration, accurate recovery, complete disconnect purge.
- Read/unread, star, archive, Move to Trash, and apply/remove existing labels.
- Standalone new-message composition using the existing rich composer and SMTP path.
- Sent, All Mail, Trash, Starred, Inbox, and selectable user-label browsing.
- Lazy exact-part received-attachment Open and Save As with progress/cancellation.
- Immediate local filtering plus explicit Gmail `X-GM-RAW` server search.
- Stable `X-GM-MSGID` identity, per-folder locators, bounded folder/body/attachment caches.
- Virtualized/diffed message rendering and nonblocking I/O.

## Scope: OUT
- Permanent deletion and Empty Trash; Delete is reversible Move to Trash.
- Creating, renaming, or deleting Gmail labels.
- Gmail REST; broad IMAP search is bounded after Gmail returns matching UIDs.
- Spam/Important management and production OAuth verification.

## Existing Code to Touch
- `src/model.rs`: canonical message/folder/attachment domain types.
- `src/gmail.rs`: LIST, folder fetch, mutations, search, MIME part fetch.
- `src/cache.rs`: folder-aware cache schema and bounded LRU migration.
- `src/drafts.rs`: multi-draft catalog and account purge.
- `src/composer.rs`, `src/smtp.rs`: standalone new-message kind.
- `src/state.rs`: request generations, optimistic mutations, folders/search/download/draft state.
- `src/worker.rs`: serialized metadata actor and bounded content jobs.
- `src/ui/*`: enabled actions, compose entry, folders, labels, downloads, search and virtualized rows.

## Architecture
`GTK action -> pure reducer (optimistic state/request ID) -> worker command -> Gmail/filesystem -> typed event -> generation check -> confirm/rollback/reconcile -> bounded cache -> keyed UI update`

Gmail identity is canonical `X-GM-MSGID`; a folder locator owns mailbox name, UIDVALIDITY and UID. Mailbox names come from LIST Special-Use. Reads use EXAMINE; mutations SELECT only their target. Uncertain mutations reconcile by canonical ID and are never blindly replayed.

## Implementation Order
1. Draft catalog, restoration gating, multiple isolated drafts, secure disconnect cleanup.
2. Stable Gmail identity, folder catalog and cache v3; virtualized message list.
3. Inbox triage mutations with optimistic rollback and authoritative reconciliation.
4. Standalone compose.
5. Cached-first folder and label navigation.
6. Lazy exact-part attachment fetch/open/save.
7. Explicit server Gmail search.
8. Integration hardening, documentation, release install and live acceptance.

## Error and Rescue Map
- Draft recovery failure: block destructive draft writes, preserve files, show Retry.
- Disconnect purge failure: hide mail, retain token until local purge succeeds, expose cleanup Retry.
- Definite IMAP mutation failure: roll back latest optimistic dimension and explain failure.
- Uncertain mutation: retain pending state and reconcile; never auto-repeat MOVE.
- Folder failure: retain cached view with stale status.
- Attachment failure/cancel: remove `.part`, preserve destination and message, allow Retry.
- Search failure: preserve local results and offer server-search Retry.
- UIDVALIDITY change: discard locators and remap; preserve bodies keyed by Gmail ID.

## Performance Requirements
- No GTK-thread network, MIME, hashing, JSON, or filesystem work.
- Optimistic action visible within one frame; cached folder first paint target under 50 ms.
- At most 500 summaries per view and 8 cached folder views; virtualized rows.
- Normal body loads exclude attachment payloads; attachment working memory at most 2 MiB.
- One metadata lane, independent body/attachment work, at most two content downloads.
- Search result display capped at 500 with a 30-second timeout and 2 KiB query bound.
- Snapshots never contain body or attachment bytes.

## Test Scenarios
- Draft restoration/write/account-switch races, multiple drafts, corrupt catalog and recovery retry.
- Token-last idempotent cleanup with injected failures at each stage.
- Exact STORE/MOVE/X-GM-LABELS/X-GM-RAW contracts, escaping and capability failures.
- Optimistic confirm/rollback/uncertain reconciliation, double toggles and stale events.
- Folder cache migration/LRU/UIDVALIDITY and canonical body reuse.
- New compose headers, Bcc, attachments, failures and saved recovery.
- MIME paths, chunk boundaries, malicious filenames, cancellation and atomic Save As.
- Search bounds, Unicode/operators, stale requests and offline local fallback.
- Revision tests ensure a flag/progress change does not rebuild the whole list.

## Scope
**Tier:** cross-stack-equivalent single repository (protocol, persistence, state and native UI)

**Reasoning:** The change crosses every application layer and contains destructive local cleanup, external protocol mutations, files and HTML composition.

**Subagents:** high-effort plan; sequential Rust implementation slices; critical, testing and design review passes.

**Skills:** autofeature planning/review; security review performed inline because no separate security skill is installed.

## Effort Plan
**Profile:** balanced
**Rules fired:** E3 (destructive cleanup), E4 (verified broken flows)

| Task | Effort | Why |
|---|---|---|
| Integrated plan | high | Destructive lifecycle and protocol design |
| Slice implementation | medium | Focused sequential code changes |
| Critical review | high | Mail mutation, filesystem and rendered HTML boundaries |
| Testing/design review | low | Bounded evidence-based review |

**Escalations during run:** none

## Assumptions
- Existing user labels may be applied or removed; label administration is deferred.
- Delete always means Move to Trash.
- Search requires explicit submission; typing remains a zero-network local filter.
- Multiple drafts are local/device-private and only one composer window is visible at a time.
