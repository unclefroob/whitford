use crate::composer::MAX_HTML_BYTES;
use webkit6::prelude::*;

#[derive(Clone)]
pub(crate) struct ComposerEditor {
    pub view: webkit6::WebView,
    pub manager: webkit6::UserContentManager,
}

impl ComposerEditor {
    pub fn new() -> Self {
        let manager = webkit6::UserContentManager::new();
        manager.register_script_message_handler("changed", None);
        let session = webkit6::NetworkSession::new_ephemeral();
        session.connect_download_started(|_, download| download.cancel());
        let view = webkit6::WebView::builder()
            .network_session(&session)
            .user_content_manager(&manager)
            .build();
        view.set_can_focus(true);
        view.set_vexpand(true);
        view.set_height_request(260);
        view.update_property(&[gtk::accessible::Property::Label("Message body")]);
        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&view) {
            settings.set_enable_javascript(true);
            settings.set_auto_load_images(false);
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
        view.connect_permission_request(|_, request| {
            request.deny();
            true
        });
        view.connect_decide_policy(|_, decision, kind| {
            if matches!(
                kind,
                webkit6::PolicyDecisionType::NavigationAction
                    | webkit6::PolicyDecisionType::NewWindowAction
            ) {
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

                // `load_html` uses an internal about:blank navigation. Blocking it
                // prevents the contenteditable document from ever being created.
                if uri.as_deref() == Some("about:blank") && !user_gesture {
                    return false;
                }
                decision.ignore();
                true
            } else {
                false
            }
        });
        view.connect_load_changed(|view, event| {
            if event == webkit6::LoadEvent::Finished {
                view.evaluate_javascript(
                    "document.getElementById('editor')?.focus()",
                    None,
                    None,
                    None::<&gtk::gio::Cancellable>,
                    |_| {},
                );
            }
        });
        Self { view, manager }
    }

    pub fn load(&self, html: &str) {
        let initial = script_safe_json(html);
        let mut nonce_bytes = [0_u8; 18];
        if getrandom::fill(&mut nonce_bytes).is_err() {
            return;
        }
        let nonce = nonce_bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        self.view.load_html(&document(&initial, &nonce), None);
    }

    pub fn command(&self, command: &str, value: Option<&str>) {
        let command = serde_json::to_string(command).unwrap_or_else(|_| "\"\"".into());
        let value = serde_json::to_string(value.unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
        self.view.evaluate_javascript(
            &format!("window.whitfordCommand({command},{value})"),
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            |_| {},
        );
    }

    pub fn snapshot(&self, callback: impl FnOnce(String, String) + 'static) {
        self.view.evaluate_javascript(
            "JSON.stringify(window.whitfordSnapshot())",
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                let Ok(value) = result else { return };
                let raw = value.to_str();
                if let Ok((html, text)) = serde_json::from_str::<(String, String)>(&raw) {
                    callback(html, text);
                }
            },
        );
    }
}

fn document(initial_json: &str, nonce: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'; script-src 'nonce-{nonce}'; form-action 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'">
<style>:root{{color-scheme:light dark}}html,body{{min-height:100%;margin:0;background:transparent;color:inherit;font:15px system-ui}}#editor{{box-sizing:border-box;min-height:250px;padding:14px;outline:none;line-height:1.5}}blockquote{{border-left:3px solid #7b8490;margin-left:8px;padding-left:12px;color:#69727d}}a{{color:#3584e4}}img{{max-width:100%;height:auto}}</style></head>
<body><div id="editor" contenteditable="true" role="textbox" aria-label="Message body" aria-multiline="true"></div><script nonce="{nonce}">
const editor=document.getElementById('editor'); editor.innerHTML={initial_json}; let timer;
const initialQuote=editor.querySelector('blockquote');if(initialQuote)initialQuote.hidden=true;
const MAX_HTML={MAX_HTML_BYTES};
function snapshot(){{let html=editor.innerHTML;if(new TextEncoder().encode(html).length>MAX_HTML)return null;return [html,editor.innerText.slice(0,MAX_HTML)]}}
function publish(){{clearTimeout(timer);timer=setTimeout(()=>{{const value=snapshot();if(value)window.webkit.messageHandlers.changed.postMessage(JSON.stringify(value))}},500)}}
editor.addEventListener('input',publish);
editor.addEventListener('paste',event=>{{event.preventDefault();const text=event.clipboardData.getData('text/plain').slice(0,MAX_HTML);document.execCommand('insertText',false,text);publish()}});
editor.addEventListener('keydown',event=>{{if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='k'){{event.preventDefault();window.whitfordCommand('promptLink','')}}}});
window.whitfordSnapshot=()=>snapshot()||['',''];
window.whitfordCommand=(cmd,value)=>{{
 editor.focus();
 if(cmd==='promptLink'){{const url=window.prompt('Link address','https://');if(url)document.execCommand('createLink',false,url);publish();return}}
 if(cmd==='toggleQuote'){{const quote=editor.querySelector('blockquote');if(quote)quote.hidden=!quote.hidden;return}}
 if(cmd==='removeQuote'){{editor.querySelectorAll('blockquote').forEach(node=>node.remove());publish();return}}
 if(cmd==='insertCid'){{const img=document.createElement('img');img.src='cid:'+value;img.dataset.whitfordCid=value;editor.prepend(img);publish();return}}
 if(cmd==='removeCid'){{editor.querySelectorAll('img').forEach(img=>{{if(img.getAttribute('src')==='cid:'+value)img.remove()}});publish();return}}
 if(cmd==='promptSignature'){{const signature=window.prompt('Signature text','');if(signature)document.execCommand('insertText',false,'\n'+signature);publish();return}}
 document.execCommand(cmd,false,value||null);publish()
}};
</script></body></html>"#
    )
}

fn script_safe_json(value: &str) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "\"\"".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_html_cannot_escape_the_inline_script() {
        let encoded = script_safe_json("</script><script>alert(1)</script>");
        assert!(!encoded.contains('<'));
        assert!(!encoded.contains('>'));
        assert!(encoded.contains("\\u003c/script\\u003e"));
    }

    #[test]
    fn editor_script_requires_a_nonce_and_paste_is_plain_text() {
        let html = document("\"\"", "test-nonce");
        assert!(html.contains("script-src 'nonce-test-nonce'"));
        assert!(html.contains("<script nonce=\"test-nonce\">"));
        assert!(html.contains("clipboardData.getData('text/plain')"));
        assert!(!html.contains("script-src 'unsafe-inline'"));
    }
}
