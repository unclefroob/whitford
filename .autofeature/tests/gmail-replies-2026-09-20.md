# Gmail replies — acceptance manifest

**Branch:** `feature/gmail-replies`  
**Platform:** native Linux / Wayland  
**Scope:** Reply-only MVP

## AF-1 — Open composer
Precondition: connected Gmail account and a fully loaded message with a usable Reply-To or From address.

1. Choose Reply from the reader header or footer.
2. Confirm To exactly matches the intended single recipient and Subject has one `Re:` prefix.
3. Confirm keyboard focus enters the empty plain-text editor.

Expected: no network request occurs until Send; Reply All and Forward remain unavailable.

## AF-2 — Send a threaded reply
Use a non-production test conversation and type an identifiable reply, then press Ctrl+Enter once.

Expected: Send disables and shows progress; Gmail accepts one message; the composer closes; the inbox refreshes; Gmail Sent contains one reply in the original thread with the expected recipient and text.

## AF-3 — Validation and duplicate protection
1. Try to send an empty/whitespace draft.
2. Send a valid draft and click/press Send repeatedly while progress is visible.

Expected: empty content is rejected locally; only one SMTP submission is started while sending.

## AF-4 — Failure retention
Disconnect the network before Send, or revoke authorization in the test account.

Expected: the composer remains open with the entire draft intact and shows actionable failure copy. Authorization failures offer Refresh Gmail. No automatic retry occurs.

## AF-5 — Delivery uncertainty
Exercise an indeterminate send only in a controlled test environment by dropping the connection during SMTP submission.

Expected: Whitford says delivery may have succeeded and keeps the draft. Send Again first requires an explicit confirmation instructing the user to check Sent; it never retries automatically.

## AF-6 — Draft-loss protection
Type a draft, then try Cancel, Escape, composer close, and application close.

Expected: each destructive close asks before discarding. Choosing Keep Editing preserves text. While a send is active, close/disconnect is blocked rather than hiding the outcome.

## AF-7 — Privacy and threading
Use messages with Reply-To, From-only, multiple Reply-To values, Unicode subject/name, existing `Re:`, no Message-ID, and multiple References IDs.

Expected: Reply-To wins; the composer displays every actual recipient (the MVP sends exactly one); no duplicate `Re:` appears; available thread IDs are preserved and bounded; absent IDs produce a valid unthreaded reply.

## Automated coverage
- Reply metadata parsing/bounds and cache schema migration.
- Typed mailbox construction, recipient selection, header-injection rejection, size/empty validation, and threading headers.
- Reducer duplicate/stale/success/failure/uncertain/disconnect paths.
- Redacted worker command/events and send-task panic containment.

## Out of scope
Reply All, Forward, rich text, attachments, arbitrary recipients, signatures, Gmail Draft synchronization, and offline queued sending.
