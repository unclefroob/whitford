---
kind: test-manifest
feature: Wayland Mail Shell MVP
slug: wayland-mail-shell
date: 2026-09-20
branch: feature/wayland-mail-shell
pr: ""
brief: .autofeature/designs/wayland-mail-shell-2026-09-20.md
platforms: [linux-desktop, wayland]
scope: single-layer
---

# Test Manifest — Wayland Mail Shell MVP

## Setup

- **Environment:** local native Wayland session; Hyprland is the primary target.
- **Credentials/roles needed:** none.
- **Seed data:** deterministic fixtures are embedded in the binary.
- **Feature flags/config:** none.
- **Dependencies:** Rust 1.98+, GTK 4.22+, and libadwaita 1.9+. Run with `GDK_BACKEND=wayland cargo run`.

## Surfaces built

### Linux desktop surfaces

| Surface | Purpose |
|---------|---------|
| Folder rail | Account identity, compose affordance, folders, unread/message counts, and sync state |
| Message list | Search, All/Unread/Attachments filters, selection, unread/star/attachment cues |
| Reader | Message metadata and body, attachment card, reader toolbar, and deferred reply controls |
| Adaptive navigation | Folder overlay below 1000sp and list/reader navigation below 650sp |
| Preview states | Online, loading, offline, empty folder, and no-results states |

## Acceptance flows

### AF-1 — Launch and browse the fixture inbox · priority: critical
- **Precondition:** dependencies installed in an active Wayland session.
- **Steps:**
  1. Run `GDK_BACKEND=wayland cargo run`.
  2. Select several Inbox messages.
  3. Select Drafts, Sent, Archive, Starred, and Trash.
- **Expected:** the native window opens with three panes; the first Inbox message is selected; folder counts, message rows, and reader update together; Trash shows the deliberate empty-folder state.

### AF-2 — Search and filter locally · priority: critical
- **Precondition:** app open on Inbox.
- **Steps:**
  1. Press Ctrl+F and search for `roadmap`.
  2. Clear the search.
  3. Select Unread, Attachments, and All.
  4. Search for an unmatched phrase.
- **Expected:** matching is case-insensitive across sender, subject, and preview; selection repairs to a visible message; filters show only their matching fixture rows; unmatched search shows `No matches`; clearing restores the list.

### AF-3 — Use keyboard navigation and archive · priority: critical
- **Precondition:** app open on Inbox with a message selected.
- **Steps:**
  1. Use Ctrl+Down/Ctrl+Up to move through messages.
  2. Use Alt+Down/Alt+Up to move through folders.
  3. Return to Inbox and press Delete.
- **Expected:** navigation remains bounded; archive removes the selected row from Inbox, updates Inbox/Archive counts, selects the next sensible message, and shows `Message archived`.

### AF-4 — Verify adaptive navigation · priority: critical
- **Precondition:** app open in a resizable window.
- **Steps:**
  1. Resize to roughly 900×760.
  2. Open and close the folder overlay.
  3. Resize to roughly 600×760, open a message, then press Escape.
  4. Resize back to roughly 1584×982.
- **Expected:** folders become an overlay below 1000sp; list and reader become navigable pages below 650sp with a working return path; the wide three-pane layout returns without losing selection.

### AF-5 — Verify loading, offline, and deferred actions · priority: normal
- **Precondition:** app open.
- **Steps:**
  1. Press Ctrl+Shift+2, Ctrl+Shift+3, then Ctrl+Shift+1.
  2. Invoke Compose, Reply, Star, Delete, Label, and attachment Download controls.
- **Expected:** loading/offline status is consistent across list, reader, and footer; returning online restores mail; deferred controls show honest milestone toasts and never pretend to complete work.

### AF-6 — Verify accessibility and long-content resilience · priority: normal
- **Precondition:** app open at wide and narrow widths.
- **Steps:**
  1. Traverse primary controls using the keyboard.
  2. Inspect focus visibility and tooltips.
  3. Open messages with long subjects, missing initials/timestamps, and attachment details.
- **Expected:** focus remains visible; icon controls have descriptive accessible labels/tooltips; selected/unread/starred/attachment states are exposed; long values truncate or wrap without displacing trailing controls; optional data uses explicit fallbacks.

## Out of scope / not covered

- Live accounts, IMAP, SMTP, OAuth, Secret Service, synchronization, or persistence.
- MIME parsing, HTML/WebKit rendering, remote content, and real attachment access.
- Working compose/reply/forward/delete/star/label actions beyond honest deferred feedback.
- Multi-account behavior, notifications, settings, light theme, final branding, and plugins.
- Verified Flatpak build; metadata exists, but `flatpak-builder` and the SDK/runtime are not installed locally.
