//! Makes a message's HTML safe to show.
//!
//! Mail is hostile input. The interface shows it in a sandboxed frame that
//! runs no scripts and loads nothing from the network, and before it gets
//! there this module reduces it to formatting: no scripts, no forms, no
//! embedded objects, no style sheets, no event handlers, and only a fixed
//! list of harmless style properties. Either layer alone should be enough;
//! both are used.
//!
//! Remote images are not loaded. Their addresses are removed here, so that
//! even a mistake in the frame's policy cannot fetch them, and the caller is
//! told that there were some.

use ammonia::Builder;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct SafeHtml {
    pub html: String,
    /// The message wanted to load pictures or other content from the
    /// network, and was not allowed to.
    pub remote_content_blocked: bool,
}

/// Style properties that only affect how text and boxes look. Nothing here
/// can fetch a URL or cover the reading pane.
const STYLE: &[&str] = &[
    "color", "background-color", "font", "font-family", "font-size", "font-style", "font-weight",
    "font-variant", "line-height", "letter-spacing", "text-align", "text-decoration",
    "text-indent", "text-transform", "vertical-align", "white-space", "word-break",
    "word-wrap", "overflow-wrap", "margin", "margin-top", "margin-right", "margin-bottom",
    "margin-left", "padding", "padding-top", "padding-right", "padding-bottom", "padding-left",
    "border", "border-top", "border-right", "border-bottom", "border-left", "border-color",
    "border-style", "border-width", "border-collapse", "border-spacing", "border-radius",
    "width", "max-width", "min-width", "height", "max-height", "min-height", "display",
    "list-style", "list-style-type", "table-layout", "direction",
];

/// Replaces references to pictures carried inside the message (`cid:…`)
/// with the pictures themselves, so they display without any network
/// access. Call before [`safe_html`].
pub fn embed_inline_images(html: &str, images: &[(String, String, Vec<u8>)]) -> String {
    let mut out = html.to_string();
    for (content_id, mime_type, data) in images {
        let reference = format!("cid:{content_id}");
        if out.contains(&reference) {
            out = out.replace(&reference, &format!("data:{mime_type};base64,{}", base64(data)));
        }
    }
    out
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i > chunk.len() {
                out.push('=');
            } else {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
    }
    out
}

pub fn safe_html(html: &str) -> SafeHtml {
    let blocked = Arc::new(AtomicBool::new(false));
    let seen = blocked.clone();
    let mut builder = Builder::default();
    builder
        .add_generic_attributes(["style", "align", "valign", "bgcolor", "dir"])
        .add_tags(["font", "center"])
        .add_tag_attributes("font", ["color", "face", "size"])
        .add_tag_attributes("table", ["border", "cellpadding", "cellspacing", "width", "height"])
        .add_tag_attributes("td", ["width", "height", "colspan", "rowspan", "nowrap"])
        .add_tag_attributes("th", ["width", "height", "colspan", "rowspan", "nowrap"])
        .add_tag_attributes("tr", ["height"])
        // Allowed only so that the filter below sees it, removes it, and
        // can report that the message wanted remote pictures.
        .add_tag_attributes("img", ["srcset"])
        .filter_style_properties(STYLE.iter().copied().collect::<HashSet<_>>())
        .url_schemes(["http", "https", "mailto", "data"].into_iter().collect())
        .link_rel(Some("noopener noreferrer"))
        .attribute_filter(move |element, attribute, value| {
            let lower = value.trim_start().to_ascii_lowercase();
            match (element, attribute) {
                // Pictures may only come from inside the message itself.
                ("img", "src") | ("img", "srcset") => {
                    if lower.starts_with("data:image/") {
                        Some(value.into())
                    } else {
                        seen.store(true, Ordering::Relaxed);
                        None
                    }
                }
                // `data:` is for embedded pictures only, never for a link.
                (_, "href") if lower.starts_with("data:") => None,
                _ => Some(value.into()),
            }
        });
    let html = builder.clean(html).to_string();
    SafeHtml {
        html,
        remote_content_blocked: blocked.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(html: &str) -> String {
        safe_html(html).html
    }

    #[test]
    fn formatting_survives() {
        let out = clean(r##"<p style="color: red; font-size: 14px">Hello <b>there</b></p><table width="100%"><tr><td bgcolor="#eee" align="center">cell</td></tr></table>"##);
        assert!(out.contains("<b>there</b>"), "{out}");
        assert!(out.contains("color:red") || out.contains("color: red"), "{out}");
        assert!(out.contains("<table") && out.contains("bgcolor=\"#eee\"") && out.contains("align=\"center\""), "{out}");
    }

    #[test]
    fn anything_active_is_removed() {
        let hostile = [
            r#"<script>alert(1)</script>"#,
            r#"<img src="x" onerror="alert(1)">"#,
            r#"<a href="javascript:alert(1)">x</a>"#,
            r#"<a href="data:text/html,<script>alert(1)</script>">x</a>"#,
            r#"<iframe src="https://evil.example"></iframe>"#,
            r#"<form action="https://evil.example"><input name="password"></form>"#,
            r#"<object data="x"></object><embed src="x">"#,
            r#"<style>body { background: url(https://evil.example/t.gif) }</style>"#,
            r#"<link rel="stylesheet" href="https://evil.example/x.css">"#,
            r#"<meta http-equiv="refresh" content="0;url=https://evil.example">"#,
            r#"<base href="https://evil.example/">"#,
            r#"<svg onload="alert(1)"><script>alert(1)</script></svg>"#,
            r#"<div onclick="alert(1)" onmouseover="alert(1)">x</div>"#,
            r#"<p style="background-image: url(https://evil.example/t.gif); position: fixed; top: 0">x</p>"#,
            r#"<p style="behavior: url(x.htc); -moz-binding: url(x)">x</p>"#,
        ];
        for html in hostile {
            let out = clean(html).to_ascii_lowercase();
            for bad in [
                "<script", "javascript:", "onerror", "onclick", "onload", "onmouseover", "<iframe",
                "<form", "<input", "<object", "<embed", "<style", "<link", "<meta", "<base", "<svg",
                "evil.example", "url(", "position", "behavior", "binding", "data:text",
            ] {
                assert!(!out.contains(bad), "{bad:?} survived in {out:?} from {html:?}");
            }
        }
    }

    #[test]
    fn remote_pictures_are_dropped_and_reported() {
        let out = safe_html(r#"<p>hi</p><img src="https://tracker.example/pixel.gif" width="1" height="1">"#);
        assert!(out.remote_content_blocked);
        assert!(!out.html.contains("tracker.example"), "{}", out.html);

        let out = safe_html(r#"<img srcset="https://tracker.example/a.png 2x" alt="logo">"#);
        assert!(out.remote_content_blocked);
        assert!(!out.html.contains("tracker.example"));

        let embedded = r#"<img src="data:image/png;base64,iVBORw0KGgo=" alt="dot">"#;
        let out = safe_html(embedded);
        assert!(!out.remote_content_blocked);
        assert!(out.html.contains("data:image/png;base64,iVBORw0KGgo="), "{}", out.html);

        assert!(!safe_html("<p>just text</p>").remote_content_blocked);
    }

    #[test]
    fn embedded_pictures_are_shown_without_the_network() {
        let png = b"\x89PNG\r\n\x1a\n".to_vec();
        let html = r#"<p>Logo: <img src="cid:logo@x" alt="logo"> and <img src="cid:missing@x"></p>"#;
        let embedded = embed_inline_images(html, &[("logo@x".into(), "image/png".into(), png)]);
        let out = safe_html(&embedded);
        assert!(out.html.contains("data:image/png;base64,iVBORw0KGgo="), "{}", out.html);
        assert!(!out.html.contains("cid:"), "an unresolved reference is dropped: {}", out.html);
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // Only real pictures can be embedded this way.
        let sneaky = embed_inline_images(r#"<img src="cid:x">"#, &[("x".into(), "text/html".into(), b"<script>".to_vec())]);
        assert!(!safe_html(&sneaky).html.contains("data:"), "{}", safe_html(&sneaky).html);
    }

    #[test]
    fn links_keep_their_destination_and_gain_rel() {
        let out = clean(r#"<a href="https://example.com/page?x=1">click</a> <a href="mailto:a@example.com">mail</a>"#);
        assert!(out.contains(r#"href="https://example.com/page?x=1""#), "{out}");
        assert!(out.contains("noopener noreferrer"), "{out}");
        assert!(out.contains("mailto:a@example.com"));
    }
}
