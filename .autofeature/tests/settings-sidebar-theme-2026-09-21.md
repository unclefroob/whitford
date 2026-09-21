---
kind: test-manifest
feature: Settings, Sidebar & System Theme
slug: settings-sidebar-theme
date: 2026-09-21
branch: feature/settings-sidebar-theme
brief: .autofeature/designs/settings-sidebar-theme-2026-09-21.md
platforms: [desktop-linux]
scope: cross-stack
---

# Test Manifest — Settings, Sidebar & System Theme

## Setup

- **Environment:** a local Hyprland/Wayland session running Whitford.
- **Account:** an authenticated Gmail account with Inbox and at least one label.
- **Credentials:** never record credentials in this file.

## Acceptance flows

### AF-1 — Mail-first navigation · priority: critical

1. Start Whitford while connected.
2. Inspect the sidebar and choose Inbox, a primary folder, and a label.
3. Open Settings with the footer entry and with `Ctrl+,`.

Expected: the sidebar contains branding, Compose, primary folders, labels, and Settings only; account, sync, recovery, cache, and storage controls appear in Settings rather than consuming navigation space.

### AF-2 — Appearance follows the system · priority: critical

1. Select **System default** in Settings.
2. Change the Hyprland/desktop color preference while Whitford is open.
3. Restart Whitford.
4. Select Light, then Dark, restarting after each choice.

Expected: System follows live desktop changes and persists as System; explicit Light/Dark overrides persist and apply immediately. App chrome remains readable in both schemes; remote email content remains safely isolated.

### AF-3 — Settings persistence and storage · priority: critical

1. Change the retained-summary limit.
2. Confirm the local-cache usage refreshes after the save.
3. Restart Whitford and reopen Settings.
4. Simulate an unwritable preferences directory if practical, then retry once writable.

Expected: limit and appearance survive restart; pruning is reflected in cache usage; a failed write is clearly recoverable and does not silently discard the in-memory selection.

### AF-4 — Contextual account controls · priority: normal

1. Inspect Settings while normally connected.
2. Disconnect, begin connection, and force a recoverable connection failure.

Expected: normal connected state presents Refresh and Disconnect without irrelevant disabled recovery actions. Connection/retry/browser/cancel actions appear only in their applicable states.

### AF-5 — Navigation accessibility · priority: normal

1. Use `Ctrl+,` to open Settings and `Escape` to return.
2. Navigate sidebar folders with keyboard controls and test a narrow window.

Expected: Settings returns to mail before collapsing reader/sidebar UI; folder navigation remains functional and controls have useful accessible labels.

