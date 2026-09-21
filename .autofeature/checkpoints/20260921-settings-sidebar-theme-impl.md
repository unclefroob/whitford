---
status: shipped-for-review
branch: feature/settings-sidebar-theme
next_step: merge-pr
scope: cross-stack
---

## Working on: Settings, Sidebar & System Theme

### Implemented

- Mail-first sidebar with primary folders, separate labels, and a compact Settings entry.
- NavigationView Settings page for appearance, sync, storage, account, and privacy controls.
- Persisted System/Light/Dark appearance preferences, including retention-only preference migration.
- Immediate theme application with libadwaita semantic colors and desktop-system default behavior.
- Acknowledged preference saves with stale-event fencing, retryable failures, and cache-usage refresh after retention pruning.

### Verification

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- `cargo build --release`
- `git diff --check`

### Completed

PR: https://github.com/unclefroob/whitford/pull/2
Status: ready for review; local release binary installed at `/home/ryan/.local/bin/whitford`.
