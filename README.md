# Whitford

Whitford is a lightweight native Gmail developer preview for Wayland. It authorizes one Gmail account in the system browser, stores the refresh token in Freedesktop Secret Service, and quickly synchronizes bounded Gmail folders and labels. A complete message is fetched only when you open it, then saved privately for offline reading. HTML mail is rendered with WebKitGTK in an ephemeral session with JavaScript, embedded navigation, downloads, and remote images disabled by default; remote images can be enabled explicitly for one message. Reply, Reply All, and Forward use a rich, non-modal composer with editable To/Cc/Bcc/subject fields, formatting, file attachments, private local draft recovery, and Gmail SMTP delivery. Compose, archive, Trash, labels, read/star changes, Gmail search, received-attachment saving, cached folder navigation, and reversible archive/Trash actions work from eligible folders and search results.

## Requirements

- Rust 1.98 or newer
- GTK 4.22 and libadwaita 1.9 development files
- WebKitGTK 6.0 development files
- A Wayland session
- A session-bus Secret Service provider such as GNOME Keyring, KWallet, or KeePassXC
- Network access to Google OAuth, userinfo, `imap.gmail.com:993`, and `smtp.gmail.com:465`

On Arch Linux:

```sh
sudo pacman -S --needed base-devel rust gtk4 libadwaita webkitgtk-6.0 pkgconf gnome-keyring
```

## Google developer-preview setup

Native development is the golden path. Set up a local OAuth client as follows:

1. Select or create the exact Google Cloud project **`whitford-email`**.
2. [Enable the Gmail API](https://console.cloud.google.com/apis/library/gmail.googleapis.com) in that project. Google's [Gmail quickstart](https://developers.google.com/workspace/gmail/api/quickstart/python) also documents the enable/consent/client sequence.
3. In Google Auth Platform, configure Branding and Audience. Choose **External → Testing** for a personal Gmail account, or **Internal** only when the project belongs to a Google Workspace organization and all users are inside that organization.
4. For External Testing, add the Gmail account you will connect under **Audience → Test users**. Google documents the audience and testing rules in [Manage App Audience](https://support.google.com/cloud/answer/15549945).
5. Under Data Access, add exactly `https://mail.google.com/`, `openid`, and `email`. Gmail IMAP and SMTP OAuth require the broad [`https://mail.google.com/` scope](https://developers.google.com/identity/protocols/oauth2/scopes), which Google describes as full mail access. Whitford reads with `EXAMINE`, UID search, and bounded `BODY.PEEK`; its only server mutation is sending an explicit reply.
6. Under Clients, create an OAuth 2.0 client with application type **Desktop app**. Download its JSON. Do not create or download a Web application client.
7. Install the downloaded file for a native run. Replace `/path/to/downloaded-client.json` with the real download path:

   ```sh
   config_root="${XDG_CONFIG_HOME:-$HOME/.config}"
   install -Dm600 /path/to/downloaded-client.json "$config_root/whitford/google-oauth.json"
   stat -c '%a %U %n' "$config_root/whitford/google-oauth.json"
   ```

   The check should show mode `600`, your user, and the resolved path. **Never copy `google-oauth.json` into this repository or any other source tree.** Whitford validates the project, Desktop-client shape, size, and Google endpoints without displaying credential values.
8. Start Whitford with `GDK_BACKEND=wayland cargo run`, choose **Connect Gmail**, select the configured test user, review the three scopes, and approve consent. The browser returns to a loopback address; Whitford then verifies the identity, stores only the refresh token in Secret Service, and loads the configured number of newest INBOX summaries.

External apps left in **Testing** issue authorizations—and offline refresh tokens for these non-identity scopes—that expire after seven days. This is expected developer-preview behavior: choose **Reconnect** and complete consent again. Internal audience behavior depends on the Workspace organization policy.

### Flatpak status and credential path

The Flatpak manifest is currently **unverified, non-runnable scaffolding**. It still needs vendored/offline Cargo sources and a locally available GNOME 50 SDK/runtime before it can be built or accepted. Native Wayland is the only golden path; do not report a Flatpak result from a native build.

After a real Flatpak bundle exists, install the same Desktop app JSON into its sandbox-specific configuration path:

```sh
flatpak_config="$HOME/.var/app/dev.whitford.Whitford/config"
install -Dm600 /path/to/downloaded-client.json "$flatpak_config/whitford/google-oauth.json"
stat -c '%a %U %n' "$flatpak_config/whitford/google-oauth.json"
```

The manifest requests network and Secret Service access and does not expose the host configuration directory broadly. Browser portal, loopback callback, config path, and Secret Service behavior remain manual Flatpak acceptance items.

## Run and verify

```sh
GDK_BACKEND=wayland cargo run
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

On first connection, Whitford binds a loopback callback, opens the system browser, validates PKCE and state, verifies the returned identity, and saves the refresh token through Secret Service. Access tokens stay in memory. Restart restores the secure authorization and refreshes INBOX without prompting.

Local **Disconnect** removes Whitford's saved authorization and deletes its local mail cache only after cleanup succeeds. It does not revoke Google-side access; use [Google Account third-party connections](https://myaccount.google.com/connections) for revocation.

Use **Keep summaries** in the folder sidebar to synchronize the newest 50, 100, 250, or 500 messages; the default is 100. Summary sync transfers bounded headers and MIME structure, not message bodies or attachment payloads. Opening a message downloads its complete RFC 5322 content without marking it read, including any MIME attachment payloads in that message, caches the readable body, and reuses it on later opens and restarts. Search covers sender and subject; attachment filtering comes from MIME structure without downloading attachment payloads during list sync.

The cache is stored under `${XDG_CACHE_HOME:-$HOME/.cache}/whitford/` with directory mode `700` and file mode `600`; preferences live under `${XDG_CONFIG_HOME:-$HOME/.config}/whitford/`. Opened bodies are stored separately with a 128 MiB least-recently-used budget and are never truncated to fit it. **Clear Cache** removes downloaded bodies while preserving summaries, authorization, and the retention preference. **Disconnect** removes authorization, summaries, and bodies. A transient sync error keeps cached summaries and downloaded bodies visible and marks them stale.

## Shortcuts

| Command | Shortcut |
|---|---|
| Focus loaded-message search | Ctrl+F |
| Previous/next folder | Alt+Up / Alt+Down |
| Previous/next message | Ctrl+Up / Ctrl+Down |
| Clear search / back / close folders | Escape |

Mail mutation shortcuts are intentionally absent.

## Manual acceptance

Use a non-production test mailbox and record each result without copying credentials or message content:

1. Move `google-oauth.json` aside, start the native app, and confirm onboarding names the missing file, resolved path, project/API/test-user requirements, broad scope, and reply-only sending limitation. Restore it with the `install -Dm600` command above.
2. Run `GDK_BACKEND=wayland cargo run`, connect, complete consent, and confirm the configured number of summaries appears without downloading every body.
3. Open a plain-text message and an HTML-only or multipart message. Confirm a loading state appears, the complete body renders, HTML typography and layout appear inside the reader, remote images start blocked, **Load images** affects only that message, external links open only after a click, and attachment/fallback copy remains honest.
4. Reopen the same message and restart Whitford; confirm its downloaded body is reused without another network fetch while summary sync continues independently.
5. Exercise **Reply**, **Reply All**, and **Forward**. Edit To/Cc/Bcc and subject, apply rich formatting, stage and remove multiple attachments, then send with the button and Ctrl+Enter. Confirm Escape, **Save & Close**, and window close save the draft on this device; only the trash action discards it. Verify replies remain in the Gmail thread and forwards start a new thread.
6. Choose **Refresh** and confirm the displayed last-successful-sync time changes. At narrow width (about 600 px), confirm status/recovery banners remain visible above both the list and reader.
7. Temporarily disconnect the network, choose **Refresh**, and confirm retained mail stays visible under an offline/stale banner with the real last sync and **Retry**. Restore the network and retry.
8. Revoke Whitford from [Google Account third-party connections](https://myaccount.google.com/connections), refresh, and confirm stale mail remains visible with **Reconnect** rather than a retry loop.
9. During a new authorization, exercise **Cancel** and **Reopen Browser**. Confirm an old or timed-out browser callback cannot change the current session.
10. With a suitable test mailbox, confirm empty INBOX and malformed messages show human-readable states and zero fallback/skipped counters are hidden.
11. Change **Keep summaries** among 50, 100, 250, and 500. Raising it should immediately refresh; lowering it should prune summaries and orphaned bodies. Restart and confirm the choice persists.
12. Note the displayed downloaded-body count and disk usage, choose **Clear Cache**, and confirm summaries remain while the open body returns to an unloaded state. Reopen it deliberately to download again.
13. Choose **Disconnect**, verify the confirmation copy, and confirm success clears summaries and bodies. A forced/unavailable cleanup must retain stale mail and offer **Retry cleanup**; do not claim this case passed unless it was actually reproduced.
14. Inspect logs and confirm they contain no client credential, OAuth URL/query/state/code/token, email identity, UID, sender, subject, body, attachment name, or search text.

## Logging

`WHITFORD_LOG` accepts `error`, `warn`, `info`, `debug`, or `trace`; unset, unknown, and `info` all select the default `info` level. For example:

```sh
WHITFORD_LOG=debug GDK_BACKEND=wayland cargo run
```

Logs are restricted to Whitford targets, but debug/trace output should still be treated as sensitive during development. Do not add raw credentials, OAuth callback data, account identity, IMAP identifiers, message metadata/content, attachment names, or search terms to tracing fields or pasted bug reports.
