# Feature: Wayland Mail Shell MVP
**Date:** 2026-09-20
**Branch:** feature/wayland-mail-shell
**Stack:** Rust 1.98 · GTK4 4.22 · libadwaita 1.9 · Wayland
**Status:** Approved for automated implementation

## Problem
Linux users on compositors such as Hyprland lack a lightweight native mail client that combines a polished, modern interface with a fast keyboard-first workflow. Existing options often feel visually dated, overly broad, or visually tied to a particular desktop environment.

## Solution
Build the first vertical slice as a fixture-driven native desktop shell matching the approved mockup in `docs/design/approved-mockup.png`. This milestone proves the interaction model, custom visual language, responsiveness, and native build before introducing the much riskier account, protocol, MIME, and HTML-rendering layers.

## User Story
As a design-conscious Wayland user, I want a fast and attractive desktop inbox shell so that I can browse, search, select, and act on mail through a focused keyboard-friendly interface.

## Context
- The repository is greenfield: no prior code, conventions, tests, or Git history.
- Rust 1.98.1, Cargo, rustfmt, and Clippy are installed.
- GTK4 4.22.5 and libadwaita 1.9.4 development libraries are installed.
- The current desktop session is Wayland under Hyprland.
- Flatpak is installed, but `flatpak-builder` and SDK runtimes are absent; author metadata now and defer build verification.
- Blueprint is unavailable; use Rust-built widgets and embedded CSS for the fastest MVP.
- The generated mockup has been copied into the repository as the visual contract.

## Assumptions
- `whitford` is a working project/app ID name, not a final brand decision.
- Dark mode is the only polished theme in this milestone.
- Local fixture data is authoritative; persistence and live network behavior are explicitly deferred.
- Custom styling should feel at home on Wayland without relying on a GNOME Shell workflow.
- Product review is skipped for speed because the preceding focused feature review already established the MVP.

## Scope: IN
- Cargo application scaffold and Git repository.
- GTK4/libadwaita application window with custom client styling.
- Wide three-pane layout: folders, message list, and reader.
- Progressive narrow-window behavior with usable navigation between list and reader.
- Reusable design tokens and component-building helpers.
- Realistic, deterministic account, folder, message, and attachment fixtures.
- Folder navigation, unread counts, message selection, and local search filtering.
- Reader toolbar, message metadata, attachment presentation, and reply action affordances.
- Keyboard shortcuts for compose, focus search, folder navigation, message navigation, archive, reply, and escape/back.
- Loading, empty-folder, no-search-results, offline, and selected-message states.
- Accessible labels/tooltips/focus behavior for primary controls.
- Unit tests for filtering, selection, folder counts, and navigation state.
- README, developer commands, desktop metadata, and early Flatpak manifest.

## Scope: OUT
- IMAP, SMTP, OAuth, Secret Service, account onboarding, or background sync.
- MIME parsing, HTML/WebKit rendering, remote images, or message sanitization.
- Durable local database, drafts, attachment file access, and notifications.
- Multi-account behavior, server-side search, settings, themes, or plugins.
- Final branding and light-theme polish.
- Verified Flatpak build, because the local builder and runtime are unavailable.

## Existing Code to Touch
- None. This is a new application.
- `docs/design/approved-mockup.png`: visual reference for the implementation.

## Edge Cases to Handle
- Empty folders and searches with zero matches.
- A selected message disappearing when folder or search filters change.
- Long sender names, subjects, snippets, and attachment names.
- Keyboard navigation at the first and last visible message.
- A narrow window must preserve a route back from the reader to the message list.
- Missing optional preview, sender initials, timestamp, or attachment metadata.

## Test Scenarios
- Launch shows fixture inbox, selects its first message, and renders the corresponding reader.
- Choosing another folder updates counts/list and safely updates or clears selection.
- Search is case-insensitive across sender, subject, and preview; clearing restores the folder.
- Empty folder and zero-result search display distinct helpful states.
- Next/previous selection never moves outside the filtered list.
- Archive removes the selected fixture from the inbox view and selects the next sensible row.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release` pass.

## Open Questions
- Final product name and application ID.
- Launch provider/authentication matrix for milestone two.
- Exact numeric memory/startup budgets once the shell can be measured.

## Scope

**Tier:** single-layer

**Reasoning:** This is one new native desktop UI application in one repository. It introduces several screens/states and tests but no backend, protocol service, or sibling repository.

**Subagents to spawn:**
- Context scan
- Rust/GTK implementation specialist adapted from the pipeline's single-layer architect role
- Test runner
- Critical, informational, testing, and design review passes

**Skills to invoke:**
- AutoFeature orchestration
- Image-generation output as the approved design reference

## Effort Plan

**Profile:** balanced, speed-biased MVP
**Rules fired:** none

| Task | Effort | Why |
|------|--------|-----|
| Context scan | medium | base |
| Technical plan | medium | base |
| Rust/GTK design | medium | base |
| Rust/GTK implementation | medium | base |
| Test runner | medium | base |
| Critical review | medium | base; no high-risk data/auth surface in this milestone |
| Informational review | medium | base |
| Testing review | medium | base |
| Design review | medium | base |

**Escalations during run:** none

## Product Review

Skipped at the user's direction to think less and build the MVP faster. The preceding focused feature review established the core product stance: visual quality wins attention, while protocol reliability belongs to a later milestone rather than being rushed into the shell.

## Implementation Plan

### Step 0 Scope Challenge

- **Existing-code check:** the repository contains only this brief and `docs/design/approved-mockup.png`; there is no application code to extend. The mockup is the visual contract and must remain unchanged.
- **Minimum viable change:** build one fixture-backed process with a single application window, pure in-memory models/state, Rust-built GTK widgets, embedded CSS, and the packaging/documentation artifacts already promised in scope. No account abstraction, repository trait, async runtime, persistence layer, or protocol-shaped interface is justified yet.
- **Complexity check:** the plan names more than eight files, but only five are Rust source/test modules. The remainder are the required stylesheet, Cargo lock/configuration, README, desktop metadata, AppStream metadata, and Flatpak manifest; combining those standard artifacts would not reduce runtime complexity. Keep production Rust to `main`, `model`, `state`, and `ui`, with no new service/classes.
- **GTK pattern check:** use installed GTK 4.22/libadwaita 1.9 facilities: `adw::Application`, `adw::ApplicationWindow`, an outer `adw::OverlaySplitView` for folders, an inner `adw::NavigationSplitView` for message list/reader, `gtk::ListBox` for the small fixed fixture set, `gio::SimpleAction` plus application accelerators for commands, `adw::ToastOverlay` for feedback, and CSS classes/tokens for styling. Avoid Blueprint, composite templates, custom widgets/subclassing, manual allocation, and a hand-rolled breakpoint engine. Do not depend on compositor transparency; reproduce the mockup with opaque/translucent-looking dark surfaces, borders, spacing, and cyan accents inside the window.
- **Footguns to avoid:** never retain an active `RefCell` borrow while updating widgets or invoking another callback; use weak widget references in long-lived closures; keep each `SimpleAction` installed once; do not use row indices as message identity; do not mutate fixture collections while iterating GTK children; and let libadwaita own narrow navigation history so the reader always has a back route.
- **Unit-testability check:** all filtering, counts, selection repair, folder/message navigation, archive behavior, and command transitions live in pure `AppState` methods. `ui.rs` only translates widget events into `Action`s and renders a read-only state snapshot. There is no I/O-bearing service to mock in this milestone.
- **Deferred without harming the objective:** real compose/reply/forward/download behavior, mutable starred/read state unless needed for the demonstrated shell, user-controlled sort/filter chips beyond the approved slice, persistence, background work, runtime theme switching, and verified Flatpak assembly.

### Architecture Review

#### Rust module boundaries

| Module | Responsibility | Must not own |
|---|---|---|
| `src/main.rs` | Process entry point, provisional app ID constant, application construction, startup CSS installation, activation, and exit status | Mail rules, fixture contents, widget layout details |
| `src/model.rs` | Small immutable domain types (`Account`, `Folder`, `Message`, `Attachment`, stable ID newtypes), optional metadata, and deterministic fixture factory | GTK types, selection rules, callbacks, filesystem/network access |
| `src/state.rs` | `AppState`, `Action`, `ViewStatus`, filter/count projections, selection invariants, navigation boundaries, archive mutation, and narrow-route intent | GTK widgets, CSS, fixture literals, external I/O |
| `src/ui.rs` | Build the adaptive widget tree, own a lightweight `Ui` handle plus shared `Rc<RefCell<AppState>>`, dispatch commands, perform targeted render passes, announce feedback, and connect accessibility/tooltips | Domain decision logic, protocol placeholders, persistence |
| `src/state/tests.rs` | Black-box-style unit tests against the `state` module's public-to-crate behavior and fixture IDs | GTK/display-dependent assertions |

Use the provisional application ID `dev.whitford.Whitford` consistently in code and metadata until branding is decided. Keep `Ui` as a plain struct of necessary widget handles, not a GTK subclass. Domain IDs are stable values rather than list offsets. Optional display fields remain `Option<T>` and presentation supplies explicit fallbacks (initials placeholder, hidden preview/timestamp/attachment details), never `unwrap()`.

#### GTK state and data flow

```text
deterministic fixtures -> AppState::new -> normalize_selection
                                      |
GTK signal / SimpleAction -> Action -> AppState::dispatch (pure transition)
                                      |
                                      +-> visible_messages / folder_counts
                                      +-> selected_message / ViewStatus
                                      |
                                      v
                              Ui::render(snapshot)
                    folders | message rows | reader | empty/offline state
                                      |
                     NavigationSplitView controls narrow list <-> reader route
```

- `Rc<RefCell<AppState>>` is the single source of truth on GTK's main thread. Dispatch borrows mutably only for the state transition, drops the borrow, then renders from an immutable borrow.
- `Action` covers `SelectFolder`, `SetSearch`, `SelectMessage`, `SelectNext`, `SelectPrevious`, `ArchiveSelected`, `ShowReader`, `ShowMessageList`, and non-mutating affordance commands. Escape clears search first, otherwise navigates reader-to-list, otherwise closes an open sidebar; it must not quit unexpectedly.
- Search is trimmed and case-insensitive across sender, subject, and optional preview. The visible list is the intersection of selected folder and search query. Folder counts come from messages, not hard-coded badges.
- After folder/search/archive changes, `normalize_selection` retains the selected stable ID if still visible, otherwise selects the nearest sensible surviving row (same index, then previous), or clears selection. The reader renders the selected message or a deliberate empty state—never stale content.
- Wide mode shows all three panes. Libadwaita collapses the inner list/reader split at narrow widths and supplies navigation/back behavior; the outer folder pane becomes an overlay at the smaller width. Exact thresholds should be tuned against the approved 1584×982 mockup and at a practical narrow size, without adding custom resize listeners.
- The approved mockup governs hierarchy and emphasis: approximately 17% folder rail, 32% message list, remaining reader at wide size; cyan selected-row edge/accent; compact search and filter bar; reader action toolbar, metadata, attachment card, and reply affordances; truncation rather than layout expansion for long values.
- Startup and all callbacks run on the GTK main context. No threads, channels, futures, or async runtime are introduced for synchronous fixtures.

#### Public contracts and invariants

- `fixtures() -> FixtureSet` is deterministic and includes populated Inbox, at least one empty folder, unread/starred examples, missing optional fields, long content, and an attachment.
- `AppState::new(FixtureSet) -> AppState` accepts empty data safely.
- `AppState::dispatch(Action) -> Transition` reports whether list, reader, folders, route, or toast feedback changed, allowing targeted rendering without duplicating rules in `ui.rs`.
- `visible_messages(&self) -> Vec<&Message>`, `selected_message(&self) -> Option<&Message>`, and `folder_count(&self, FolderId) -> usize` never panic on missing IDs.
- Invariant: `selected_message_id` is either `None` or identifies a member of the current visible set after every mutating action.
- Archive is an in-memory folder move/removal from Inbox view. Repeating archive with no selection is a safe no-op with user feedback.

### Code Quality Review

- Keep domain terminology consistent: `FolderId`, `MessageId`, `selected_folder_id`, `selected_message_id`, `search_query`, `visible_messages`, `normalize_selection`; avoid generic `data`, `item`, or `handle_event` names where the mail concept is known.
- Centralize transitions in `AppState::dispatch`; mouse and keyboard commands must use the same actions. Centralize widget refresh in small `render_folders`, `render_messages`, `render_reader`, and `render_status` helpers.
- Keep CSS selectors feature-scoped (`.whitford-window`, `.folder-pane`, `.message-row`, `.reader-pane`) and define repeated colors, radii, and spacing at the top of the embedded stylesheet where GTK CSS permits. Do not reproduce styling inline in Rust.
- Use `Result` only for genuinely fallible startup boundaries and `Option` for absent domain data. Avoid `unwrap`, `expect`, broad error swallowing, and speculative error enums for pure no-op commands.
- Target fewer than 300 lines per Rust module. If `ui.rs` approaches that threshold, split only its render helpers into `src/ui/render.rs`; do not pre-create that module.
- Accessibility is part of widget construction: meaningful accessible labels for icon-only buttons, visible tooltips, logical tab order, keyboard focus indication, and text labels that are not encoded only by color.
- Run `cargo fmt --check` and Clippy with warnings denied. Do not suppress lints unless a comment explains why the GTK API requires it.

### Unit Test Plan

Use Rust's built-in test harness; no new testing dependency and no display server are required. The canonical test file is `src/state/tests.rs`, enabled by `#[cfg(test)] mod tests;` from `state.rs`. Model fixture-shape tests may stay beside the factory in `model.rs` only if they do not duplicate state behavior.

`AppState::new` / selection normalization — `src/state/tests.rs`

- ✓ happy: fixture Inbox with messages selects the first visible stable ID and exposes its reader.
- ✓ nil: selected ID `None` with a populated visible list normalizes to the first row; absent optional message fields remain safe.
- ✓ empty: empty fixture/folder leaves selection `None` and projects the empty-folder state.
- ✓ error: unknown folder/message IDs do not panic, leave a valid state, and produce a no-op/feedback transition.

`visible_messages` / search — `src/state/tests.rs`

- ✓ happy: mixed-case query matches sender, subject, and preview only within the selected folder.
- ✓ nil: a message with `preview: None` is searchable through its other fields without panic.
- ✓ empty: blank/whitespace search restores all folder messages; unmatched query projects no-search-results, distinct from empty-folder.
- ✓ error: malformed Unicode/case-fold input remains valid Rust text and produces a deterministic result without corrupting selection.

`folder_count` — `src/state/tests.rs`

- ✓ happy: fixture counts match messages and archive updates Inbox/Archive counts.
- ✓ nil: unknown folder ID returns zero rather than panicking.
- ✓ empty: a known empty folder reports zero.
- ✓ error: orphaned/unknown message folder IDs are ignored or rejected by fixture normalization according to one documented invariant, never counted twice.

`dispatch(SelectFolder | SelectMessage | SetSearch)` — `src/state/tests.rs`

- ✓ happy: folder/message changes update visible rows, selection, reader, and change flags together.
- ✓ nil: selecting `None`/a no-longer-visible message clears or repairs selection explicitly.
- ✓ empty: moving to an empty folder clears reader; zero-result search does the same with the correct status.
- ✓ error: an unknown ID is a safe no-op with feedback and preserves the last valid folder.

`dispatch(SelectNext | SelectPrevious)` — `src/state/tests.rs`

- ✓ happy: navigation advances by visible order after filtering.
- ✓ nil: navigation with no selection chooses the first/last sensible row.
- ✓ empty: navigation on an empty visible set is a no-op.
- ✓ error/boundary: previous at first and next at last do not wrap or move out of range.

`dispatch(ArchiveSelected)` — `src/state/tests.rs`

- ✓ happy: selected Inbox message moves to Archive, counts change, and the same-index next row is selected.
- ✓ nil: no selected message produces a no-op/feedback transition.
- ✓ empty: archiving the only visible row leaves no selection and the correct empty state.
- ✓ error: a selected ID missing from storage is repaired without panic or partial mutation.

`dispatch(ShowReader | ShowMessageList)` and Escape precedence — `src/state/tests.rs`

- ✓ happy: selecting/opening a message requests the reader route; back requests the list route.
- ✓ nil: reader request without a selected message remains on the list with feedback.
- ✓ empty: Escape with empty search and list route has no destructive effect.
- ✓ error: repeated route/back actions are idempotent and do not alter mail state.

Fixture/model projections — `src/model.rs` tests

- ✓ happy: deterministic fixtures contain the visual-contract cases (unread, starred, attachment, long text, multiple folders).
- ✓ nil: missing initials, timestamp, preview, and attachment metadata map to defined fallbacks.
- ✓ empty: an empty attachment list renders no attachment region.
- ✓ error: duplicate stable IDs are detected by a debug/test validation helper before state construction.

GTK composition is deliberately not unit-tested headlessly in this milestone. `cargo build`, Clippy, and a manual Wayland smoke pass cover signal wiring and adaptive rendering; business behavior remains fully covered without initializing GTK.

### Error & Rescue Map

| Codepath | What can fail | Error type | Rescue action | User sees |
|---|---|---|---|---|
| `main` / `adw::Application::run` | no display, application registration, or GTK startup failure | GTK/GIO startup failure / non-zero exit | preserve GTK diagnostic, emit operation/app ID context where controllable, return non-zero; no retry loop | platform launch failure/diagnostic rather than a hung window |
| startup CSS installation | display unavailable or invalid CSS | missing `gdk::Display`; GTK CSS parse diagnostic | fail startup when there is no display; log CSS parse diagnostics during development and keep structural GTK defaults where GTK permits | usable unstyled/degraded window for CSS parse issues; no blank window |
| `fixtures` validation | duplicate IDs or unknown folder reference introduced by developer | validation error in debug/test path | log/assert before UI construction; fix fixture source, never silently overwrite | application does not show internally inconsistent mail |
| GTK callback state borrow | accidental nested mutable borrow | `RefCell` runtime panic | design dispatch to drop mutable borrow before rendering; cover transitions as pure tests | no expected user-facing path; treated as implementation defect |
| select folder/message by stale ID | row disappears after rerender/action | domain no-op/invalid target | repair selection and request targeted rerender; log at debug level only if unexpected | valid current list/reader or concise toast |
| archive with missing/no selection | stale or absent selected ID | domain no-op | normalize state; do not partially mutate; return feedback transition | “No message selected” toast when user initiated |
| compose/reply/forward/download affordance | behavior intentionally deferred; no backing I/O | unsupported MVP command | never pretend success; leave state unchanged and show one concise toast | “Available in a later milestone” |
| Flatpak build | builder/SDK absent locally | tooling/environment failure | document prerequisites and defer verification; native Cargo build remains release gate | README explains Flatpak is metadata-only for this milestone |

There are no network, database, filesystem-read, device, or background-job paths in runtime scope. No retry/backoff policy is needed. A silent callback catch or blank reader is a critical gap and must block completion.

### Shadow Path Testing

| Data flow | Happy | Nil | Empty | Error | Coverage / user result |
|---|---|---|---|---|---|
| fixtures → `AppState::new` → initial view | Inbox selects first mail | optional fields use fallbacks | no mail yields empty-folder | duplicate/invalid fixture detected | unit tests for all four; no panic/stale reader |
| folder + query → filter → list | matching rows shown | missing preview skipped safely | folder-empty vs no-results distinguished | unknown folder rejected/no-op | unit tests; explicit empty-state copy |
| row/action → selected ID → reader | selected content rendered | no selection renders prompt | zero rows renders state panel | stale ID normalized | unit tests; reader never retains prior mail |
| next/previous → visible index → selection | moves one row | no selection picks sensible edge | no rows no-op | boundary cannot overflow | unit tests; optional toast only when user action needs feedback |
| archive → mutate folder → counts/list/reader | move and select neighbor | no selection no-op | last row clears reader | stale ID causes no partial mutation | unit tests; concise toast on invalid user command |
| narrow select → navigation split → reader/back | reader opens and back returns | no selected reader is blocked | empty list stays list-side | repeated back/open idempotent | state tests plus manual Wayland smoke |
| shortcut → `SimpleAction` → same dispatch | command equals pointer behavior | unavailable selection no-op | empty list remains valid | unsupported command shows toast | state tests for action; manual accelerator smoke |

Every runtime data flow above has defined handling and a test. GTK presentation mechanics that require a display are explicitly assigned to the manual smoke checklist rather than mislabeled as unit coverage.

### Observability Checklist

- [ ] Startup diagnostics include the operation and provisional application ID; a fatal startup error returns non-zero.
- [ ] Unexpected invalid fixture references and stale IDs include the relevant folder/message ID, without logging message bodies, subjects, sender addresses, or search text.
- [ ] User-caused safe no-ops are represented through `Transition` feedback and a toast, not warning/error logs.
- [ ] No empty `match Err(_)`, silent catch equivalent, or ignored fallible result is introduced.
- [ ] GTK/libadwaita warnings remain visible on stderr during native development; use `G_MESSAGES_DEBUG` when diagnosing, not permanent noisy entry/exit logging.
- [ ] Pure successful state transitions are not logged. This local UI has no critical background paths, so per-action logging would add noise and expose mail-like fixture content.
- [ ] Deferred affordances give honest user feedback and never log a false success.
- [ ] README includes reproducible format, lint, test, debug-run, and release-build commands plus the manual Wayland smoke matrix.

### Performance Review

- The fixture list is intentionally small, so `gtk::ListBox` and O(n) filtering/count projection are simpler than a `gio::ListModel`/factory stack. Keep filtering and counts linear; do not nest a full-message scan inside each message row render.
- A state `Transition` marks dirty regions so search rerenders the message list/reader, archive rerenders counts/list/reader, and a route-only action does not rebuild every pane. Do not reconstruct the whole window per action.
- Search updates synchronously for the fixture set. Add debounce/background work only when real mailbox scale proves it necessary.
- Keep only fixture strings and small widget handles in memory. Attachment metadata is textual; do not load file bytes or images. Sender avatars are initials/CSS, avoiding image decode/cache work.
- Use weak captures for application/window/UI closures where ownership could cycle. GTK owns child widgets; application state must not own duplicate widget trees.
- Release acceptance records cold-start feel and idle memory as observations, not invented budgets. Numeric budgets remain deferred as stated in Open Questions.

### Files to create

- `Cargo.toml`: package metadata and only the gtk-rs GTK4/libadwaita dependencies needed by the installed native libraries.
- `Cargo.lock`: generated and committed reproducible dependency resolution for this application binary.
- `src/main.rs`: bootstrap, app ID, CSS install, activation, and exit handling.
- `src/model.rs`: domain types, stable IDs, optional metadata, fixtures, and fixture validation tests.
- `src/state.rs`: action reducer/state machine and pure list/count/selection/navigation projections.
- `src/state/tests.rs`: comprehensive happy/nil/empty/error unit tests for state behavior.
- `src/ui.rs`: adaptive GTK/libadwaita composition, signals/actions, rendering helpers, accessibility, and toasts.
- `src/style.css`: embedded dark visual tokens and component-scoped styles matching the approved mockup.
- `README.md`: prerequisites, commands, architecture summary, shortcut table, limitations, and manual smoke matrix.
- `data/dev.whitford.Whitford.desktop`: provisional desktop launcher metadata.
- `data/dev.whitford.Whitford.metainfo.xml`: provisional AppStream metadata.
- `build-aux/dev.whitford.Whitford.yml`: early Flatpak manifest, documented as unverified locally.

No icon asset is invented in this milestone; metadata should use a safe generic mail icon or omit an app-specific icon until branding supplies one. Do not copy the mockup into runtime resources.

### Files to modify

- None beyond the files created above. `docs/design/approved-mockup.png` remains read-only as the visual contract.

### Implementation order

1. Initialize Git on `feature/wayland-mail-shell`; add `Cargo.toml`, generate `Cargo.lock`, and establish the minimal libadwaita application bootstrap.
2. Define domain models, stable IDs, deterministic fixtures, and fixture validation in `model.rs`.
3. Implement the pure `AppState`/`Action` transition layer and its full state unit suite before connecting GTK callbacks.
4. Build the adaptive outer folder and inner list/reader splits in `ui.rs`; install centralized actions/accelerators and accessibility metadata.
5. Add targeted render helpers for folders/counts, filtered message rows, reader/attachment, loading/empty/no-results/offline/selected states, archive repair, and honest deferred-action toasts.
6. Add embedded `style.css` and tune wide/narrow visuals against `docs/design/approved-mockup.png`, preserving truncation, focus visibility, and compositor-independent contrast.
7. Add README, desktop/AppStream metadata, and the unverified Flatpak manifest using the same provisional app ID.
8. Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release`; then manually smoke-test on Wayland at wide and narrow sizes, pointer and every documented shortcut, all empty/offline/loading states, long/missing metadata, focus traversal, and back navigation.

### Test files and release gates

- `src/state/tests.rs`: state construction, filtering, selection repair, folder counts, navigation bounds, archive behavior, route/Escape rules, and invalid-target handling.
- Inline `#[cfg(test)]` tests in `src/model.rs`: fixture determinism/validation and optional-field cases only.
- Automated gates: `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `cargo test`; `cargo build --release`.
- Manual Wayland gate: visual comparison to the approved mockup at wide size; folder overlay and list/reader back navigation at narrow size; all shortcuts; pointer selection; long-text ellipsis; keyboard focus visibility; tooltips/accessibility labels; empty folder, no results, offline, loading, selected message; and deferred-action toast feedback.

### NOT in scope / deferred items

- IMAP/SMTP/OAuth/Secret Service, account onboarding, sync scheduling, retries, and background execution: milestone two protocol work.
- MIME/HTML/WebKit, sanitization, remote images, and real attachments/downloads: require separate security and rendering design.
- Database/cache, durable archive/read/star/draft state, and real compose/reply/forward: fixtures only in this shell.
- Multi-account, server search, notifications, settings, plugins, light theme, final brand/app ID/icon, and custom theming controls: do not expand the vertical slice.
- `gio::ListModel` virtualization, async search, pagination, and numeric startup/memory budgets: revisit with realistic mailbox scale and measurements.
- Verified Flatpak output: manifest only until `flatpak-builder` and the SDK runtime are installed.

### What already exists

- `docs/design/approved-mockup.png`: immutable visual contract for pane hierarchy, density, dark palette, cyan accent, mail-row anatomy, reader layout, attachment card, and action placement.
- `.autofeature/designs/wayland-mail-shell-2026-09-20.md`: approved scope and acceptance contract; this plan is appended to it.
- Installed local toolchain: Rust/Cargo 1.98.1, GTK4 4.22.5, libadwaita 1.9.4, rustfmt, Clippy, Wayland/Hyprland, and Flatpak without its builder/SDK.

### Failure modes summary

| Codepath | Test coverage | Error handling |
|---|---|---|
| Startup/display/application registration | release build + manual launch | non-zero exit and GTK/GIO diagnostic |
| Invalid fixtures/stable IDs | yes, unit | reject/assert with context before UI |
| Filtered selection becomes stale | yes, unit | deterministic normalize/clear and rerender |
| Empty folder vs no search results | yes, unit + manual | distinct deliberate state views |
| First/last navigation | yes, unit + manual | bounded no-op, never overflow/wrap |
| Archive selected/last/missing message | yes, unit + manual | atomic mutation or no-op, repair selection, toast |
| Narrow reader loses return path | state unit + manual adaptive smoke | libadwaita navigation split/back action |
| Deferred affordance invoked | state unit + manual | unchanged state and honest toast |
| CSS/visual regression | build diagnostic + manual comparison | structural fallback; fix before release |
| Flatpak tooling absent | documented only | explicit unverified status; native gates remain authoritative |

### User Challenges

None. The approved scope, toolchain, and visual contract support this plan without a product or architecture decision from the user. Final branding/app ID, protocol/auth matrix, and numeric performance budgets remain explicitly deferred questions rather than blockers.

## Rust/GTK Specialist Design

### Validated stack and dependencies

- The installed stack is Rust/Cargo 1.98.1, GTK 4.22.5, and libadwaita 1.9.4. Use edition 2024 and exactly two direct GUI dependencies: `gtk = { package = "gtk4", version = "0.11.4", features = ["v4_22"] }` and `adw = { package = "libadwaita", version = "0.9.2", features = ["v1_9", "gtk_v4_22"] }`. These are the current crates and expose the installed API levels; import their re-exported `gtk::gio` and `gtk::glib` rather than adding duplicate direct versions.
- Force the MVP's dark appearance with `adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark)`. Install one application-priority `gtk::CssProvider` from `include_str!("style.css")`; GTK CSS uses `@define-color`, not browser CSS custom properties, and does not provide backdrop blur. The window and panes must therefore remain opaque with contrast coming from surface colors, borders, and modest shadows.

### Adaptive composition corrections

- The proposed nesting is the correct libadwaita triple-pane pattern: an outer `adw::OverlaySplitView` (`sidebar = folder rail`, `content = inner split`) containing an `adw::NavigationSplitView` (`sidebar/content` must each be an `adw::NavigationPage`). Put an `adw::ToolbarView` and `adw::HeaderBar` in both navigation pages so the reader receives libadwaita's automatic back button and accessible page title when collapsed.
- Split views do **not** choose collapse thresholds themselves. Add two `adw::Breakpoint`s to `adw::ApplicationWindow` using `BreakpointCondition::new_length(MaxWidth, ..., LengthUnit::Sp)` and `Breakpoint::add_setter`: at `1000sp`, set outer `collapsed = true`; at `650sp`, also set inner `collapsed = true`. The setters automatically revert when a breakpoint un-applies. These are initial MVP thresholds to verify at 100% and large-text scaling, not resize-signal logic.
- Start with outer width fraction `0.18`, min/max `220sp/280sp`; inner sidebar fraction `0.37`, min/max `320sp/520sp`. This matches the mockup's approximate 17%/31%/52% geometry without hard-coding pane pixels. Use `LengthUnit::Sp` throughout.
- Outer collapse overlays the folder rail; it does not navigate it. Provide a sidebar-toggle button in the inner page header(s), shown by the `1000sp` breakpoint, and close the overlay after folder activation with `set_show_sidebar(false)`. Selecting a message calls `inner.set_show_content(true)` when collapsed; back/Escape calls `set_show_content(false)`. Do not build a parallel navigation stack.
- Escape precedence remains: clear non-empty search, then return reader to list, then close an open folder overlay, then no-op. Let the widgets own `show-content/show-sidebar`; a small pure `escape_outcome(context)` helper may be unit-tested without duplicating those GTK properties in `AppState`.

### State, UI, and file boundaries

- `model.rs` owns stable IDs, immutable display data, fixtures, and validation. `state.rs` owns folder/search projections, selected ID, archive mutation, and bounded message/folder navigation. It has no GTK types. After dispatch, create an owned `ViewSnapshot`, drop every `RefCell` borrow, and only then render; never render from inside an active state borrow because GTK signal re-entry can panic.
- Pre-split the UI rather than waiting for a monolithic `ui.rs`: `ui/mod.rs` holds `Ui` and orchestration, `ui/build.rs` constructs panes/widgets and breakpoints, `ui/actions.rs` installs commands/shortcuts once, and `ui/render.rs` performs folder/list/reader/status refreshes. Keep each production file near 300 lines; introduce no subclasses or service layer.
- `gtk::ListBox` is appropriate for the small fixture set. Rows carry a stable `MessageId` in the closure/data mapping, never a numeric index. Rendering may rebuild the small list, but selection normalization happens in state first and row callbacks all dispatch the same actions as keyboard commands.

### Actions, styling, and accessibility

- Install window-scoped `gio::SimpleAction`s once (`win.compose`, `win.focus-search`, `win.folder-next`, `win.folder-previous`, `win.message-next`, `win.message-previous`, `win.archive`, `win.reply`, `win.back`, and the honest deferred commands). Register application accelerators with `set_accels_for_action`; all pointer buttons use `action_name` so there is one command path.
- Use global accelerators only where they cannot corrupt text entry: `<Primary>n`, `<Primary>f`, `<Alt>Up/Down`, `<Primary>Up/Down`, `Delete`, `<Primary>r`, and `Escape`. If the mockup's bare `R/F` and mail-style `J/K` are retained, add them through a managed `gtk::ShortcutController` on the non-editable mail panes, not as application-wide accelerators; they must not fire while search has focus.
- Use symbolic icon names with text where the mockup has text. Every icon-only button gets both a tooltip and `gtk::accessible::Property::Label`; page titles are meaningful because libadwaita uses them for the back button and screen reader. Preserve visible focus rings, minimum 40sp pointer targets, ellipsized single-line list metadata, wrapped reader body text, and cyan-plus-shape selection indication rather than color alone.
- Scope CSS classes to Whitford and use `@define-color` tokens. Do not style GTK internals by fragile child-node position, hide focus outlines, or claim compositor transparency. `adw::ToastOverlay` wraps the complete adaptive content and reports deferred/no-op commands without logging fixture content.

### Verification

- Pure unit tests cover filtering (including Unicode/missing preview), counts, selection repair, folder/message bounds, archive neighbor choice, and Escape outcome. GTK construction remains compile-checked rather than initialized in unit tests.
- Run the four existing Cargo gates, then launch natively with the Wayland backend and verify at approximately `1584x982`, `900x760`, and `600x760`: three-pane geometry, folder overlay/toggle, reader back button, breakpoint reversibility, pointer and every shortcut, focus traversal, search typing without bare-key interception, long/missing fields, all explicit status views, and deferred-action toasts. Also inspect stderr for GTK CSS/property warnings and confirm `WAYLAND_DISPLAY` is in use rather than accepting an XWayland-only pass.

### User Challenge

None. Final naming and milestone-two protocol choices remain deferred and do not block this runnable shell.
