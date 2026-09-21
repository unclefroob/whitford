---
kind: test-manifest
feature: Always-on Mail Sync
slug: always-on-mail-sync
date: 2026-09-21
branch: feature/unified-mail-actions
pr: https://github.com/unclefroob/whitford/pull/1
brief: .autofeature/designs/always-on-mail-sync-2026-09-21.md
platforms: [desktop-linux]
scope: cross-stack
---

# Test Manifest — Always-on Mail Sync

## Setup

- **Environments:** a local Wayland/Hyprland session running Whitford.
- **Credentials/roles needed:** an authenticated Gmail account configured for Whitford; never include credentials in this file.
- **Seed data:** a populated Inbox and a way to deliver a new unread email to the account.
- **Feature flags / config:** none.
- **Dependencies:** working Gmail IMAP connectivity and a desktop notification daemon.

## Surfaces built

### Desktop mail client

| Surface | Purpose |
|---|---|
| Background Inbox sync | Incremental, bounded Inbox polling while Whitford is open. |
| Sync status | Accessible idle, checking, retrying, and paused status copy. |
| Desktop notification | Privacy-safe count-only new-unread notification with Open Inbox action. |
| Worker/cache watermark | Restart-safe unread notification deduplication and account-epoch safety. |

## Acceptance flows

### AF-1 — First background baseline is silent · priority: critical

- **Precondition:** Whitford is connected with existing unread Inbox messages.
- **Steps:**
  1. Start Whitford and wait for initial synchronization plus the first background cycle.
- **Expected:** Inbox loads normally; no desktop notification is sent for pre-existing unread mail.

### AF-2 — New unread mail notifies once · priority: critical

- **Precondition:** Whitford has established an Inbox baseline.
- **Steps:**
  1. Deliver one new unread email.
  2. Wait for the next background check.
  3. Trigger a further background check without new mail.
- **Expected:** Inbox updates without disrupting the current view; one count-only notification appears; no duplicate appears on the later check or after restart.

### AF-3 — Notification opens Inbox · priority: normal

- **Precondition:** a Whitford new-mail notification is visible.
- **Steps:**
  1. Activate the notification or choose Open Inbox.
- **Expected:** Whitford presents its window and navigates to Inbox. The notification reveals no sender, subject, account, or message content.

### AF-4 — Foreground work stays responsive · priority: critical

- **Precondition:** background polling is due while a folder, Gmail search, or message reader is active.
- **Steps:**
  1. Start a server search or open a message.
  2. Allow a background check to complete.
  3. Manually refresh or mutate a message.
- **Expected:** search/reader/selection remains intact, manual work remains responsive, and no background update replaces the active projection.

### AF-5 — Offline and account-boundary recovery · priority: normal

- **Precondition:** Whitford is connected.
- **Steps:**
  1. Temporarily remove network access, then restore it.
  2. Observe retrying status, then successful recovery.
  3. Disconnect or switch account while a background check is in flight.
- **Expected:** retries back off without a busy loop; success resets the schedule; terminal authorization failure pauses polling; stale work cannot update the old account cache or notify.

## Out of scope / not covered

- IMAP IDLE/push, background sync after the application exits, configurable notification policy, and per-message notification previews.
- Threading, multi-account unified Inbox, generic IMAP, and notification click-to-message navigation.
