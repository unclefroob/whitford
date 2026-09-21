# Product Review: Apple Mail Parity Roadmap

Mode: product (whole-product audit)  
Target: behavioural parity with Apple Mail on macOS, adapted for Linux/Hyprland  
Mapped: 13 surfaces, 9 journeys  
Verified: 6 high-severity claims checked against code, 0 refuted

## Product decision

Whitford should target Apple Mail's daily-driver capability, not reproduce Apple's visual design or proprietary ecosystem. Gmail remains the first provider, but the architecture should stop assuming one account, one provider, one Inbox, or one foreground session.

Parity has three meanings:

- **Native parity:** implement the same user outcome directly.
- **Linux equivalent:** integrate an appropriate desktop or open standard instead of an Apple service.
- **Explicit non-goal:** exclude an Apple-locked capability unless a standards-based equivalent exists.

Performance remains a product requirement: cached messages must open immediately, synchronization must stay off the UI thread, lists must remain virtualized, and background work must be bounded and observable.

## P0: finish and trust the current product

1. **Unify message identity and actions across every view.** Reply currently fails for messages opened from server search, and read/unread, star, labels, archive, and trash are artificially Inbox-only. Actions should operate on a stable message/thread identity from Inbox, Starred, labels, All Mail, Trash, and search.
2. **Add reversible destructive actions.** Show Undo after archive/trash, support Move to Inbox/restore, and make permanent deletion an explicit Trash-only action.
3. **Build a real Drafts mailbox.** List, search, resume, discard, and recover every draft—not only the most recent one. Then synchronize drafts with Gmail while preserving crash-safe local saves.
4. **Make refresh and authentication recovery authoritative.** Refresh account state, folder catalog, and the current view; prevent expired-token retry loops.
5. **Fix misleading states and documentation.** Retention changes must report persistence/pruning failures, and the README/AppStream feature descriptions must match the shipped app.

## Phase 1: Apple Mail daily-driver core

1. **Always-on synchronization and notifications** — Gmail push/IMAP IDLE where possible, bounded fallback polling, desktop notifications, unread counts, reconnect/backoff, and no UI-thread network work.
2. **Conversation view** — group by Gmail thread, expand individual messages, reply at message or thread level, and preserve stable scroll/selection while updates arrive.
3. **Complete mailbox model** — Drafts, Junk/Spam, Important, Archive semantics, Favorites, user-created mailboxes/labels, move/copy, and consistent mutations from every surface.
4. **Offline-first send pipeline** — compose without a connection, transactional Outbox, retry/cancel, uncertain-send reconciliation, Undo Send, and Send Later.
5. **Bulk workflow and navigation** — multi-select, bulk triage, sort/filter, keyboard-first navigation, next/previous unread, and configurable swipe/shortcut actions.
6. **Contacts and addressing** — recipient autocomplete, recent recipients, address groups, From/Reply-To selection, aliases, and per-account signatures.
7. **Multiple accounts** — unified and per-account inboxes, strict cache isolation, per-account settings, and account-aware compose. Add standards-based IMAP/SMTP providers after the Gmail path is abstracted.

## Phase 2: Apple Mail productivity parity

- Remind Me, follow-up suggestions, VIP senders, flags, muted conversations, and unsubscribe.
- Smart mailboxes/saved searches and local rules with clear server-versus-device behavior.
- Redirect, templates/stationery equivalents, print, import/export, and attachment management.
- Junk controls, sender blocking, remote-content privacy controls, and phishing/suspicious-link warnings.
- S/MIME where interoperable; consider OpenPGP as a Linux-native extension.
- Search across headers, bodies, attachments, people, dates, and mailboxes, with fast local results followed by server completion.

## Phase 3: modern intelligence and categorization

- Primary/Transactions/Updates/Promotions categories with an opt-in local classifier.
- Priority summaries, thread summaries, proofreading, and writing assistance behind an explicit local or user-selected provider boundary.
- Extensibility for rules, commands, and integrations without compromising message privacy.

## Apple-specific capability mapping

- **Apple Intelligence:** optional local-model or user-configured provider; never required for core mail.
- **Focus Filters:** integrate with desktop notification profiles / Do Not Disturb where available.
- **Markup:** open attachments in the user's preferred editor and ingest the saved result.
- **Hide My Email and iCloud aliases:** support aliases supplied by the account/provider; do not imitate Apple's relay service.
- **Continuity/Handoff and MailKit extensions:** defer unless a standards-based Linux integration provides a real user benefit.

## Recommended next feature run

Build the **Always-on Mail Foundation** first. It closes current broken paths and establishes the data model required by notifications, threading, multi-account support, and fast opening.

Acceptance criteria:

- A selected cached message opens immediately without waiting for network I/O.
- New mail and metadata changes arrive in the background with reconnect/backoff and desktop notifications.
- Every loaded message has one stable identity regardless of folder, label, local filter, or server search.
- Reply, Reply All, Forward, read/unread, star, labels, archive, and trash work from all valid views.
- Archive and trash offer Undo; Trash supports restore.
- Synchronization is incremental and bounded, never blocks GTK's main thread, and exposes useful progress/error state.
- Tests cover cross-view identity, mutation reconciliation, reconnects, duplicate events, offline startup, and large cached mailboxes.

Suggested command:

`$autofeature:autofeature-do mode:automated Implement Whitford's Always-on Mail Foundation: stable folder-independent message/thread identity; immediate cache-first opening; background Gmail synchronization with bounded polling/IDLE strategy, reconnect/backoff and desktop notifications; make reply/forward and metadata mutations work from folders, labels and server search; add optimistic reconciliation plus Undo for archive/trash and restore from Trash. Keep all network and database work off the GTK main thread, preserve virtualized lists, and add performance/regression tests. [skip-product-review]`

## Definition of parity complete

Maintain a capability matrix for Apple Mail's current documented surface. A feature is complete only when its primary flow, offline/error flow, keyboard path, accessibility state, performance budget, and automated tests pass. Proprietary Apple capabilities require either a documented Linux equivalent or an explicit non-goal.
