# Product Review: Whitford

Mode: product (whole-product audit)  
Mapped: 12 surfaces, 6 journeys  
Live URL: not checked  
Verified: 7 high-severity claims checked against code, 0 refuted

Whitford has a compelling privacy-first, fast/offline Gmail wedge, but it is over-invested in composer polish while essential mailbox workflows are missing or unsafe. The next feature run should be **Inbox Triage Actions**: read/unread, star, archive, delete, and label mutations with reliable Gmail synchronization and failure handling. New-message compose, mailbox navigation, received attachments, search, and draft/account-lifecycle safety should follow.

## High (9)

1. **Inbox has no triage actions** — Inbox list and reader. Add read/unread, star, archive, delete, and label mutations with optimistic UI, rollback, and Gmail reconciliation. ✓ verified
2. **Users cannot compose a new message** — Primary navigation and composer entry points. Expose a New Message flow using the existing composer and SMTP pipeline. ✓ verified
3. **Mailbox navigation is Inbox-only** — Navigation and uncertain-send recovery. Add Sent, Drafts, Starred, All Mail, Trash, and labels. ✓ verified
4. **Received attachments cannot be opened or saved** — Reader and mail ingestion. Fetch attachment payloads and provide safe Open and Save As actions. ✓ verified
5. **Search is local, bounded, and Inbox-only** — Search. Add server-backed Gmail search while retaining explicit offline filtering. ✓ verified
6. **Startup draft restoration can overwrite a recovered draft** — Startup and composer initialization. Gate writes until restoration completes and use stable draft identities. ✓ verified
7. **Disconnect leaves sensitive account artifacts on disk** — Drafts, staged attachments, and signatures. Make disconnect remove all account-local data before confirming. ✓ verified
8. **OAuth activation is developer-only** — OAuth configuration and onboarding. Establish a distributable production OAuth path or explicitly commit to a self-hosted power-user model.
9. **A single global draft can resume in unrelated replies** — Draft model. Move to per-compose/per-thread draft records with explicit recovery choices.

## Medium (6)

1. Draft-load failures are mislabeled and offer no retry.
2. Retention persistence failures can falsely confirm success.
3. Composer authentication recovery can enter a refresh loop.
4. Unopened message bodies are unavailable offline.
5. Privacy-first, fast/offline Gmail is Whitford's strongest strategic wedge.
6. Composer parity is over-invested relative to mailbox basics.

## Recommended feature sequence

1. **Inbox Triage Actions** — complete the routine inbox-management loop.
2. **New Message Compose** — reuse the composer and SMTP investment for standalone mail.
3. **Mailbox Navigation and Sent Recovery** — make core folders and uncertain-send recovery usable.
4. **Received Attachment Downloading** — close the read-and-use flow.
5. **Full-Mailbox Gmail Search** — search beyond the bounded local Inbox window.
6. **Draft Isolation and Secure Disconnect** — eliminate recovery races and clean all account-local data.

## Recommended next run

`$autofeature:autofeature-do mode:automated Implement Inbox Triage Actions in Whitford: Gmail IMAP mutations for read/unread, star/unstar, archive, trash/delete, and labels; enable the existing controls; use optimistic UI with rollback and authoritative refresh/reconciliation; preserve responsiveness and offline cached reading; include keyboard actions, clear pending/error feedback, and comprehensive state/worker/IMAP tests. [skip-product-review]`
