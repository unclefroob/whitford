use adw::prelude::*;
use webkit6::prelude::*;

const CSP_BLOCKED: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; media-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";
const CSP_REMOTE_IMAGES: &str = "default-src 'none'; img-src https: http: data:; style-src 'unsafe-inline'; font-src data:; media-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

pub(super) fn message_body(source: &str) -> gtk::Box {
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    let network_session = webkit6::NetworkSession::new_ephemeral();
    network_session.connect_download_started(|_, download| download.cancel());
    let view = webkit6::WebView::builder()
        .network_session(&network_session)
        .build();
    view.set_vexpand(true);
    view.set_height_request(620);
    view.add_css_class("whitford-email-view");
    view.update_property(&[gtk::accessible::Property::Label("Email content")]);
    view.set_background_color(&gtk::gdk::RGBA::WHITE);

    if let Some(settings) = webkit6::prelude::WebViewExt::settings(&view) {
        harden(&settings);
    }
    deny_embedded_permissions_and_navigation(&view);

    let remote_notice = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["whitford-remote-content"])
        .build();
    remote_notice.append(
        &gtk::Label::builder()
            .label("Remote images are blocked for privacy")
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    let load_images = gtk::Button::builder()
        .label("Load images")
        .tooltip_text("Allow remote images for this message")
        .build();
    load_images.update_property(&[gtk::accessible::Property::Label(
        "Load remote images for this message",
    )]);
    load_images.connect_clicked({
        let view = view.clone();
        let source = source.to_owned();
        let remote_notice = remote_notice.clone();
        move |_| {
            if let Some(settings) = webkit6::prelude::WebViewExt::settings(&view) {
                settings.set_auto_load_images(true);
            }
            view.load_html(&html_document(&source, true), None);
            remote_notice.set_visible(false);
        }
    });
    remote_notice.append(&load_images);

    section.append(&remote_notice);
    section.append(&view);
    view.load_html(&html_document(source, false), None);
    section
}

fn harden(settings: &webkit6::Settings) {
    settings.set_auto_load_images(false);
    settings.set_enable_javascript(false);
    settings.set_enable_javascript_markup(false);
    settings.set_enable_html5_database(false);
    settings.set_enable_html5_local_storage(false);
    settings.set_enable_media(false);
    settings.set_enable_media_stream(false);
    settings.set_enable_mediasource(false);
    settings.set_enable_page_cache(false);
    settings.set_enable_webaudio(false);
    settings.set_enable_webgl(false);
    settings.set_enable_webrtc(false);
}

fn deny_embedded_permissions_and_navigation(view: &webkit6::WebView) {
    view.connect_permission_request(|_, request| {
        request.deny();
        true
    });
    view.connect_decide_policy(|_, decision, kind| match kind {
        webkit6::PolicyDecisionType::NavigationAction
        | webkit6::PolicyDecisionType::NewWindowAction => {
            let navigation = decision
                .clone()
                .downcast::<webkit6::NavigationPolicyDecision>()
                .ok()
                .and_then(|decision| decision.navigation_action());
            let uri = navigation
                .as_ref()
                .and_then(|navigation| navigation.request())
                .and_then(|request| request.uri());
            let user_gesture = navigation
                .as_ref()
                .is_some_and(|navigation| navigation.is_user_gesture());

            if uri.as_deref() == Some("about:blank") && !user_gesture {
                return false;
            }
            if user_gesture
                && let Some(uri) = uri.as_deref()
                && is_external_link(uri)
            {
                let _ = gtk::gio::AppInfo::launch_default_for_uri(
                    uri,
                    None::<&gtk::gio::AppLaunchContext>,
                );
            }
            decision.ignore();
            true
        }
        _ => false,
    });
}

fn is_external_link(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "mailto"))
}

fn html_document(source: &str, allow_remote_images: bool) -> String {
    let csp = if allow_remote_images {
        CSP_REMOTE_IMAGES
    } else {
        CSP_BLOCKED
    };
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{csp}">
<meta name="referrer" content="no-referrer">
<style>
  :root {{ color-scheme: light; }}
  html, body {{ min-height: 100%; margin: 0; padding: 0; background: #fff; color: #202124; }}
  body {{ padding: 22px; box-sizing: border-box; overflow-wrap: anywhere; font: 16px/1.5 system-ui, sans-serif; }}
  img {{ max-width: 100% !important; height: auto !important; }}
  table {{ max-width: 100% !important; }}
  pre {{ white-space: pre-wrap; }}
  a {{ color: #087ea4; }}
</style>
</head>
<body>{source}</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_document_preserves_markup_without_remote_permissions() {
        let document = html_document("<p>Hello <strong>world</strong></p>", false);
        assert!(document.contains("<strong>world</strong>"));
        assert!(document.contains("default-src 'none'"));
        assert!(document.contains("img-src data:"));
        assert!(!document.contains("img-src https:"));
        assert!(document.contains("referrer\" content=\"no-referrer"));
    }

    #[test]
    fn remote_image_document_only_expands_image_sources() {
        let document = html_document("<img src=\"https://example.test/pixel\">", true);
        assert!(document.contains("img-src https: http: data:"));
        assert!(document.contains("default-src 'none'"));
        assert!(document.contains("script-src") || document.contains("default-src 'none'"));
    }

    #[test]
    fn external_link_allowlist_rejects_active_and_local_schemes() {
        assert!(is_external_link("https://example.test/message"));
        assert!(is_external_link("mailto:user@example.test"));
        assert!(!is_external_link("javascript:alert(1)"));
        assert!(!is_external_link("file:///etc/passwd"));
        assert!(!is_external_link("data:text/html,bad"));
    }
}
