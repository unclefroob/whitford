---
status: native-live-smoke-partial
branch: feature/gmail-imap-integration
next_step: native-manual-acceptance
scope: single-layer
---

## Working on: Gmail OAuth and IMAP integration

### Implemented

- Owned, protocol-neutral mail/account/snapshot model and pure reducer with operation IDs, stale-event rejection, offline retention, disconnect cleanup semantics, and read-only mutation guards.
- Bounded XDG Google desktop-client config validation, PKCE/state loopback authorization, token refresh, verified userinfo identity, and redacted secret wrappers.
- Freedesktop Secret Service refresh-token persistence using DH encryption with no plaintext fallback.
- rustls Gmail IMAP XOAUTH2, `EXAMINE INBOX`, UID search, newest-50 selection, one bounded `BODY.PEEK` fetch, MIME/plain-text mapping, and partial fallbacks.
- Dedicated Tokio worker thread with nonblocking GTK handoff, cancellation/supersession, bounded waits, browser launch recovery, and secure disconnect.
- Developer-preview GTK copy/actions, INBOX-only navigation, freshness/limit/partial metadata, disabled mutations/accelerators, Flatpak permissions, README, and acceptance manifest.

### Verification

- `cargo fmt --check`: PASS
- `cargo test`: PASS — 46 passed, 0 failed
- `cargo clippy --all-targets -- -D warnings`: PASS
- `cargo build --release`: PASS
- Credential artifact scan: PASS — no credential/token/env paths tracked or present in the repository scan

### Remaining manual acceptance

- Native Wayland browser/loopback authorization, live Gmail read, restart restore, manual refresh, offline/revoked-token recovery, malformed/empty inbox, and Secret Service disconnect flows.
- Flatpak offline-source build and portal/Secret Service checks with GNOME 50 SDK.

No commit, push, live OAuth credential read, network account access, or Google-side mutation was performed.

### First review-repair pass

- Hardened loopback callback reads with one absolute deadline, incremental header framing, exact 8 KiB limits, and generated response length.
- Added stage-aware cancellation cleanup, task panic/channel failure recovery, default-collection-only Secret Service access, bounded streamed provider bodies, linear HTML danger-block stripping, retained offline rendering, re-consent recovery, skipped IMAP accounting, verified identity copy, and disconnect-cleanup guidance.
- Expanded display-independent coverage from 23 to 46 tests. Full live IMAP wire behavior remains a named manual check; pure command-plan and fetch-classification tests prove the application-owned command vocabulary and edge cases.

### Second design/DX review-repair pass

- Preserved six redacted OAuth configuration failure categories and safe resolved paths through worker, reducer, and presentation layers, with exact corrective copy.
- Replaced user-visible Rust `Debug` labels and Unix epochs with exhaustive human copy plus tested relative/absolute local time formatting.
- Added dedicated adaptive developer-preview onboarding, list/reader recovery banners, explicit degraded retained-mail states, real last-sync display, and state-specific accessible actions.
- Added partial-sync summaries that omit zero counters and message-level fallback/truncation notices.
- Reworked README setup into numbered Google Cloud steps, safe native/Flatpak installation commands, seven-day External Testing caveat, reproducible manual acceptance, and logging/redaction guidance. Flatpak is explicitly unverified/non-runnable scaffolding; native remains the golden path.
- Expanded display-independent coverage from 46 to 55 tests.

### Verification after second repair

- `cargo fmt --check`: PASS
- `cargo test`: PASS — 55 passed, 0 failed
- `cargo clippy --all-targets -- -D warnings`: PASS
- `cargo build --release`: PASS
- `git diff --check`: PASS

Native Wayland UI, live Google consent/IMAP/Secret Service, forced disconnect-cleanup failure, and all Flatpak checks remain manual and were not claimed.

### Final security/UX review repair

- Made indeterminate credential persistence cleanup-conservative: only confirmed deletion clears the marker, while failed deletion becomes cleanup-required `DisconnectFailed` with Disconnect-only retry.
- Retained exact reducer recovery operations, routed authorization rejections to Connect, made worker loss restart-only, and immediately dispatch worker loss on command-send failure.
- Replaced unbounded `UID SEARCH ALL` with a bounded newest-at-most-50 sequence `FETCH (UID)` followed by one UID `BODY.PEEK` batch, including race/skipped accounting.
- Removed the config metadata/read race with one open handle and a maximum-plus-one bounded read.
- Corrected browser-launch and future-time copy and updated AppStream metadata for the real read-only Gmail preview.
- Expanded display-independent coverage from 55 to 63 tests.

### Verification after final repair

- `cargo fmt --check`: PASS
- `cargo test`: PASS — 63 passed, 0 failed
- `cargo clippy --all-targets -- -D warnings`: PASS
- `cargo build --release`: PASS
- `git diff --check`: PASS
- Credential artifact scan: PASS — no credential filenames, private-key material, or Google token/key prefixes found outside ignored build artifacts or in the diff

Residual manual-only checks remain native Wayland browser/loopback authorization, live Gmail read/restore/refresh, actual Secret Service prompts plus forced real cleanup failure, offline/revoked-token flows, and Flatpak build/portal/Secret Service behavior. No live credentials or accounts were accessed.

### Security closure repair

- Retained the OS worker-thread handle in the application lifecycle, sent `Shutdown` on window destruction and again defensively after GTK exits, and joined after `application.run()` so bounded indeterminate Secret Service cleanup completes before process exit without blocking the normal GTK close callback.
- Cleared the strong UI holder on window destruction; actions and async callbacks remain weak-UI plus cloned-sender only, avoiding a permanent application/window ownership path and any possibility of joining from the worker.
- Made retryable startup configuration errors expose only `win.retry`; reducer recovery remains `Sync(Restore)`, so fixing configuration resumes the saved-token restore rather than starting competing browser consent.
- Added deterministic worker lifecycle, reducer, and presentation regressions. Display-independent coverage is now 66 tests.

### Verification after security closure

- `cargo fmt --check`: PASS
- `cargo test`: PASS — 66 passed, 0 failed
- `cargo clippy --all-targets -- -D warnings`: PASS
- `cargo build --release`: PASS
- `git diff --check`: PASS
- Credential artifact scan: PASS — no credential filenames, private-key material, or Google token/key prefixes found outside ignored build artifacts or in the diff

Residual manual-only checks remain native Wayland close during an indeterminate Secret Service operation, browser/loopback authorization, live Gmail read/restore/refresh, actual Secret Service prompts plus forced real cleanup failure, offline/revoked-token flows, and Flatpak build/portal/Secret Service behavior. No live credentials or accounts were accessed.

### Native live smoke and newsletter readability repair

- Completed browser/loopback authorization against the configured Gmail test account and loaded the newest INBOX messages over read-only IMAP.
- Restarted the native Wayland app and confirmed Secret Service restored authorization without another consent prompt.
- Corrected multipart selection after a real newsletter exposed generated plain-text tracking URLs: Whitford now prefers sanitized HTML-derived visible text and falls back to `text/plain` only when needed.
- Added a regression fixture that retains human link labels while omitting tracking destinations.

### Verification after live repair

- `cargo fmt --check`: PASS
- `cargo test`: PASS — 67 passed, 0 failed
- `cargo clippy --all-targets -- -D warnings`: PASS
- `cargo build --release`: PASS
- `git diff --check`: PASS

Live authorization, Gmail INBOX loading, and saved-token restart restoration are now verified. Manual refresh timing, failure/recovery cases, disconnect cleanup, malformed live mail, responsive edge states, and Flatpak remain open acceptance items.

### Full HTML reader repair

- Replaced the GTK plain-text body label with WebKitGTK HTML rendering backed by an ephemeral network session.
- Disabled JavaScript, embedded navigation, permission requests, downloads, persistent web storage, media, WebRTC, and remote images by default; links open externally only after a user gesture and remote images require a per-message opt-in.
- Removed the IMAP byte-range request, reader body caps, truncated state, and truncation banner. Whitford now requests the complete message with `BODY.PEEK[]` and renders its complete HTML or plain-text body.
- Corrected MIME selection so only a real `text/html` part creates a WebKit reader; plain-text messages retain the lightweight GTK label.

### Verification after full-reader repair

- `cargo fmt --check`: PASS
- `cargo test`: PASS — 69 passed, 0 failed
- `cargo clippy --all-targets -- -D warnings`: PASS
- `cargo build --release`: PASS
- `git diff --check`: PASS
- Native installed release: launched, Gmail authorization restored, INBOX sync completed, and WebKit content process started.
