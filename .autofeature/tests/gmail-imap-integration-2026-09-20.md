# Gmail IMAP integration acceptance manifest

## Automated

- [x] `cargo fmt --check`
- [x] `cargo test` — 69 passed
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo build --release`
- [x] Config path/shape/endpoint bounds and credential redaction tests pass without reading a real credential.
- [x] OAuth URL/callback PKCE, state, duplicate, denial, host, path, and loopback checks pass.
- [x] XOAUTH2 framing, bounded newest-50 sequence UID discovery, complete `BODY.PEEK[]` retrieval, expunge/race/skipped accounting, MIME fallback/HTML/plain-text mapping, and reducer stale-event tests pass.
- [x] Indeterminate/partial credential save tests cover both confirmed cleanup and failed cleanup retention; cleanup failure only offers Disconnect retry.
- [x] Retry regression tests retain Restore/Refresh/Disconnect and route token, userinfo, and IMAP authentication rejection to Connect.
- [x] WorkerUnavailable is non-retryable with restart guidance, and command-send failure immediately leaves progress state.
- [x] Config reads are bounded to maximum plus one byte from one opened handle; exact-limit and limit-plus-one tests pass.
- [x] Future timestamp formatting never emits negative “ago” copy, and browser-launch recovery copy names only available actions.
- [x] Configuration failures preserve missing/unreadable/too-large/invalid-or-wrong-type/wrong-project/directory-unavailable categories and safe resolved paths through worker/state copy.
- [x] Worker phases and service failures use exhaustive human-readable presentation mappings; no Rust `Debug` labels or raw Unix timestamps remain in user-facing UI.
- [x] Browser-launch failure remains visible with recovery guidance after the worker acknowledges cancellation.
- [x] Pure time-formatting tests cover relative boundaries and deterministic absolute local-offset output.
- [x] Auth-required and failed-disconnect reducer tests retain selected mail under explicit degraded status; worker loss retains mail as offline/stale.
- [x] Partial-sync copy hides zero counters and reader fallback copy is explicit.
- [x] Application lifecycle retains the worker OS thread, sends shutdown, and joins it after GTK exits; deterministic coverage proves shutdown closes the event channel before join returns.
- [x] Retryable startup configuration failure enables only `win.retry` and preserves `Restore`, so an existing saved authorization resumes without competing browser Connect.
- [x] Multipart newsletters prefer sanitized HTML-derived visible text over generated plain-text tracking URLs; a regression fixture proves link labels remain while destinations are omitted.
- [x] HTML messages render in an ephemeral WebKitGTK session with JavaScript, embedded navigation, downloads, and remote images blocked by default; the per-message image opt-in only expands `img-src`.
- [x] The reader has no byte-range fetch, body cap, truncated model state, or truncation banner; the newest-50 sync requests each complete RFC 5322 message.

## Native Wayland manual

- [ ] Fresh start without config shows developer-preview setup guidance.
- [ ] Missing, unreadable, too-large, invalid/wrong-type, wrong-project, and unavailable-directory config cases show the exact safe path and corrective action without credential values.
- [x] First Connect launches the browser and loads Gmail INBOX read-only.
- [ ] Cancel and Reopen Browser behave safely; late callbacks do not change state.
- [x] Restart restores the Secret Service token without another consent prompt.
- [ ] Refresh updates the snapshot and actual loaded/fallback/skipped metadata.
- [ ] Offline/provider failure retains in-session mail and offers retry.
- [ ] Offline, auth-required, and failed-disconnect banners remain above both message list and reader at narrow width and show the real last successful sync.
- [ ] Revoked authorization requires reconnect and does not retry-loop.
- [ ] Empty inbox and malformed mail render deterministic fallbacks.
- [ ] Partial-sync banners hide zero counters; fallback messages show a reader-level indication.
- [ ] Only INBOX is available; compose/archive/delete/read/star/label/download controls and accelerators cannot report success.
- [ ] Disconnect confirmation distinguishes local removal from Google revocation; successful cleanup clears mail, failed cleanup retains it.
- [ ] Logs contain no credential, OAuth URL/query/state/code/token, identity, UID, sender, subject, body, attachment name, or search text.
- [ ] Closing the native Wayland window during an indeterminate Secret Service save/delete remains responsive, then waits only for the bounded cleanup before process exit.

## Flatpak manual

- [ ] Status remains unverified/non-runnable scaffolding until the following build/runtime checks are completed; native is the golden path.
- [ ] Offline Cargo sources generated and `flatpak-builder` succeeds with GNOME 50 runtime.
- [ ] Sandbox config path is accepted without host filesystem access.
- [ ] OpenURI portal reaches the loopback callback and Secret Service prompts/operations work.
