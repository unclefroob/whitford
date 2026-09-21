# Feature: Multi-account Unified Inbox

**Date:** 2026-09-21  
**Branch:** `feature/multi-account-inbox`  
**Stack:** Rust 2024 · GTK4/libadwaita · Gmail IMAP/SMTP OAuth  
**Mode:** automated  
**Status:** Draft

## Problem

Whitford currently treats one Gmail identity as the entire application: connecting another identity replaces the prior mailbox, refresh token, cache, drafts, and sync state. A person with work and personal Gmail must disconnect and reconnect repeatedly, cannot see all new mail together, and risks disrupting local data.

## Solution

Create a durable account registry and account-scoped credentials/cache/runtime state. Whitford opens on a Unified Inbox that merges each connected account’s Inbox in deterministic newest-first order; the sidebar also lets the user select an individual account and its folders. Every network operation, body, mutation, attachment, draft, and composer is bound to its originating account.

## User Story

As a person with multiple Gmail accounts, I want to add them once and see a unified inbox while retaining per-account views, so I can handle all mail without switching connections or accidentally sending from the wrong identity.

## Assumptions

- Unified Inbox is the default landing view; all other folders are per account.
- One Gmail OAuth Desktop client configuration is shared, but refresh tokens are account-specific and remain only in Secret Service.
- Existing cache retention remains a global per-account limit for this first release; no cross-account quota rebalancing is introduced.
- The existing singleton account is migrated safely on first launch, preserving its cache, drafts, and authorization where possible.

## Scope: IN

- Add accounts without disconnecting existing ones; reject duplicate Gmail identities safely.
- Persistent opaque account IDs, account registry, account-scoped Secret Service records and cache namespaces.
- One-time, recoverable migration of current singleton account data/token to the account registry.
- Unified Inbox projection with deterministic ordering and account badge; per-account Inbox/folder/label navigation.
- Account-aware compose From selection, replies, sends, body loading, attachments, mutations, undo and searches.
- Per-account background sync, retry/backoff state, watermark/notification deduplication, bounded fair scheduling.
- Settings account manager: add, reconnect, remove one account, and view individual sync status.
- Comprehensive state/cache/secrets/worker/UI tests and manual acceptance flows.

## Scope: OUT

- Outlook, generic IMAP, shared/team mailboxes, aliases, delegated send-as identities, and account groups.
- Cross-account search, unified Sent/All Mail/Trash, conversation threading, and per-account retention quotas.
- Remote/cloud synchronization of account configuration or plaintext credential export.

## Existing Code to Touch

- `src/model.rs`: introduce account and account-scoped message/navigation identities.
- `src/secrets.rs`: replace singleton keyring selector with account-scoped records and migration.
- `src/cache.rs`: account registry, per-account namespaces, atomic legacy migration and scoped cleanup.
- `src/worker.rs`: account-keyed OAuth/runtime auth/commands/events and fair background sync.
- `src/state.rs`, `src/state/tests.rs`: per-account reducer state and Unified Inbox projection.
- `src/composer.rs`, `src/smtp.rs`, `src/drafts.rs`: bind compose/send/drafts to account ID and preserve account isolation.
- `src/ui/`: account-aware sidebar, unified row identity/badges, composer From selection and Settings account manager.

## Edge Cases to Handle

- Same Gmail numeric message ID, label, or folder name in two accounts must never collide.
- Duplicate OAuth identity must retain the existing token/cache and report a useful error.
- Disconnecting account A must not touch account B’s keyring token, cache, drafts, staged attachments, notifications, or selected view.
- Interrupted migration must be idempotent and never delete the legacy account until new registry/cache/token writes succeed.
- Stale body/send/mutation/sync events from one account must not update another account’s UI.
- Offline, expired, rate-limited, or slow account must not block healthy accounts; foreground account work gets priority.
- Replies and sends always use the source/selected account, including after navigation changes.

## Test Scenarios

- Migrate existing single-account data and restore it alongside a newly added account.
- Add two accounts with equal Gmail message IDs; verify body/cache/mutation/send isolation.
- Merge unified Inbox ordering deterministically across timestamp ties and missing timestamps.
- Navigate an account folder, compose/reply/send through that account, then switch views without changing identity.
- Disconnect one account and prove the other remains connected, cached, synchronized, and selectable.
- Verify duplicate identity, keyring failure, interrupted migration, stale worker event, and per-account backoff behavior.

## Scope

**Tier:** cross-stack

**Reasoning:** This is a single native desktop repository but changes durable data/cache and OAuth-token security boundaries, asynchronous worker protocols, reducer/state projection, and GTK UI. It is treated as cross-stack for parallel design/review.

**Subagents to spawn:** cache/security architecture; state/worker architecture; GTK product/UI review; technical plan and pre-ship review passes.

**Skills to invoke:** security review equivalent, test manifest, simplify review.

## Effort Plan

**Profile:** balanced  
**Rules fired:** E2 — refresh-token, OAuth and Secret Service changes; E3 — durable migration/re-key of existing account data.

| Task | Effort | Why |
|---|---|---|
| Technical plan | high | E2/E3 |
| Cache/secrets architecture and implementation | high / medium | E2/E3 / base implementation |
| State/worker architecture and implementation | high / medium | E2 / base implementation |
| GTK UI design and implementation | medium / medium | base |
| Critical pre-ship review | high | E2 |
| Read-only/testing/design reviews | low | base |

**Escalations during run:** none
