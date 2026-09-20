# Whitford

Whitford is a native, read-only Gmail developer preview for Wayland. It authorizes one Gmail account in the system browser, stores only the refresh token in Freedesktop Secret Service, and displays a bounded snapshot of the newest 50 INBOX messages. Sending, archive, delete, labels, read/star changes, additional folders, downloads, and offline disk storage are deliberately unavailable.

## Requirements

- Rust 1.98 or newer
- GTK 4.22 and libadwaita 1.9 development files
- A Wayland session
- A session-bus Secret Service provider such as GNOME Keyring, KWallet, or KeePassXC
- Network access to Google OAuth, userinfo, and `imap.gmail.com:993`

On Arch Linux:

```sh
sudo pacman -S --needed base-devel rust gtk4 libadwaita pkgconf gnome-keyring
```

## Google developer-preview setup

Native development is the golden path. Set up a local OAuth client as follows:

1. Select or create the exact Google Cloud project **`whitford-email`**.
2. [Enable the Gmail API](https://console.cloud.google.com/apis/library/gmail.googleapis.com) in that project. Google's [Gmail quickstart](https://developers.google.com/workspace/gmail/api/quickstart/python) also documents the enable/consent/client sequence.
3. In Google Auth Platform, configure Branding and Audience. Choose **External → Testing** for a personal Gmail account, or **Internal** only when the project belongs to a Google Workspace organization and all users are inside that organization.
4. For External Testing, add the Gmail account you will connect under **Audience → Test users**. Google documents the audience and testing rules in [Manage App Audience](https://support.google.com/cloud/answer/15549945).
5. Under Data Access, add exactly `https://mail.google.com/`, `openid`, and `email`. Gmail IMAP requires the broad [`https://mail.google.com/` scope](https://developers.google.com/identity/protocols/oauth2/scopes), which Google describes as full mail access. Whitford's implementation is nevertheless read-only: it uses `EXAMINE`, UID search, and bounded `BODY.PEEK`, and all mutation UI is disabled.
6. Under Clients, create an OAuth 2.0 client with application type **Desktop app**. Download its JSON. Do not create or download a Web application client.
7. Install the downloaded file for a native run. Replace `/path/to/downloaded-client.json` with the real download path:

   ```sh
   config_root="${XDG_CONFIG_HOME:-$HOME/.config}"
   install -Dm600 /path/to/downloaded-client.json "$config_root/whitford/google-oauth.json"
   stat -c '%a %U %n' "$config_root/whitford/google-oauth.json"
   ```

   The check should show mode `600`, your user, and the resolved path. **Never copy `google-oauth.json` into this repository or any other source tree.** Whitford validates the project, Desktop-client shape, size, and Google endpoints without displaying credential values.
8. Start Whitford with `GDK_BACKEND=wayland cargo run`, choose **Connect Gmail**, select the configured test user, review the three scopes, and approve consent. The browser returns to a loopback address; Whitford then verifies the identity, stores only the refresh token in Secret Service, and loads up to 50 newest INBOX messages.

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

Local **Disconnect** removes Whitford's saved authorization and clears session mail only after secure deletion succeeds. It does not revoke Google-side access; use [Google Account third-party connections](https://myaccount.google.com/connections) for revocation.

Search and filters apply only to the loaded newest-50 snapshot. A transient sync error keeps the prior in-process snapshot marked stale. No mail or token cache is written to disk.

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

1. Move `google-oauth.json` aside, start the native app, and confirm onboarding names the missing file, resolved path, project/API/test-user requirements, broad scope, and read-only limitation. Restore it with the `install -Dm600` command above.
2. Run `GDK_BACKEND=wayland cargo run`, connect, complete consent, and confirm the verified account plus newest-50 metadata appear.
3. Open a plain-text message and an HTML-only or multipart message. Confirm safe text, attachment metadata, and any fallback/truncation notice are honest.
4. Restart Whitford and confirm Secret Service restores the authorization without another browser prompt.
5. Choose **Refresh** and confirm the displayed last-successful-sync time changes. At narrow width (about 600 px), confirm status/recovery banners remain visible above both the list and reader.
6. Temporarily disconnect the network, choose **Refresh**, and confirm retained mail stays visible under an offline/stale banner with the real last sync and **Retry**. Restore the network and retry.
7. Revoke Whitford from [Google Account third-party connections](https://myaccount.google.com/connections), refresh, and confirm stale mail remains visible with **Reconnect** rather than a retry loop.
8. During a new authorization, exercise **Cancel** and **Reopen Browser**. Confirm an old or timed-out browser callback cannot change the current session.
9. With a suitable test mailbox, confirm empty INBOX and malformed/truncated messages show bounded, human-readable states and zero fallback/skipped counters are hidden.
10. Choose **Disconnect**, verify the confirmation copy, and confirm success clears mail. A forced/unavailable Secret Service cleanup must retain stale mail and offer **Retry cleanup**; do not claim this case passed unless it was actually reproduced.
11. Inspect logs and confirm they contain no client credential, OAuth URL/query/state/code/token, email identity, UID, sender, subject, body, attachment name, or search text.

## Logging

`WHITFORD_LOG` accepts `error`, `warn`, `info`, `debug`, or `trace`; unset, unknown, and `info` all select the default `info` level. For example:

```sh
WHITFORD_LOG=debug GDK_BACKEND=wayland cargo run
```

Logs are restricted to Whitford targets, but debug/trace output should still be treated as sensitive during development. Do not add raw credentials, OAuth callback data, account identity, IMAP identifiers, message metadata/content, attachment names, or search terms to tracing fields or pasted bug reports.
