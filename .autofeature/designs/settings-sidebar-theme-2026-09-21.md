# Feature: Settings, Sidebar, and System Theme

**Date:** 2026-09-21  
**Branch:** feature/settings-sidebar-theme  
**Stack:** Rust 2024 · GTK4/libadwaita · Wayland  
**Status:** Draft

## Problem

Whitford’s sidebar currently tries to be navigation, account onboarding, refresh controls, cache management, and sync diagnostics at once. It consumes most of the vertical space and makes common folders feel secondary. It also forces a dark theme even when the system is light, which breaks the expected Hyprland desktop experience.

## Solution

Make the sidebar mail-first: a compact account header, primary mailboxes, and a clearly separated labels section. Move account, storage, sync, notification, and appearance preferences into a dedicated Settings page. Adopt libadwaita’s system color scheme by default, while optionally allowing the user to choose System, Light, or Dark and persisting that choice privately.

## User Story

As a Whitford user, I want the sidebar to focus on mail navigation and a clear Settings page for client preferences, so the app feels calm, native, and aligned with my system theme.

## Scope: IN

- Stop forcing dark mode; default to the system scheme.
- Appearance setting: System default, Light, Dark; persist and apply immediately.
- Dedicated Settings page opened from the sidebar/header and keyboard action.
- Settings sections: Appearance, Synchronization, Storage, Account, and About/Privacy.
- Move cache retention, cache usage/clear action, account email/disconnect, sync status, and background notification summary out of the main sidebar.
- Compact sidebar with primary folders, a separate Labels group, duplicate special-folder label suppression, and a footer Settings entry.
- Preserve accessible labels, keyboard navigation, and narrow-window navigation behavior.

## Scope: OUT

- Multiple accounts, account switching, advanced per-label notification rules, custom CSS themes, and editable Gmail labels/mailboxes.
- A full GNOME control-center integration; this stays a self-contained libadwaita settings surface.

## Existing Code to Touch

- `src/main.rs`: system color-scheme startup/default behavior.
- `src/cache.rs`: versioned preferences read/write for appearance selection alongside retention.
- `src/state.rs`: appearance preference/action and settings snapshot state.
- `src/ui/build.rs`, `src/ui/actions.rs`, `src/ui/mod.rs`, `src/ui/render.rs`: settings page, compact sidebar, actions, and immediate theme application.
- `src/style.css`: remove dark-only assumptions and ensure readable system light/dark surfaces.

## Edge Cases to Handle

- Existing preference file migration and failed preference write must not lose the active UI state or falsely claim persistence.
- System theme changes while System is selected update naturally; Light/Dark override immediately.
- Disconnected/offline users can still open settings and see meaningful disabled/recovery states.
- Settings navigation from a narrow/collapsed layout must retain a valid back path.
- A user label named like a special folder must not create confusing duplicate navigation entries.

## Test Scenarios

- Fresh install follows system light/dark preference; choosing Light/Dark/System maps to the correct libadwaita scheme and persists across restart.
- Cache retention change, cache clear, account disconnect, and sync status remain available from Settings.
- Sidebar shows primary folders first, labels separately, and Settings without the former account-control clutter.
- Preference-write failure reports a recoverable status and preserves the current selection safely.

## Assumptions

- Keep the current focused dark visual language, but let libadwaita/system colors provide the base palette instead of forcing dark mode.
- Default settings route is a dedicated navigation page, not a modal, so it works comfortably on desktop and narrow layouts.

## Scope

**Tier:** cross-stack (persistent preferences, state, GTK shell/rendering, system integration)

**Subagents:** preferences/state, GTK layout/settings, theme/accessibility — all `gpt-5.6-terra`.

**Skills:** automated planning, direct Cargo verification, pre-ship review; product review skipped because this feature directly addresses a live UX report.

## Effort Plan

**Profile:** forced:medium (`gpt-5.6-terra` for all delegated work)
**Rules fired:** none

| Task | Model | Effort | Why |
|---|---|---|---|
| Preferences/state design | gpt-5.6-terra | medium | persistence and reducer behavior |
| GTK/settings design | gpt-5.6-terra | medium | navigation and interaction |
| Theme/accessibility design | gpt-5.6-terra | medium | system integration and visual safety |
| Implementation/review | gpt-5.6-terra | medium | user-requested model |

**Escalations during run:** none

## Implementation Plan

1. **Make preferences a versioned, single source of truth (`cache.rs`, then `state.rs`).** Replace the retention-only private JSON shape with `Preferences { version: 1, retained_messages, appearance }`, where `AppearancePreference` is the serde-stable `System | Light | Dark` enum and its default is `System`. Read a missing `version`/`appearance` as the existing retention value plus `System`; reject an invalid retention value as today, but never fail startup because an old or malformed appearance value is present. Keep the existing `write_private_json` atomic, mode-0600 path and expose `load_preferences()` plus a save operation that preserves both fields, so changing retention cannot erase appearance (or vice versa). Add `_at` helpers/tests for old retention-only JSON, missing file, invalid new fields, round-trip, and write error.

2. **Give preference changes an acknowledged state contract (`state.rs`, `worker.rs`).** Store the loaded appearance preference and a small persistence status in `AppState`; include both in `ViewSnapshot`. Add `Action::SetAppearance(AppearancePreference)` and a worker command/event pair carrying the full preferences snapshot and a request/generation ID (for example `SavePreferences` -> `PreferencesSaved { request_id, result }`). Reducer contract: an accepted selection updates the in-memory preference immediately and emits the save effect; only the matching completion changes persistence status. On a write error, retain the active selection/theme, mark it unsaved, and return recoverable feedback such as “Appearance changed, but could not be saved”; do not silently claim persistence or roll back the visible choice. Route the existing cache-limit save through the same full-snapshot path or otherwise serialize it with appearance saves, preventing last-writer field loss. Do not move blocking filesystem work onto GTK’s main thread.

3. **Apply the libadwaita scheme at startup and on every accepted selection (`main.rs`, `ui/mod.rs`).** Remove `ForceDark`. Convert the persisted enum at the UI boundary to `adw::ColorScheme::{Default, ForceLight, ForceDark}` and call `StyleManager::default().set_color_scheme(...)` before the first window is presented, then from the appearance action’s render/effect path. `System` must use `Default`, allowing live desktop light/dark changes; Light and Dark must override immediately. The UI is the sole style-manager writer, while state remains GTK-free and testable.

4. **Introduce a real settings route without replacing mail navigation (`ui/build.rs`, `ui/actions.rs`, `ui/mod.rs`).** Put the existing mail shell (`OverlaySplitView` + inner `NavigationSplitView`) in an `adw::NavigationView` root page and create a tagged `adw::NavigationPage` for Settings; push/pop that page rather than showing a dialog or conditionally swapping an arbitrary child. Add `win.open-settings` (with a documented accelerator such as `<Primary>comma`) and a footer Settings button in the folder sidebar. The settings page uses `adw::PreferencesPage`/groups: Appearance (three-choice `adw::ComboRow` or equivalent), Synchronization (existing status plus Refresh/Retry/recovery controls), Storage (retention, usage, Clear Cache), Account (verified address, connect/disconnect), and About & Privacy (local-data/authorization explanation). Reuse the current actions and confirmation effects, preserving their enablement. Its navigation header supplies a back button; `Escape`/back must pop Settings before the existing search/reader/folder collapse rules, so narrow windows always have a valid return path.

5. **Project sidebar semantics from the catalog instead of rendering every folder (`state.rs`, `ui/render.rs`).** Replace the flat sidebar input in `ViewSnapshot` with a semantic projection such as `SidebarFolders { primary: Vec<Folder>, labels: Vec<Folder> }`, retaining the full catalog for message-label actions. `primary` contains only non-`Label` kinds in the catalog’s existing rank order; `labels` contains label kinds, sorted as supplied, after filtering label display names that case-insensitively equal a displayed primary special-folder name (so a label named Inbox/Sent/etc. cannot produce two indistinguishable destinations). Render primary rows, then a visible “Labels” heading and label rows only when labels remain. Keep selection state, icon/count accessibility text, click behavior, and collapsed-sidebar dismissal; update Alt+Up/Down to traverse the displayed projection, not hidden duplicates. The compact header retains only identity/Compose as appropriate and the Settings footer—move refresh, disconnect, cache, and diagnostics exclusively to Settings.

6. **Make CSS color-scheme safe, not a visual redesign (`style.css`).** Remove `@define-color` values and literal dark surfaces/text/white-alpha hover assumptions that force a dark canvas. Rebase app surfaces, text, borders, selection, muted copy, focus, destructive/suggested controls, and WebKit container edges on libadwaita/GTK semantic symbols (for example `@window_bg_color`, `@view_bg_color`, `@card_bg_color`, `@window_fg_color`, `@borders`, `@accent_bg_color`, `@accent_fg_color`, `@insensitive_fg_color`, plus `alpha()` only over semantic colors). Preserve spacing, hierarchy, focus visibility, and the mail-reader safety boundary; do not add a custom theme system or redesign unrelated composer/message surfaces.

7. **Verify behavior at the right layers.** Add cache unit tests for migration and atomic save failure; reducer tests for immediate selection, stale completion ignored, persistence failure retaining the active preference, and no preference-field clobber during rapid retention/appearance changes; projection tests for order, empty labels, and special-name suppression. Add focused GTK/UI tests where feasible for action/page push-pop, setting controls reflecting snapshots, and accessible names/states. Run `cargo fmt`, `cargo test`, and `cargo clippy --all-targets -- -D warnings`; manually exercise fresh/system-light and system-dark starts, live system flips while System is selected, Light/Dark overrides and restart persistence, simulated write failure with toast/status, disconnected/offline Settings states, cache clear/disconnect confirmations, keyboard navigation, and narrow-width Settings back navigation. Keep rendering incremental: only rebuild folder rows when the folder projection changes, and never trigger mail refreshes merely by opening Settings or changing theme.
