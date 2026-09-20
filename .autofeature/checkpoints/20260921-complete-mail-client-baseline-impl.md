---
status: post-implementation
branch: feature/complete-mail-client-baseline
next_step: native-live-acceptance
scope: cross-stack-equivalent-single-repo
---

## Working on: Complete Mail Client Baseline

### Implemented

- Multi-draft restoration and secure, retry-safe account cleanup.
- Stable Gmail identity, Special-Use folder discovery and bounded folder-aware cache.
- Inbox read/star/archive/trash/label mutations with optimistic reconciliation.
- Standalone rich message composition.
- Cached-first Gmail folder and label navigation.
- Lazy exact-part received attachment Open/Save As.
- Explicit full-mailbox Gmail search with instant local filtering.
- Virtualized message list and hardened account/session boundaries.

### Verification

- Formatting: PASS
- Strict Clippy, all targets/features: PASS
- Unit tests: 187 passed
- Optimized release build: PASS
- Pre-ship critical/testing/UI review: blockers repaired

### Remaining

1. Install the release binary and launch under native Wayland.
2. Run the live-account flows in `.autofeature/tests/complete-mail-client-baseline-2026-09-21.md`.
3. Push/open a PR when a Git remote exists.
