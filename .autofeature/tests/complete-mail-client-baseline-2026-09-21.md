# Test Manifest: Complete Mail Client Baseline

**Branch:** `feature/complete-mail-client-baseline`  
**Platform:** Linux/Wayland, GTK4/libadwaita  
**Scope:** Gmail mailbox management, composition, navigation, attachments, search, drafts and account cleanup

## Setup

- Launch the installed Whitford binary under Wayland.
- Connect the configured Gmail test account.
- Use an Inbox containing read/unread, starred, labelled, HTML, plain-text and attachment messages.

## Acceptance flows

### AF-1 — Inbox triage

Open Inbox, toggle read and star, apply/remove an existing label, archive one message and move another to Trash. Each action should update immediately, remain correct after refresh, and never duplicate or insert the message into the wrong folder.

### AF-2 — New message and drafts

Press Ctrl+N, compose a standalone message with To/Cc/Bcc, formatting, signature and attachment, then Save & Close. Start another draft, resume each exact draft independently, send one, and confirm no reply-thread headers are attached to standalone mail.

### AF-3 — Folder navigation

Switch among Inbox, Starred, Sent, All Mail, Trash and a user label. Cached folders should paint immediately, refresh in the background, retain selection/focus sensibly, and reuse already-downloaded bodies.

### AF-4 — Received attachments

Open an attachment-bearing message. The body should appear without waiting for attachment payloads. Test Open, Save As, progress and cancel. Cancellation must leave no partial file; Save As must preserve an existing destination if the download fails.

### AF-5 — Gmail search

Type in search and confirm instant local sender/subject filtering without network state. Press Enter or Search Gmail for a full-mailbox query; test results, cancellation, retry, query edits and a no-results query. Truncated searches should state the displayed/total count.

### AF-6 — Offline and error recovery

With cached folders available, disable networking and switch folders/open cached bodies. Cached content should remain visible with stale/offline feedback. Mutations and server search should fail clearly without corrupting cached state.

### AF-7 — Disconnect cleanup

Create multiple drafts, stage attachments and open a received attachment, then disconnect. Whitford must hide mail immediately and remove tokens, summaries, bodies, attachment cache, drafts, staged files and signature. If cleanup fails, only Retry cleanup should be offered and the retry must be idempotent.

### AF-8 — Performance

Set retention to 500, navigate through messages and toggle flags. Scrolling, selection and mutations should remain responsive; unchanged message rows should not be recreated and attachment progress should not rebuild the reader.

## Automated coverage

- Canonical Gmail identity, localized folder discovery and bounded folder/body caches.
- Exact IMAP mutation/search/attachment query construction and escaping.
- Optimistic confirmation, rollback, uncertain reconciliation and stale generation rejection.
- Draft restoration races, corrupt catalogs, multiple drafts and staged-file cleanup.
- Attachment chunk decoding, limits, atomic writes, symlink/traversal rejection and cancellation.
- Cross-account transition, token-last cleanup and crash-retry cache markers.
- Virtualized 500-row list model stability.

## Out of scope

- Permanent deletion and Empty Trash.
- Creating, renaming or deleting Gmail labels.
- Spam/Important management.
- Gmail REST search pagination.
- Production Google OAuth verification/distribution.
