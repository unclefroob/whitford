# Feature: Gmail replies
**Date:** 2026-09-20
**Branch:** feature/gmail-replies
**Stack:** Rust 2024, GTK4/libadwaita, Tokio, Lettre SMTP, Gmail OAuth/IMAP
**Status:** In progress (automated)

## Problem
Whitford displays Reply controls but deliberately disables them. Users can read Gmail but must leave the client to answer a message. The app also currently discards the RFC 5322 headers needed to address and thread a correct reply.

## Solution
Add a safe Reply-only MVP: parse and cache bounded reply metadata from the already-downloaded message, open a native composer, and submit a UTF-8 plain-text reply through Gmail SMTP using the existing OAuth grant. Sending runs in a lane independent from IMAP sync/body loading, preserves drafts on failure, prevents duplicate submissions, and never logs content or credentials.

## User Story
As a Gmail user, I want to reply to the open message from Whitford so that I can complete the basic email loop without switching clients.

## Scope: IN
- Reply to `Reply-To`, falling back to `From`, from a fully loaded message.
- Preserve subject and Gmail threading with bounded `In-Reply-To` and `References` headers.
- Native plain-text composer with recipient/subject display, Send, Cancel, keyboard focus, loading, retry, and failure feedback.
- SMTP submission to Gmail over verified TLS with XOAUTH2 and the connected account as From.
- Dedicated send operation state/lane, stale-event rejection, double-send prevention, and retained draft after any failure.
- Refresh inbox after confirmed SMTP acceptance; Gmail owns the Sent copy.

## Scope: OUT
- Reply All, Forward, attachments, rich-text composition, signatures, Gmail Drafts synchronization, arbitrary recipients, and offline send queue.
- Automatic retry after SMTP DATA; delivery may be ambiguous and duplication is worse than an explicit retry decision.

## Existing Code to Touch
- `src/model.rs`, `src/message.rs`, `src/cache.rs`: reply envelope and body-cache version.
- `src/smtp.rs`: bounded message construction and Gmail SMTP transport.
- `src/worker.rs`: independent send command/event/task using in-memory runtime auth.
- `src/state.rs`, `src/state/tests.rs`: composer/send state, validation, stale-result handling.
- `src/ui/{build,mod,render,actions}.rs`: native composer and enabled Reply actions.
- `Cargo.toml`, `README.md`: Lettre and write-capability documentation.

## Edge Cases to Handle
- Missing/invalid Reply-To and From, absent Message-ID, long References, Unicode names/subjects, existing `Re:` prefix.
- Empty or oversized drafts, CR/LF header injection, expired authorization, offline/TLS/timeout/rejection, and uncertain delivery.
- Double Send, selection/account changes during send, closing a dirty composer, and old cached bodies without reply metadata.

## Test Scenarios
- Reply-To wins over From; missing Reply-To falls back; unavailable target disables Reply.
- Outgoing MIME has one `Re:`, valid From/To, and correct bounded threading headers.
- Empty/oversized bodies cannot send; a second Send emits no second command.
- Failures retain draft; stale completions do nothing; success closes once and requests refresh.
- SMTP uses XOAUTH2 over TLS; logs/debug output expose no token, address, subject, or body.

## Assumptions
- Automated mode continues from the current feature run.
- Existing `https://mail.google.com/` consent is reused for SMTP XOAUTH2.
- Reply only is the smallest complete capability; Reply All and Forward remain visibly disabled.
- Drafts are in-memory for the MVP and protected against accidental close while dirty.

## Scope
**Tier:** single-layer — one native desktop application, crossing its model, worker, reducer, and GTK boundaries.

## Effort Plan
**Profile:** balanced, medium floor. SMTP/auth design and critical review use high effort because this sends user data externally; implementation/test/design reviews use medium.

## Product Review
- P0: correct target and threading metadata must be retained before Reply is enabled.
- P0: drafts survive failures; automatic retry is forbidden because post-DATA delivery can be uncertain.
- P0: composer state must not live only in transient reader widgets that are rebuilt by rendering.
- P0: connected account is the immutable From identity; recipients/content/tokens never enter logs.
- P1 deferred: Reply All, quoting controls, autosaved/Gmail drafts, attachments, and rich text.

## Implementation Plan
1. Add bounded reply metadata to `MessageBody`, parse it from the complete RFC 5322 body, and bump the body-cache schema so old entries refetch.
2. Add Lettre with Tokio/Rustls/XOAUTH2; build a pure reply message constructor and a timed Gmail SMTP sender with typed failures.
3. Add a dedicated worker send lane using runtime auth without blocking or cancelling IMAP/body work.
4. Add reducer-owned composer and send request state with validation, generation gating, draft retention, and refresh after accepted delivery.
5. Add a native composer surface and wire only Reply; keep Reply All/Forward disabled.
6. Verify parsing, MIME/threading, injection boundaries, reducer shadow paths, full suite, strict Clippy, release build, installation, and manual send acceptance.

### Performance and security
- Build the MIME message only in the worker. The MVP creates a fresh verified-TLS SMTP transport per explicit send rather than retaining another long-lived copy of the OAuth access token; connection pooling can follow when credentials have a safe rotation/invalidation owner.
- Never refetch a v3 cached message merely to open its composer.
- Cap draft bytes and threading headers, use Lettre's typed mailbox/message builder, and keep SMTP work off GTK.
- Do not persist drafts or SMTP credentials; do not log raw SMTP errors because server text can contain addresses.
