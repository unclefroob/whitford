# Gmail-style composer parity — acceptance manifest

**Branch:** `feature/gmail-composer-parity`  
**Platform:** native Linux / Wayland  
**Drafts:** local-only, private, account-scoped

## AF-1 — Reply and Reply All
Open a loaded test conversation and exercise Reply and Reply All.

Expected: Reply-To wins over From; Reply All includes original To/Cc, excludes the connected account, deduplicates recipients, and preserves Gmail threading. Manually edited To/Cc/Bcc values are authoritative.

## AF-2 — Forward
Open Forward, add recipients, edit the subject/body, and send.

Expected: subject has one `Fwd:` prefix, forwarded history is present, and no `In-Reply-To`/`References` headers join the original thread.

## AF-3 — Rich formatting
Exercise bold, italic, underline, lists, quote, link, undo/redo, remove formatting, paste, and Unicode. Send to a test Gmail account.

Expected: Gmail renders sanitized HTML and a readable plain-text alternative. Pasted scripts, handlers, forms, remote embeds, and unsafe URLs do not survive or execute.

## AF-4 — Attachments and inline images
Attach multiple files, remove one, drag in a file, insert and remove an inline image, and send.

Expected: visible totals are correct, Send is blocked while staging, missing/oversized/excess files produce clear retained-draft errors, regular attachments arrive once, and inline images render through CID without local paths.

## AF-5 — Signature and quoted content
Configure a multiline signature, create a new reply, expand/collapse/remove quoted history, and send.

Expected: signature is inserted once only into new drafts; quote starts collapsed; changing the preference does not rewrite an existing draft.

## AF-6 — Draft lifecycle
Edit recipients, subject, rich body, and files. Wait for “Saved on this device,” close with Escape, restart Whitford, and Resume Draft.

Expected: all content returns. Closing never discards; trash explicitly confirms and removes draft/staged files. A save failure keeps the composer visible and reports failure honestly.

## AF-7 — Window and keyboard behavior
Browse mail while the non-modal composer is open; toggle compact/expanded size; test Ctrl+B/I/U/K/Z/Shift+Z and Ctrl+Enter; test a 600×560 window.

Expected: editor state/focus survives, bottom actions remain reachable via scrolling, and sending cannot start twice.

## AF-8 — Failure and privacy boundaries
Test offline/auth rejection and, in a controlled environment, an indeterminate SMTP completion.

Expected: draft remains intact; uncertain delivery requires checking Sent and explicit resend confirmation; account switching never exposes another account’s draft/signature; Bcc is absent from serialized message headers; logs contain no content, recipients, filenames, paths, IDs, or credentials.

## Automated coverage
- Composer constructors, recipient policy/deduplication, subject/thread semantics and HTML sanitization.
- Multipart alternative/related/mixed MIME, CID parts, Bcc privacy and bounds.
- Private atomic drafts, staging quotas, traversal/symlink defenses, cleanup and account isolation.
- Draft save/load/delete/stage generations, stale-event rejection, send uncertainty and worker failure.
- Inline editor synchronization, attachment staging send gate, and honest save-close state.

## Out of scope
Smart Reply/Compose, scheduled send, confidential mode, Drive integration, templates, read receipts, aliases, send-and-archive, multiple simultaneous composers, and Gmail-synced drafts.
