# Lazy message loading acceptance manifest

## Feature

- Scope: performance-first Gmail summary sync, on-demand bodies, body cache management
- Platform: native Rust/GTK4 Wayland desktop
- Branch: `feature/gmail-imap-integration`
- Design: `.autofeature/designs/lazy-message-loading-2026-09-20.md`

## Automated acceptance

- Summary IMAP query is parameterized for 50/100/250/500 and contains bounded header fields plus `BODYSTRUCTURE`, never full `BODY.PEEK[]`.
- Full `BODY.PEEK[]` exists only in the exact single-UID body-open path and never uses a mutating IMAP command.
- Summary mapping cannot invoke full MIME/body parsing; complete opened bodies remain untruncated.
- Authoritative v2 summary cache removes expunged/stale membership and migrates v1 only after successful save.
- Per-message body files round-trip privately, reconcile orphan/corrupt entries, report usage, and obey a 128 MiB LRU budget without truncating the current body.
- Startup/filter normalization performs no body request; deliberate click/keyboard open requests once.
- Stale body results and completions after Clear Cache are ignored.
- Body-only state changes do not change list revision; render guards preserve list/WebView work across unrelated events.
- Required gates: `cargo fmt --check`, strict Clippy, full tests, release build.

## Native acceptance flows

### AF-1 — Fast summary startup

Precondition: authorized Gmail account with at least 100 messages.

1. Choose 100 summaries and restart Whitford.
2. Confirm cached summaries appear while refresh runs.
3. Confirm the list remains responsive and no reader body loads until a message is deliberately opened.

Expected: summary count reaches up to 100; startup does not download 100 complete messages.

### AF-2 — On-demand body and cache hit

1. Open an uncached plain or HTML message.
2. Confirm the reader immediately shows a loading state, then the complete message.
3. Navigate away and reopen it, then restart and reopen it again.

Expected: first open uses Gmail; later opens use the private body cache. HTML safety and remote-image controls remain unchanged.

### AF-3 — Navigation and stale completion

1. Open a slow/large uncached message.
2. Immediately open a different message.

Expected: the first completion never replaces the second reader; list scrolling/focus remains stable.

### AF-4 — Offline behavior

1. Open and cache one message; leave another unopened.
2. Disconnect networking and restart.
3. Open the cached message, then the uncached message.

Expected: cached content opens; uncached content shows “Not available offline” with reader-local Retry. Summary-level offline status remains independent.

### AF-5 — Retention and attachment filtering

1. Raise retention from 50 to 500.
2. Confirm an immediate summary refresh and attachment filtering without body downloads.
3. Lower to 50.

Expected: summaries and orphan bodies prune to 50; selection remains valid.

### AF-6 — Clear Cache

1. Cache several opened bodies and note displayed body count/bytes.
2. Choose Clear Cache and confirm.

Expected: body count becomes zero, summaries and authorization remain, an open body is removed, and no in-flight completion repopulates it.

### AF-7 — Disconnect

1. Choose Disconnect and confirm.

Expected: authorization, summaries, and opened bodies are removed; retention preference remains.

## Out of scope

- Attachment payload downloads, compose/reply, mutation, folders, multi-account, Flatpak verification, and automated GTK/WebKit interaction.
