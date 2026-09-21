---
kind: test-manifest
feature: Unified Mail Actions
slug: unified-mail-actions
date: 2026-09-21
branch: feature/unified-mail-actions
pr: ""
brief: .autofeature/designs/unified-mail-actions-2026-09-21.md
platforms: [desktop-linux]
scope: cross-stack
---

# Test Manifest — Unified Mail Actions

## Setup

- **Environments:** local Wayland/Hyprland desktop session running Whitford.
- **Credentials/roles needed:** an authenticated Gmail account with the configured Whitford OAuth client; never record the token in this file.
- **Seed data:** one message in Inbox, one in a user label, one search-only result, and one non-Inbox message such as Sent or All Mail.
- **Feature flags / config:** none.
- **Dependencies:** Gmail IMAP and SMTP connectivity for mutation confirmation; local cache populated for cache-first checks.

## Surfaces built

### Desktop mail client

| Surface | Purpose |
|---|---|
| Message reader actions | Folder-independent reply, read, star, label, archive, and Trash actions. |
| Server Gmail search | Open, reply to, and mutate a selected server-search result. |
| Archive/Trash feedback | Toast and persistent accessible Undo affordance. |
| Gmail IMAP worker and cache | Identity-verified folder-local mutation resolution and authoritative cache reconciliation. |

## Acceptance flows

### AF-1 — Reply to a Gmail search result · priority: critical

- **Precondition:** a Gmail search result is not currently in the visible folder list.
- **Steps:**
  1. Run a Gmail search and open the result.
  2. Wait for its body to load, or use a result whose body is cached.
  3. Choose Reply, Reply All, and Forward in separate attempts.
- **Expected:** each compose draft uses the selected result's subject and reply context; no error says the source is absent from the current folder.

### AF-2 — Archive and Undo from a non-Inbox view · priority: critical

- **Precondition:** an Inbox message is visible through Starred, All Mail, a label, or Gmail search.
- **Steps:**
  1. Archive the message from that view.
  2. Confirm immediate local feedback and invoke Undo.
  3. Refresh the affected mailbox.
- **Expected:** the action is enabled based on the message, not the selected view; archive removes Inbox membership; Undo restores it; the final list agrees with Gmail.

### AF-3 — Trash Undo preserves original membership · priority: critical

- **Precondition:** a Sent-only or All-Mail message without Inbox membership is selected.
- **Steps:**
  1. Move it to Trash.
  2. Wait for confirmation and choose Undo.
  3. Refresh Sent/All Mail and Inbox.
- **Expected:** the message is restored to its former membership and user labels, and is not incorrectly added to Inbox.

### AF-4 — Uncertain or failed mail action · priority: normal

- **Precondition:** simulate offline/timeout or reject the IMAP action in a test account.
- **Steps:** archive or Trash a selected message; optionally press Undo before confirmation.
- **Expected:** definite Gmail rejection restores the local view with clear feedback. An uncertain outcome is reconciled rather than blindly replayed or falsely presented as complete.

### AF-5 — Cache-first performance and keyboard/accessibility · priority: normal

- **Precondition:** a message body is present in Whitford's local cache; list has hundreds of rows.
- **Steps:**
  1. Open the cached message while disconnected.
  2. Use reader action buttons and keyboard archive/delete actions.
  3. Let the initial Undo toast expire.
- **Expected:** cached content opens without waiting for Gmail; unchanged list rows remain stable; a screen-reader-accessible persistent Undo action remains while Undo is valid; no global Ctrl+Z steals editor undo.

## Out of scope / not covered

- Background IMAP IDLE/polling and desktop notifications.
- Threaded conversations, multi-account/unified Inbox, generic IMAP/SMTP, bulk actions, and permanent deletion.
- Undo persistence across application restart; pending operations are reconciled safely, but the transient Undo affordance is intentionally process-local.
