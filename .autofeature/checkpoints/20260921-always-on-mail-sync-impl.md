---
status: post-implementation
branch: feature/unified-mail-actions
next_step: ship
scope: cross-stack
---

## Working on: Always-on Mail Sync

### Implemented

- One-shot, generation-gated background Inbox polling with bounded backoff.
- Worker-only Gmail/token/cache work, with foreground metadata work not blocked by remote polling.
- Durable silent-baseline and unread-ID notification watermark.
- Account epoch fencing to prevent stale background commits or token writes after account transitions.
- Count-only native desktop notifications with Open Inbox action.
- State/UI isolation for active search and reader views, plus accessible background status copy.

### Verification

- `cargo fmt --check`: PASS
- `cargo clippy --all-targets --all-features -- -D warnings`: PASS
- `cargo test --all-targets --all-features`: PASS (219 library tests)
- `cargo build --release`: PASS
- Critical, testing, and GTK pre-ship findings: repaired and re-verified.

### Completed

PR: https://github.com/unclefroob/whitford/pull/1
Status: SHIPPED
