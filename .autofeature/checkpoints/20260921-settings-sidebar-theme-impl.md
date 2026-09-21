---
status: post-implementation
branch: feature/settings-sidebar-theme
next_step: review-and-ship
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

