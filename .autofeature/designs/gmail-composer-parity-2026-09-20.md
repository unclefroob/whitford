# Feature: Gmail-style composer parity
**Date:** 2026-09-20
**Branch:** feature/gmail-composer-parity
**Stack:** Rust 2024, GTK4/libadwaita, WebKitGTK, Tokio, Lettre SMTP
**Status:** In progress (automated)

## Problem
Whitford's safe Reply MVP is modal, fixed-recipient, and plain text. It does not meet the expected Gmail Web composition workflow.

## Solution
Replace it with one resumable, non-modal native composer supporting Reply, Reply All, Forward, editable To/Cc/Bcc, core rich formatting, links, inline images, attachments, one account signature, quoted history, private local draft autosave, explicit discard, and compact/expanded pop-out behavior. Preserve XOAUTH2 SMTP, thread correctness, uncertain-delivery safeguards, and reader isolation.

## Scope: IN
- Reply, Reply All, Forward with correct recipients and threading.
- Editable To/Cc/Bcc and subject with validation/deduplication.
- Bold/italic/underline, lists, quote, links, undo/redo, remove formatting.
- Sanitized multipart plain+HTML email.
- Local file attachments and CID inline images with 25 MB conservative limit.
- One account-scoped signature and quoted/forwarded history.
- Private account-scoped atomic local drafts; close/Escape saves, trash explicitly discards.
- One non-modal composer with compact/expanded modes and complete keyboard/accessibility behavior.

## Scope: OUT
- Smart Reply/Compose, scheduled send, confidential mode, Google Drive, templates, read receipts, aliases, send-and-archive, multiple simultaneous composers, Gmail-synced drafts.

## Product constraints
- Closing never implies discard; drafts say “Saved on this device.”
- Only trash deletes a meaningful draft, after confirmation.
- Reply All excludes the connected account and deduplicates; manual edits are authoritative.
- Forward has no reply-thread headers and starts with no recipient.
- Sending flushes visible editor state first; failures and uncertain delivery retain everything.
- Reader WebKit hardening remains unchanged; composer editing is isolated.

## Architecture
- `composer.rs`: generic draft/envelope, modes, recipient policy, sanitizer, quote/signature composition and validation.
- `drafts.rs`: private atomic account-scoped JSON plus staged files, traversal/symlink/bounds protection.
- `smtp.rs`: generic multipart/alternative, related inline CID, mixed attachment construction and XOAUTH2 submission.
- `worker.rs`: generic send plus independent serialized draft/staging lane.
- `state.rs`: composer/draft/save/stage/send generations and revision gates.
- `ui/composer_view.rs`: isolated hardened contenteditable WebKit editor and bounded snapshot bridge.
- GTK composer shell: recipient controls, subject, toolbar, attachment rows, save/send/discard/pop-out states.

## Performance/security gates
- No attachment bytes or full rich document in ordinary inbox snapshots.
- Debounced/coalesced draft saves and editor snapshots; file I/O/MIME/sanitization off GTK.
- Stage selected files privately; never persist arbitrary source paths or log content/addresses/paths.
- Strict allowlist sanitizer, safe URL schemes, no remote loads, scripts, forms, embeds, SVG or event handlers.
- Preflight recipient/content/file/encoded-size limits before MIME allocation.
- Bcc must not appear in serialized visible headers; stale operations and duplicate sends are rejected.

## Acceptance
- Gmail receives threaded Reply/Reply All and unthreaded Forward with correct recipients.
- Rich formatting, link, signature, quote, attachments and inline CID images render in Gmail Web.
- Draft survives close, app restart, offline/auth failure and send failure; success/discard removes it once.
- Large staging/sending never freezes inbox scrolling or body loading.

## Scope
**Tier:** single-layer, high-risk native feature spanning UI, local persistence, MIME and authenticated outbound network I/O.

## Effort Plan
High for domain/security/MIME design and critical review; medium for implementation, UI, testing and documentation. The user approved composer parity and proprietary Gmail features are explicitly excluded.

## Implementation order
1. Freeze generic composer/draft/submission contracts and pure validation/sanitization/MIME tests.
2. Add private draft/staged-file persistence.
3. Generalize worker/reducer from Reply-only to all modes and autosave lifecycle.
4. Replace plain modal editor with non-modal rich composer and recipient/attachment/signature surfaces.
5. Run full tests, adversarial review, release build, install, and manual Gmail acceptance manifest.
