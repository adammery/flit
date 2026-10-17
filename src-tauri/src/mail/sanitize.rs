//! SECURITY-CRITICAL (CLAUDE.md hard rule): every change here alters how
//! untrusted email HTML is rendered. The output of `build_srcdoc` is the ONLY
//! HTML form a message body may take on its way to the webview, and it is
//! rendered exclusively inside a fully sandboxed iframe (MessageView.svelte).
//! The one sanctioned exception is `sanitize_fragment`: the same sanitizer
//! pass, exposed bare for the reply-quote a compose window carries — it may
//! enter the compose editor only after an explicit user click (see CLAUDE.md).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;

use crate::mail::parse::InlineImage;

/// CSP embedded in every rendered message document. `default-src 'none'`
/// blocks ALL remote loads — tracking pixels, fonts, media, CSS url().
/// `img-src data:` is the one carve-out: data: URIs carry no network
/// request, so they can't track; they render the message's own cid images.
const BODY_CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'";

/// Base styling for the message document — our own trusted CSS, the reason
/// style-src 'unsafe-inline' is allowed above. Untrusted CSS is admitted only
/// through the STYLE_PROPERTIES allowlist: inline `style` attributes via
/// ammonia's filter, and `<style>` blocks via mail::css (parsed, filtered and
/// re-serialised, then injected right after this base).
// why html{overflow-y:hidden}: the conversation view auto-sizes the frame to
// this document, so it must never scroll vertically itself — a bar (even a
// few px of rounding) would steal wheel events from the conversation column.
// Horizontal stays scrollable for wide fixed-width mail content.
const BODY_STYLE: &str = "html{overflow-y:hidden}\
     body{font-family:system-ui,sans-serif;font-size:0.875rem;color:#1a1a1a;\
     margin:0.5rem;word-wrap:break-word}img{max-width:100%}\
     details.flit-quote{margin-top:0.75rem}\
     details.flit-quote>summary{list-style:none;display:inline-block;padding:1px 9px;\
     border-radius:9px;background:#e8e8ea;color:#5a5a60;font-size:0.7rem;\
     letter-spacing:0.1em;cursor:pointer;user-select:none}\
     details.flit-quote>summary::-webkit-details-marker{display:none}";

/// CSS properties permitted inside `style` attributes. ammonia's
/// `filter_style_properties` normalises every value and drops invalid
/// declarations and @rules, so only these property names — with a valid
/// value — reach the webview.
///
/// Most **URL-bearing** properties (`list-style-image`, `cursor`, `content`,
/// `border-image`, `mask`) are deliberately absent — the CSS network/tracking
/// vector. CSP (`img-src data:`) would block the load, but the sanitizer must
/// hold on its own, so they never survive here. `background-color` (colour
/// only) stands in for solid backgrounds.
///
/// `background` and `background-image` are the exception, and only for the
/// inline `style` ATTRIBUTE filter (`inline_style_property_set`, never this
/// set directly — see its doc comment): decided 2026-09-18, any `url(...)`
/// inside either gets the same resolve-or-leave-inert treatment as
/// `<img src>` instead of a blanket strip (senders commonly paint a banner
/// photo that way, and CSP backstops an unresolved reference exactly as it
/// does for `<img>`) — and a url-less `background:<color>` (most real-world
/// uses, e.g. a button background) now needs no resolution at all to
/// survive, the same as `background-color` always has.
///
/// why positioning IS allowed: senders hide preheader/preview text with
/// `position:absolute;left:-9999px` and the sr-only pattern, so stripping
/// `position` made that hidden text render in normal flow. It is safe because
/// the message renders in a fully sandboxed iframe — `position:fixed`/absolute
/// resolve against the iframe's own viewport (the message pane), so a message
/// still cannot paint over the app's real chrome. (ProtonMail keeps position
/// for the same reason.)
const STYLE_PROPERTIES: &[&str] = &[
    // Text & fonts
    "color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "font-variant",
    "font-stretch",
    "line-height",
    "letter-spacing",
    "word-spacing",
    "text-align",
    "text-align-last",
    "text-decoration",
    "text-decoration-color",
    "text-decoration-line",
    "text-decoration-style",
    "text-transform",
    "text-indent",
    "text-overflow",
    "text-shadow",
    "white-space",
    "vertical-align",
    "direction",
    "unicode-bidi",
    "word-break",
    "word-wrap",
    "overflow-wrap",
    "writing-mode",
    // Colour (no url)
    "background-color",
    "opacity",
    // Box model
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "border",
    "border-width",
    "border-style",
    "border-color",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-top-width",
    "border-top-style",
    "border-top-color",
    "border-right-width",
    "border-right-style",
    "border-right-color",
    "border-bottom-width",
    "border-bottom-style",
    "border-bottom-color",
    "border-left-width",
    "border-left-style",
    "border-left-color",
    "border-radius",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-bottom-left-radius",
    "border-bottom-right-radius",
    "border-collapse",
    "border-spacing",
    "box-sizing",
    "box-shadow",
    "outline",
    "outline-color",
    "outline-style",
    "outline-width",
    // Sizing
    "width",
    "height",
    "max-width",
    "max-height",
    "min-width",
    "min-height",
    // Flow & display
    "display",
    "visibility",
    "overflow",
    "overflow-x",
    "overflow-y",
    "float",
    "clear",
    // Positioning — safe inside the sandboxed iframe (see the type docs),
    // and needed so off-screen/sr-only hidden preheaders stay hidden.
    "position",
    "top",
    "right",
    "bottom",
    "left",
    "z-index",
    "clip",
    // Tables
    "table-layout",
    "caption-side",
    "empty-cells",
    // Lists (type/position only — list-style-image is url-bearing)
    "list-style-type",
    "list-style-position",
    // Flexbox
    "flex",
    "flex-direction",
    "flex-wrap",
    "flex-flow",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "justify-content",
    "align-items",
    "align-content",
    "align-self",
    "gap",
    "row-gap",
    "column-gap",
    "order",
];

/// The allowlist as a set — shared by the inline-`style` filter and the
/// `<style>`-block sanitizer (mail::css) so both honour the same policy.
fn style_property_set() -> HashSet<&'static str> {
    STYLE_PROPERTIES.iter().copied().collect()
}

/// `style_property_set()` plus `background`/`background-image` and the
/// latter's non-url layout companions — used ONLY for the inline `style`
/// ATTRIBUTE filter (ammonia's `.filter_style_properties()` call in
/// `sanitize()`), never for `<style>`-block content (mail::css keeps plain
/// `style_property_set()` there).
///
/// Why the split matters: this set only governs whether a PROPERTY NAME
/// survives with a syntactically valid value — it has no idea whether a
/// `url(...)` inside that value points at an already-resolved data: URI, a
/// still-remote https: URL, or isn't there at all (most `background:` mail
/// uses are a plain solid colour, e.g. a button's `background:#fa9e2a` —
/// that survives untouched and needs no resolution). For the inline
/// attribute that's fine, because `resolve_style_backgrounds` runs first (in
/// `attribute_filter`, before this allowlist is ever consulted) and has
/// ALREADY swapped every resolvable URL for its data: URI and left every
/// other one exactly as `img_src` leaves an unresolved `<img src>` — inert,
/// backstopped by CSP. A `<style>` block never goes through that rewrite
/// (ammonia drops `<style>` content outright; mail::css re-parses the
/// original text separately), so widening ITS allowlist the same way would
/// let a raw, never-examined `url(https://…)` straight through — that stays
/// blocked.
fn inline_style_property_set() -> HashSet<&'static str> {
    let mut set = style_property_set();
    set.extend([
        "background",
        "background-image",
        "background-repeat",
        "background-position",
        "background-position-x",
        "background-position-y",
        "background-size",
        "background-attachment",
        "background-origin",
        "background-clip",
    ]);
    set
}

/// Most bytes of `data:` image URIs one document may inline. Every
/// reference to an image is its own full copy, so without a ceiling one
/// inline image referenced thousands of times (a few KB of markup) grows
/// into gigabytes on its way to the webview. Real mail stays far below it:
/// servers cap a whole message at 25-50 MB.
#[cfg(not(test))]
const MAX_INLINED_BYTES: usize = 64 * 1024 * 1024;
// why smaller under test: the budget tests inline right up to the ceiling,
// and 64 MB of attribute text takes seconds through an unoptimised ammonia.
// Same logic, smaller number.
#[cfg(test)]
const MAX_INLINED_BYTES: usize = 64 * 1024;

/// What `build_srcdoc` hands back: the locked-down document plus how many
/// loadable remote images stayed blocked (drives the "Load images" banner).
#[derive(Debug)]
pub struct SanitizedBody {
    pub html: String,
    /// https img references NOT resolved by `remote` — each one would load
    /// if the user asked for it. http: refs are excluded: they are blocked
    /// too, but never loadable (hard rule: all network I/O over TLS).
    pub blocked_remote: usize,
}

/// Sanitize untrusted email HTML and wrap it in a locked-down standalone
/// document for iframe `srcdoc` rendering. `images` are the message's own
/// cid attachments; `remote` maps already-fetched https URLs to data: URIs
/// (empty = remote images stay blocked). Every image the webview shows is a
/// data: URI — the document itself never triggers a network request.
///
/// why: sanitization happens on every read, never at store time — the DB is
/// treated as untrusted and future sanitizer improvements apply retroactively
/// to already-cached mail.
/// `redundant_quote`: the caller verified this message's quoted history
/// repeats an earlier message of its conversation — cut it instead of
/// folding it (the content sits one card up).
/// `translations`: the reader asked for the message in another language —
/// its texts replaced by the local model's (see mail::translate).
pub fn build_srcdoc(
    untrusted_html: &str,
    images: &[InlineImage],
    remote: &HashMap<String, String>,
    redundant_quote: bool,
    translations: Option<&HashMap<String, String>>,
) -> SanitizedBody {
    let (clean, blocked_remote) = sanitize(untrusted_html, images, remote);
    // Post-sanitize: only the text between the clean fragment's tags
    // changes, and it goes back escaped — model output never becomes markup.
    let clean = match translations {
        Some(translations) => crate::mail::translate::apply(&clean, translations),
        None => clean,
    };
    // Post-sanitize, pre-embed: our own constant markup wrapped around the
    // already-clean fragment at element boundaries (see mail::quote).
    let clean = if redundant_quote {
        crate::mail::quote::strip_html_quote(clean)
    } else {
        crate::mail::quote::fold_html_quote(clean)
    };
    // Message <style> blocks: sanitized separately (mail::css parses and
    // re-serialises them under the same property allowlist) and injected as
    // our own trusted <style>, AFTER BODY_STYLE so the message overrides our
    // base. The css module returns text already safe to embed here.
    let message_css =
        crate::mail::css::sanitize_style_blocks(untrusted_html, &style_property_set());
    // `<base target="_blank">`: link clicks must leave the app entirely, and
    // no script (not even the parent's listeners) runs inside the sandboxed
    // frame on WebKit. Aiming every link at a new window turns the click into
    // a native new-window request, which the app denies and forwards to the
    // default browser — see the on_new_window handler in lib.rs.
    SanitizedBody {
        html: format!(
            "<!doctype html><html><head>\
             <meta charset=\"utf-8\">\
             <meta http-equiv=\"Content-Security-Policy\" content=\"{BODY_CSP}\">\
             <base target=\"_blank\">\
             <style>{BODY_STYLE}{message_css}</style>\
             </head><body>{clean}</body></html>"
        ),
        blocked_remote,
    }
}

/// Sanitize untrusted email HTML into a bare fragment: the same pass as
/// `build_srcdoc`, but without the srcdoc document wrapper, the viewer's
/// quote-fold markup, or `<style>`-block re-injection. Remote images are
/// never resolved here — their refs survive inert, exactly as in an
/// unloaded viewer render. This is the form a reply embeds as its quoted
/// original (blockquote content of the outgoing HTML part).
pub fn sanitize_fragment(untrusted_html: &str, images: &[InlineImage]) -> String {
    sanitize(untrusted_html, images, &HashMap::new()).0
}

fn sanitize(
    untrusted_html: &str,
    images: &[InlineImage],
    remote: &HashMap<String, String>,
) -> (String, usize) {
    // why: owned/cloned data moved into the closure — ammonia's
    // attribute_filter demands 'static, so it can't borrow the arguments.
    let data_uris: HashMap<String, String> = images
        .iter()
        .map(|image| (image.content_id.clone(), data_uri(image)))
        .collect();
    let remote = remote.clone();
    let blocked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&blocked);
    // why an atomic, not a plain counter: ammonia's attribute_filter must be
    // Send + Sync, so the closure can only mutate through one. Owned by the
    // closure (moved in) — nothing reads it afterwards.
    let inlined = AtomicUsize::new(0);
    let untrusted_html = rename_foreign_named_tags(untrusted_html);

    let html = ammonia::Builder::default()
        // Both schemes must be whitelisted or ammonia's scheme pass strips
        // them before attribute_filter ever sees the value. Neither leaks
        // through raw: the filter below resolves or removes every cid:, and
        // confines data: to img src.
        .add_url_schemes(&["cid", "data"])
        // mail-parser hands us the FULL document, <head> and all. ammonia
        // unwraps non-whitelisted tags (keeping their text), which would leak
        // the <title> as the body's first line — the actual cause of "PTUF6C |
        // …" / "$bifrost_…$" showing. clean_content_tags drops these tags AND
        // their text; script/style stay (this call replaces the default set).
        .clean_content_tags(
            ["script", "style", "title", "noscript"]
                .into_iter()
                .collect(),
        )
        // Inline styles carry newsletter layout. `style` is allowed on every
        // element, but filter_style_properties keeps only STYLE_PROPERTIES
        // (no url-bearing, no positioning) with a valid value — the rest,
        // and any @rule, is dropped.
        .add_generic_attributes(&["style"])
        .filter_style_properties(inline_style_property_set())
        // `class`/`id` are the hooks the injected <style> selectors match on
        // (mail::css). `hidden` is another way senders hide preheader text.
        // None can reference a URL or run script.
        .add_generic_attributes(&["class", "id", "hidden"])
        // Legacy presentational HTML that older mail (and many ESP templates)
        // still relies on. `<font>` plus per-tag layout attributes. Decided
        // 2026-09-18: `background` is now here too — the attribute twin of
        // CSS background-image, resolved by the SAME policy as `<img src>`
        // below (img_src), not a blanket carve-out.
        .add_tags(&["font", "center"])
        .add_tag_attributes("font", &["color", "face", "size"])
        .add_tag_attributes(
            "table",
            &[
                "bgcolor",
                "background",
                "width",
                "height",
                "cellpadding",
                "cellspacing",
                "border",
                "align",
                "valign",
            ],
        )
        .add_tag_attributes("tr", &["bgcolor", "background", "align", "valign"])
        .add_tag_attributes(
            "td",
            &[
                "bgcolor",
                "background",
                "width",
                "height",
                "align",
                "valign",
                "colspan",
                "rowspan",
                "nowrap",
            ],
        )
        .add_tag_attributes(
            "th",
            &[
                "bgcolor",
                "background",
                "width",
                "height",
                "align",
                "valign",
                "colspan",
                "rowspan",
                "nowrap",
            ],
        )
        .add_tag_attributes(
            "img",
            &["width", "height", "align", "border", "hspace", "vspace"],
        )
        .attribute_filter(move |element, attribute, value| {
            let is_img_src = element == "img" && attribute == "src";
            let is_background_attr =
                attribute == "background" && matches!(element, "table" | "tr" | "td" | "th");
            if is_img_src || is_background_attr {
                return img_src(value, &data_uris, &remote, &counter, &inlined);
            }
            if attribute == "style" {
                // Decided 2026-09-18: a CSS background/background-image is
                // the same network vector as <img src>, so any url(...)
                // inside either gets the same resolve-or-leave-inert
                // treatment (img_src) instead of a blanket strip. A
                // url-less background (a solid colour — most real mail
                // buttons) is left alone entirely; only the bytes inside
                // url(...) are ever touched, everything else in the
                // declaration list is untouched.
                return Some(resolve_style_backgrounds(
                    value, &data_uris, &remote, &counter, &inlined,
                ));
            }
            // Everywhere else (a href, blockquote cite, …) data: and cid:
            // are removed — a data: link in the sandbox is still a webview
            // navigation and has no legitimate use in mail.
            if scheme_is(value, "data") || scheme_is(value, "cid") {
                return None;
            }
            Some(value.into())
        })
        .clean(&untrusted_html)
        .to_string();

    let blocked = blocked.load(Ordering::Relaxed);
    (html, blocked)
}

/// Tag names ammonia treats as SVG/MathML elements when checking for
/// namespace switches (`is_svg_tag`/`is_mathml_tag` in ammonia 4.1.3). An
/// HTML element carrying one of these names fails that check and is removed
/// TOGETHER WITH ITS CONTENT — unlike any other unknown tag, which ammonia
/// merely unwraps. Templating engines do emit such names as custom tags in
/// plain HTML mail (DPD: `<text>`), so the sanitizer sees `<span>` instead,
/// which ammonia filters like every other element. Left out on purpose:
/// `a`, `font`, `span`, `title`, `style`, `script` (ammonia accepts them as
/// HTML), `svg` and `math` (real namespace roots, handled by ammonia) and
/// `image` (html5ever already parses it as `<img>`).
#[rustfmt::skip]
const FOREIGN_NAMED_TAGS: &[&str] = &[
    // SVG
    "animate", "animateMotion", "animateTransform", "circle", "clipPath", "defs", "desc",
    "discard", "ellipse", "feBlend", "feColorMatrix", "feComponentTransfer", "feComposite",
    "feConvolveMatrix", "feDiffuseLighting", "feDisplacementMap", "feDistantLight",
    "feDropShadow", "feFlood", "feFuncA", "feFuncB", "feFuncG", "feFuncR", "feGaussianBlur",
    "feImage", "feMerge", "feMergeNode", "feMorphology", "feOffset", "fePointLight",
    "feSpecularLighting", "feSpotLight", "feTile", "feTurbulence", "filter", "foreignObject",
    "g", "line", "linearGradient", "marker", "mask", "metadata", "mpath", "path", "pattern",
    "polygon", "polyline", "radialGradient", "rect", "set", "stop", "switch", "symbol", "text",
    "textPath", "tspan", "use", "view",
    // MathML
    "abs", "and", "annotation", "annotation-xml", "apply", "approx", "arccos", "arccosh",
    "arccot", "arccoth", "arccsc", "arccsch", "arcsec", "arcsech", "arcsin", "arcsinh",
    "arctan", "arctanh", "arg", "bind", "bvar", "card", "cartesianproduct", "cbytes", "ceiling",
    "cerror", "ci", "cn", "codomain", "complexes", "compose", "condition", "conjugate", "cos",
    "cosh", "cot", "coth", "cs", "csc", "csch", "csymbol", "curl", "declare", "degree",
    "determinant", "diff", "divergence", "divide", "domain", "domainofapplication", "emptyset",
    "eq", "equivalent", "eulergamma", "exists", "exp", "exponentiale", "factorial", "factorof",
    "false", "floor", "fn", "forall", "gcd", "geq", "grad", "gt", "ident", "imaginary",
    "imaginaryi", "implies", "in", "infinity", "int", "integers", "intersect", "interval",
    "inverse", "lambda", "laplacian", "lcm", "leq", "limit", "list", "ln", "log", "logbase",
    "lowlimit", "lt", "maction", "maligngroup", "malignmark", "matrix", "matrixrow", "max",
    "mean", "median", "menclose", "merror", "mfenced", "mfrac", "mglyph", "mi", "min", "minus",
    "mlabeledtr", "mlongdiv", "mmultiscripts", "mn", "mo", "mode", "moment", "momentabout",
    "mover", "mpadded", "mphantom", "mprescripts", "mroot", "mrow", "ms", "mscarries",
    "mscarry", "msgroup", "msline", "mspace", "msqrt", "msrow", "mstack", "mstyle", "msub",
    "msubsup", "msup", "mtable", "mtd", "mtext", "mtr", "munder", "munderover",
    "naturalnumbers", "neq", "none", "not", "notanumber", "notin", "notprsubset", "notsubset",
    "or", "otherwise", "outerproduct", "partialdiff", "pi", "piece", "piecewise", "plus",
    "power", "primes", "product", "prsubset", "quotient", "rationals", "real", "reals", "reln",
    "rem", "root", "scalarproduct", "sdev", "sec", "sech", "selector", "semantics", "sep",
    "setdiff", "share", "sin", "sinh", "subset", "sum", "tan", "tanh", "tendsto", "times",
    "transpose", "true", "union", "uplimit", "variance", "vector", "vectorproduct", "xor",
];

/// Rewrite start/end tags named in FOREIGN_NAMED_TAGS to `span` before the
/// HTML reaches ammonia (see the constant for why). Works on the tag token
/// alone, the way the tokenizer delimits it: `<`, optional `/`, a name that
/// runs to whitespace, `/` or `>`. Attributes, text and entities are left
/// as they are. Raw-text content (script, style, title, comments) is scanned
/// too, but ammonia drops all of it anyway.
fn rename_foreign_named_tags(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let mut start = i + 1;
        if bytes.get(start) == Some(&b'/') {
            start += 1;
        }
        if !bytes.get(start).is_some_and(u8::is_ascii_alphabetic) {
            i += 1;
            continue;
        }
        let mut end = start;
        while end < bytes.len()
            && !matches!(
                bytes[end],
                b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/' | b'>'
            )
        {
            end += 1;
        }
        // why: `start`/`end` sit right before ASCII bytes (or at EOF), so
        // they are char boundaries and slicing cannot panic.
        let name = &html[start..end];
        if FOREIGN_NAMED_TAGS
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(name))
        {
            out.push_str(&html[copied..start]);
            out.push_str("span");
            copied = end;
        }
        i = end;
    }
    out.push_str(&html[copied..]);
    out
}

/// The https image URLs a message references, in document order, deduped —
/// the fetch work-list for "load remote images". Collected through the same
/// ammonia pass as rendering, so only references that would actually appear
/// in the sanitized document are ever fetched. Covers `<img src>`, the
/// `background` attribute on a table cell, and `background-image` inside an
/// inline `style` attribute — the same three places `sanitize()` resolves.
pub fn remote_image_urls(untrusted_html: &str) -> Vec<String> {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let collector = Arc::clone(&seen);
    let record = move |value: &str| {
        if scheme_is(value, "https") {
            let url = effective_url(value);
            // why: no unwrap — a poisoned lock just yields fewer URLs.
            if let Ok(mut urls) = collector.lock() {
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
        }
    };
    let _ = ammonia::Builder::default()
        .add_url_schemes(&["cid", "data"])
        .add_generic_attributes(&["style"])
        .add_tag_attributes("table", &["background"])
        .add_tag_attributes("tr", &["background"])
        .add_tag_attributes("td", &["background"])
        .add_tag_attributes("th", &["background"])
        .attribute_filter(move |element, attribute, value| {
            let is_img_src = element == "img" && attribute == "src";
            let is_background_attr =
                attribute == "background" && matches!(element, "table" | "tr" | "td" | "th");
            if is_img_src || is_background_attr {
                record(value);
            } else if attribute == "style" {
                for (_, url) in background_url_ranges(value) {
                    record(url);
                }
            }
            Some(value.into())
        })
        .clean(&rename_foreign_named_tags(untrusted_html));
    seen.lock().map(|urls| urls.clone()).unwrap_or_default()
}

/// Policy for `<img src>`: cid: resolves to the matching inline image (or
/// nothing), data:image/* passes through (inert, can't track), fetched
/// https refs resolve to their data: URI, everything else survives for
/// layout but the CSP prevents it from ever loading. Resolving stops once
/// the document has inlined `MAX_INLINED_BYTES` (`inlined` keeps count).
fn img_src<'v>(
    value: &'v str,
    data_uris: &HashMap<String, String>,
    remote: &HashMap<String, String>,
    blocked: &AtomicUsize,
    inlined: &AtomicUsize,
) -> Option<Cow<'v, str>> {
    let effective = effective_url(value);
    if scheme_is(value, "cid") {
        let cid = &effective["cid:".len()..];
        return data_uris
            .get(cid)
            .filter(|uri| take_budget(uri, inlined))
            .map(|uri| uri.clone().into());
    }
    if scheme_is(value, "data") {
        // Only images may inline data — data:text/html and friends have no
        // business in mail markup.
        if starts_with_ignore_ascii_case(&effective, "data:image/") {
            return Some(value.into());
        }
        return None;
    }
    if scheme_is(value, "https") {
        match remote.get(&effective) {
            Some(uri) if take_budget(uri, inlined) => return Some(uri.clone().into()),
            // Fetched, but over budget: stays an inert URL. Not counted as
            // blocked — loading images again would not show it either.
            Some(_) => {}
            None => {
                blocked.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Some(value.into())
}

/// Reserve room for one more inlined copy of `uri`; false once it would
/// push the document past `MAX_INLINED_BYTES`. A smaller image later on
/// may still fit.
fn take_budget(uri: &str, inlined: &AtomicUsize) -> bool {
    let used = inlined.load(Ordering::Relaxed);
    if used + uri.len() > MAX_INLINED_BYTES {
        return false;
    }
    inlined.store(used + uri.len(), Ordering::Relaxed);
    true
}

/// Byte range (into `style`) of the URL argument — already unquoted and
/// trimmed — for every `background: url(...)` or `background-image:
/// url(...)` declaration in an inline `style` attribute value, in document
/// order. A plain textual scan, not a CSS parser: a `style` attribute is a
/// flat declaration list (no nested `{}`/@rules), so splitting on `;`
/// boundaries is well defined.
///
/// Matching "background" alone (the shorthand — e.g. a button's
/// `background:#fa9e2a`, no url at all) also naturally excludes every OTHER
/// `background-*` longhand (`background-color`, `background-position`, …):
/// each is found by the same `background` substring search, but then fails
/// the colon-boundary check below (`-color`/`-position`/… sits between the
/// name and the colon, not whitespace), so only bare `background` and
/// `background-image` ever reach the `url(` search.
///
/// The one thing this does NOT understand is a CSS-escaped property/token
/// (`\62 ackground-image`, `\75rl(`) — such a declaration is left alone here
/// and simply falls to the ordinary property-name allowlist afterward
/// (`background-image` is allowed, so ammonia's own value parser, which DOES
/// unescape, may still admit it unresolved — inert, same as an unresolved
/// `<img src>`, never as a fetched load).
///
/// Shared by `resolve_style_backgrounds` (which uses the ranges to splice in
/// a resolved value) and `remote_image_urls` (which only needs the URLs, to
/// build the fetch work-list) — one scanner, both agree on what counts as a
/// background reference.
fn background_url_ranges(style: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let lower = style.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("background") {
        let name_start = search_from + rel;
        search_from = name_start + "background".len();
        let name_len = if lower[name_start..].starts_with("background-image") {
            "background-image".len()
        } else {
            "background".len()
        };
        let name_end = name_start + name_len;
        // Property-name position: only whitespace, or nothing, before it
        // back to the start of the string or the previous `;`.
        let at_boundary =
            style[..name_start].trim().is_empty() || style[..name_start].trim_end().ends_with(';');
        if !at_boundary {
            continue;
        }
        // why look only at the next non-space character: searching for the
        // ':' through the rest of the string made every "background" cost
        // the length of the attribute — quadratic on a hostile style.
        let after_name = &lower[name_end..];
        let value = after_name.trim_start();
        if !value.starts_with(':') {
            continue; // not whitespace-only before ':' — a longer property name
        }
        let value_start = name_end + (after_name.len() - value.len()) + 1;
        let decl_end = lower[value_start..]
            .find(';')
            .map_or(style.len(), |i| value_start + i);
        // Nothing past here can start a property before `decl_end`, so
        // every exit below moves the search on to the next declaration.
        search_from = decl_end;
        let Some(url_rel) = lower[value_start..decl_end].find("url(") else {
            continue; // e.g. a solid `background:#fa9e2a` — nothing to resolve
        };
        let url_start = value_start + url_rel + "url(".len();
        let Some(close_rel) = style[url_start..decl_end].find(')') else {
            continue;
        };
        let url_end = url_start + close_rel;
        let raw = style[url_start..url_end].trim().trim_matches(['\'', '"']);
        out.push((url_start..url_end, raw));
    }
    out
}

/// Resolve every `background`/`background-image` `url(...)` reference
/// inside an inline `style` attribute value through the same policy as
/// `<img src>` (`img_src` — cid resolves, data:image/* passes, a fetched
/// https: URL becomes its data: URI, everything else survives inert for CSP
/// to block). Only the bytes inside `url(...)` are ever replaced; the rest
/// of the declaration list — including any `;` inside a substituted
/// `data:...;base64,...` URI, and any url-less `background:<color>` — is
/// left byte-for-byte untouched, which is why this splices by byte range
/// instead of splitting the string on `;` and rejoining.
fn resolve_style_backgrounds<'a>(
    style: &'a str,
    data_uris: &HashMap<String, String>,
    remote: &HashMap<String, String>,
    blocked: &AtomicUsize,
    inlined: &AtomicUsize,
) -> Cow<'a, str> {
    let ranges = background_url_ranges(style);
    if ranges.is_empty() {
        return Cow::Borrowed(style);
    }
    let mut out = String::with_capacity(style.len());
    let mut copied = 0;
    for (range, raw) in ranges {
        if let Some(resolved) = img_src(raw, data_uris, remote, blocked, inlined) {
            out.push_str(&style[copied..range.start]);
            out.push_str(&resolved);
            copied = range.end;
        }
    }
    out.push_str(&style[copied..]);
    Cow::Owned(out)
}

/// The URL as a browser's parser would see it: WHATWG strips leading and
/// trailing C0 controls/spaces and removes tabs and newlines anywhere.
///
/// why: a naive starts_with("data:") is bypassable with "\tdata:…" — this
/// check must agree with the browser, not with the raw bytes.
fn effective_url(value: &str) -> String {
    value
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

fn scheme_is(value: &str, scheme: &str) -> bool {
    starts_with_ignore_ascii_case(&effective_url(value), &format!("{scheme}:"))
}

/// why: bytes, not &str[..n] slicing — a multibyte char at the cut point
/// would panic, and this runs on attacker-controlled input.
fn starts_with_ignore_ascii_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn data_uri(image: &InlineImage) -> String {
    format!(
        "data:{};base64,{}",
        image.content_type,
        base64::engine::general_purpose::STANDARD.encode(&image.data)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_scan_stays_linear_on_hostile_styles() {
        // Each "background" used to search for its ':' through the rest of
        // the attribute. With no colon anywhere that is quadratic: this 2 MB
        // attribute took 5 s, and the time grows with the size squared.
        //
        // why a ratio and not a time limit: a fixed limit flaked on a slow CI
        // runner. Four times the input takes about four times as long when
        // the scan is linear and about sixteen times when it is quadratic,
        // on any machine; best of three runs keeps scheduling noise out.
        let best_time = |repeats: usize| {
            let style = "background;".repeat(repeats);
            (0..3)
                .map(|_| {
                    let started = std::time::Instant::now();
                    assert!(background_url_ranges(&style).is_empty());
                    started.elapsed()
                })
                .min()
                .expect("three runs")
        };

        let small = best_time(25_000);
        let large = best_time(100_000);

        assert!(
            large < small * 8,
            "4x the input took {large:?} vs {small:?} — not linear"
        );
    }

    fn png(content_id: &str) -> InlineImage {
        InlineImage {
            content_id: content_id.to_string(),
            content_type: "image/png".to_string(),
            data: b"\x89PNG\r\n\x1a\n".to_vec(),
        }
    }

    /// Most tests care only about the document, not the blocked count.
    fn srcdoc(untrusted_html: &str, images: &[InlineImage]) -> String {
        build_srcdoc(untrusted_html, images, &HashMap::new(), false, None).html
    }

    #[test]
    fn fragment_is_sanitized_but_bare() {
        let fragment = sanitize_fragment(
            "<p>hi</p><script>alert(1)</script>\
             <blockquote><p>&gt; old</p></blockquote>\
             <img src=\"cid:photo1\">",
            &[png("photo1")],
        );

        assert!(!fragment.to_lowercase().contains("<script"));
        // Bare: no srcdoc document shell, no viewer-only fold markup.
        assert!(!fragment.contains("<!doctype"));
        assert!(!fragment.contains("<details"));
        assert!(fragment.contains("<p>hi</p>"));
        // cid images resolve to inert data: URIs like in the viewer.
        assert!(fragment.contains("data:image/png"));
    }

    #[test]
    fn fragment_leaves_remote_images_unresolved() {
        let fragment = sanitize_fragment(r#"<img src="https://t.example/x.png">"#, &[]);

        // The ref survives for the recipient's own client to decide on —
        // we never fetch it, so quoting adds no network I/O.
        assert!(fragment.contains("https://t.example/x.png"));
    }

    #[test]
    fn strips_script_tags() {
        let doc = srcdoc("<p>hi</p><script>alert(1)</script>", &[]);

        assert!(!doc.to_lowercase().contains("<script"));
        assert!(doc.contains("<p>hi</p>"));
    }

    #[test]
    fn strips_event_handlers() {
        let doc = srcdoc(r#"<img src="cid:x" onerror="alert(1)">"#, &[]);

        assert!(!doc.contains("onerror"));
    }

    #[test]
    fn strips_javascript_urls() {
        let doc = srcdoc(r#"<a href="javascript:alert(1)">click</a>"#, &[]);

        assert!(!doc.contains("javascript:"));
    }

    #[test]
    fn keeps_img_src_for_structure() {
        // The tag survives so the layout is intact, but the embedded CSP
        // (default-src 'none') prevents it from ever loading.
        let doc = srcdoc(r#"<img src="https://example.com/pixel.png" alt="x">"#, &[]);

        assert!(doc.contains("<img"));
        assert!(doc.contains("pixel.png"));
    }

    #[test]
    fn points_every_link_at_a_new_window() {
        // why: WebKit runs no JS in a scripts-sandboxed frame — not even
        // listeners the parent attached (WebKit bug 218086) — so a link click
        // cannot be intercepted in the frame. `target=_blank` turns it into a
        // new-window request instead, which the Rust side denies and hands to
        // the default browser (see the on_new_window handler in lib.rs).
        let doc = srcdoc(r#"<a href="https://example.com/x">x</a>"#, &[]);

        assert!(doc.contains(r#"<base target="_blank">"#));
    }

    #[test]
    fn strips_sender_chosen_link_targets() {
        // The base target above only governs links that carry none of their
        // own — a sender-set target would opt back out of it (and _top would
        // aim at the app frame). ammonia's allowlist has no `target`, so none
        // survives; this test is the tripwire if that ever changes.
        let doc = srcdoc(r#"<a href="https://example.com" target="_top">x</a>"#, &[]);

        assert!(!doc.contains("_top"));
    }

    #[test]
    fn embeds_the_lockdown_csp() {
        let doc = srcdoc("<p>hi</p>", &[]);

        assert!(doc.contains(
            r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'">"#
        ));
        assert!(doc.starts_with("<!doctype html>"));
    }

    #[test]
    fn strips_iframes_and_forms() {
        let doc = srcdoc(
            r#"<iframe src="https://evil"></iframe><form action="https://evil"><input></form>"#,
            &[],
        );

        assert!(!doc.contains("<iframe"));
        assert!(!doc.contains("<form"));
    }

    #[test]
    fn counts_loadable_remote_images_left_blocked() {
        let body = build_srcdoc(
            r#"<img src="https://a.example/x.png"><img src="https://a.example/x.png">
               <img src="http://plain.example/y.png"><img src="data:image/gif;base64,AA==">"#,
            &[],
            &HashMap::new(),
            false,
            None,
        );

        // Both https refs count (each render blocks each occurrence); the
        // http: one is blocked too but never loadable (TLS-only rule), and
        // data: needs no loading.
        assert_eq!(body.blocked_remote, 2);
    }

    #[test]
    fn resolves_remote_references_from_the_fetched_map() {
        let mut remote = HashMap::new();
        remote.insert(
            "https://a.example/x.png".to_string(),
            "data:image/png;base64,iVBORw0KGgo=".to_string(),
        );

        let body = build_srcdoc(
            r#"<img src="https://a.example/x.png"><img src="https://a.example/missing.png">"#,
            &[],
            &remote,
            false,
            None,
        );

        assert!(body
            .html
            .contains(r#"src="data:image/png;base64,iVBORw0KGgo=""#));
        assert!(body.html.contains("missing.png"));
        assert_eq!(body.blocked_remote, 1);
    }

    #[test]
    fn collects_https_image_urls_deduped() {
        let urls = remote_image_urls(
            r#"<img src="https://a.example/x.png"><img src="https://a.example/x.png">
               <img src="&#9;https://tab.example/y.png"><img src="http://plain.example/z.png">
               <img src="cid:photo"><img src="data:image/gif;base64,AA==">
               <iframe><img src="https://stripped.example/never.png"></iframe>"#,
        );

        assert_eq!(
            urls,
            vec![
                "https://a.example/x.png".to_string(),
                "https://tab.example/y.png".to_string(),
            ]
        );
    }

    #[test]
    fn resolves_cid_references_to_data_uris() {
        let doc = srcdoc(r#"<p>pic:</p><img src="cid:photo1">"#, &[png("photo1")]);

        assert!(doc.contains(r#"src="data:image/png;base64,iVBORw0KGgo=""#));
        assert!(!doc.contains("cid:"));
    }

    #[test]
    fn unresolvable_cid_loses_its_src() {
        let doc = srcdoc(r#"<img src="cid:ghost" alt="x">"#, &[png("photo1")]);

        assert!(!doc.contains("cid:"));
        assert!(!doc.contains("src="));
        // The tag itself survives for layout.
        assert!(doc.contains("<img"));
    }

    #[test]
    fn stops_inlining_images_past_the_per_document_budget() {
        // Every reference is a full copy of the image. One inline image
        // named over and over turned a few KB of markup into gigabytes.
        let big = InlineImage {
            content_id: "big".to_string(),
            content_type: "image/png".to_string(),
            data: vec![0; MAX_INLINED_BYTES / 8],
        };
        let one_copy = data_uri(&big).len();
        let html = r#"<img src="cid:big">"#.repeat(40);

        let (doc, _) = sanitize(&html, &[big], &HashMap::new());

        let copies = doc.matches("data:image/png;base64,").count();
        assert_eq!(copies, MAX_INLINED_BYTES / one_copy);
        assert!(doc.len() <= MAX_INLINED_BYTES + 64 * 1024);
        // References past the budget lose their src, like an unknown cid.
        assert_eq!(doc.matches("<img>").count(), 40 - copies);
    }

    #[test]
    fn remote_images_past_the_budget_stay_inert() {
        let mut remote = HashMap::new();
        // Three copies fit the budget, a fourth would not.
        let a_third = "A".repeat(MAX_INLINED_BYTES / 3 - 64);
        let huge = format!("data:image/png;base64,{a_third}");
        remote.insert("https://a.example/x.png".to_string(), huge);
        let html = r#"<img src="https://a.example/x.png">"#.repeat(5);

        let (doc, blocked) = sanitize(&html, &[], &remote);

        assert_eq!(doc.matches("data:image/png;base64,").count(), 3);
        // The rest keep their https URL, which the CSP never loads. They
        // are not "blocked": loading images again would not show them.
        assert_eq!(doc.matches("src=\"https://a.example/x.png\"").count(), 2);
        assert_eq!(blocked, 0);
    }

    #[test]
    fn keeps_data_image_uris_on_img_only() {
        let doc = srcdoc(
            r#"<img src="data:image/gif;base64,R0lGOD"><a href="data:text/html,<script>x</script>">l</a>"#,
            &[],
        );

        assert!(doc.contains(r#"src="data:image/gif;base64,R0lGOD""#));
        assert!(!doc.contains("href=\"data:"));
    }

    #[test]
    fn strips_non_image_data_uris_from_img_src() {
        let doc = srcdoc(r#"<img src="data:text/html,<b>x</b>" alt="x">"#, &[]);

        assert!(!doc.contains("data:text"));
    }

    #[test]
    fn data_scheme_detection_survives_whatwg_url_cleanup() {
        // A tab inside the scheme fools naive prefix checks but not a
        // browser's URL parser — both sides must agree it's data:.
        let doc = srcdoc(
            "<a href=\"\td\nata:text/html,x\">l</a><img src=\"\tdata:image/png;base64,AA==\">",
            &[],
        );

        assert!(!doc.contains("href="));
        assert!(doc.contains("img"));
    }

    #[test]
    fn keeps_safe_inline_style_properties() {
        // The everyday newsletter vocabulary: colors, fonts, spacing, borders,
        // table sizing — the reason bodies looked broken without it. Wrapped
        // in a real table so ammonia's tree normalisation keeps the <td>.
        let doc = srcdoc(
            r#"<table><tr><td style="background-color:#f4f4f4;color:#333;padding:16px;font-family:Arial;font-size:14px;border:1px solid #ccc;width:600px">hi</td></tr></table>"#,
            &[],
        );

        assert!(doc.contains("background-color:#f4f4f4"));
        assert!(doc.contains("color:#333"));
        assert!(doc.contains("padding:16px"));
        assert!(doc.contains("font-family:Arial"));
        assert!(doc.contains("width:600px"));
    }

    #[test]
    fn drops_head_title_text_instead_of_leaking_it_into_the_body() {
        // mail-parser hands us the whole document, <head> included. ammonia
        // unwraps <title> by default and keeps its text — so the title
        // rendered as the first line of the body (the real Ryanair/HBO bug,
        // and most "preheader" sightings). Its content must be dropped whole.
        let doc = srcdoc(
            r#"<html><head><title>PTUF6C | Barcelona sale</title></head><body><p>Real content</p></body></html>"#,
            &[],
        );

        let body = doc.split_once("<body>").unwrap().1;
        assert!(!body.contains("PTUF6C"));
        assert!(!body.contains("Barcelona"));
        assert!(body.contains("Real content"));
    }

    #[test]
    fn keeps_positioning_so_offscreen_preheaders_stay_hidden() {
        // The common off-screen hide: without position/left this text would
        // render in normal flow. Safe because the sandboxed iframe confines
        // positioning to the message pane.
        let doc = srcdoc(
            r#"<div style="position:absolute;left:-9999px;top:-9999px">Preview text</div>"#,
            &[],
        );

        assert!(doc.contains("position:absolute"));
        assert!(doc.contains("left:-9999px"));
    }

    #[test]
    fn strips_url_bearing_style_properties_except_background() {
        // Every OTHER CSS property that can reach the network is still
        // dropped by the allowlist. background-color (no url) still
        // survives. Decided 2026-09-18: background and background-image are
        // the exception — url(...) inside either survives unresolved here
        // (no `remote` entry for it), the same way an unresolved <img src>
        // does: inert, backstopped by CSP, never fetched by the sanitizer
        // itself. See `resolves_background_image_style_property_when_remote_has_it`
        // and `resolves_background_shorthand_url_when_remote_has_it` for the
        // resolved cases, and `keeps_solid_color_background_shorthand` for
        // the far more common url-less case (a button's `background:#color`).
        // Uses sanitize_fragment (bare, no document wrapper) so the
        // assertions can't accidentally match our own trusted base CSS,
        // which also has a "cursor" declaration.
        let fragment = sanitize_fragment(
            r#"<div style="background-image:url(https://t.example/p.png);
               background:url(https://t.example/q.png);
               list-style-image:url(https://t.example/r.png);
               cursor:url(https://t.example/c.cur),auto;
               background-color:#fff">x</div>"#,
            &[],
        );

        assert!(fragment.contains("background-image:url(https://t.example/p.png)"));
        assert!(fragment.contains("background:url(https://t.example/q.png)"));
        assert!(!fragment.contains("list-style-image"));
        assert!(!fragment.contains("cursor"));
        assert!(fragment.contains("background-color:#fff"));
    }

    #[test]
    fn keeps_solid_color_background_shorthand() {
        // The actual bug this closes: bunny.net's "Manage Billing" button is
        // `background:#fa9e2a` (a plain colour, no url at all) with white
        // text — previously stripped outright along with every other
        // `background:` use, leaving white-on-white, unreadable text. A
        // url-less background needs no resolution and survives
        // unconditionally, the same as background-color always has.
        let fragment = sanitize_fragment(
            r#"<a style="background:#fa9e2a;color:#FFFFFF">Manage Billing</a>"#,
            &[],
        );

        assert!(fragment.contains("background:#fa9e2a"));
        assert!(fragment.contains("color:#FFFFFF") || fragment.contains("color:#ffffff"));
    }

    #[test]
    fn resolves_background_shorthand_url_when_remote_has_it() {
        let mut remote = HashMap::new();
        remote.insert(
            "https://t.example/bg.png".to_string(),
            "data:image/png;base64,iVBORw0KGgo=".to_string(),
        );
        let doc = build_srcdoc(
            r#"<table><tr><td style="background:url(https://t.example/bg.png) #fff">x</td></tr></table>"#,
            &[],
            &remote,
            false,
            None,
        )
        .html;

        assert!(doc.contains("background:url(data:image/png;base64,iVBORw0KGgo=) #fff"));
        assert!(!doc.contains("t.example"));
    }

    #[test]
    fn escaped_background_image_url_still_only_survives_inert() {
        // A CSS-escaped "url(" doesn't let the sanitizer's OWN scanner spot
        // and resolve the reference (background_url_ranges only
        // understands a literal "url(") — but ammonia's value parser
        // unescapes it when validating the now-allowed background-image
        // property, so the declaration still lands in exactly the same
        // inert, CSP-backstopped state as the plain-spelled version. No
        // `remote` entry exists for it either way, so nothing here could
        // have been swapped for a data: URI regardless of the escape.
        let fragment = sanitize_fragment(
            r#"<div style="background-image:\75rl(https://t.example/p.png)">x</div>"#,
            &[],
        );

        assert!(fragment.contains("background-image:url(https://t.example/p.png)"));
        assert!(!fragment.contains("data:"));
    }

    #[test]
    fn empties_the_style_attribute_when_nothing_survives() {
        // ammonia leaves an inert style="" rather than removing the attribute
        // — what matters is that the forbidden declaration is gone. Uses
        // list-style-image (still fully blocked) rather than background-image
        // (which is the one url-bearing property now allowed to survive
        // unresolved — see the test above).
        let doc = srcdoc(
            r#"<p style="list-style-image:url(https://t/x.png)">hi</p>"#,
            &[],
        );

        assert!(!doc.to_lowercase().contains("url("));
        assert!(!doc.contains("t/x.png"));
        assert!(doc.contains("hi"));
    }

    #[test]
    fn resolves_background_image_style_property_when_remote_has_it() {
        let mut remote = HashMap::new();
        remote.insert(
            "https://t.example/banner.png".to_string(),
            "data:image/png;base64,iVBORw0KGgo=".to_string(),
        );
        let doc = build_srcdoc(
            r#"<table><tr><td style="background-image:url(https://t.example/banner.png);
               background-repeat:no-repeat;background-size:cover;
               background-position:center top">x</td></tr></table>"#,
            &[],
            &remote,
            false,
            None,
        )
        .html;

        assert!(doc.contains("background-image:url(data:image/png;base64,iVBORw0KGgo=)"));
        assert!(!doc.contains("t.example"));
        // The non-url companions needed to actually position/size the image
        // survive alongside it, unconditionally (they carry no URL).
        assert!(doc.contains("background-repeat:no-repeat"));
        assert!(doc.contains("background-size:cover"));
        assert!(doc.contains("background-position:center top"));
    }

    #[test]
    fn resolving_one_background_image_does_not_disturb_a_neighbouring_declaration() {
        // The substituted data: URI contains a literal ';' of its own
        // ("data:image/png;base64,…") — resolve_style_backgrounds
        // splices by byte range, not by splitting the string on ';', so
        // that embedded ';' must not be mistaken for a declaration boundary
        // and swallow the next real declaration.
        let mut remote = HashMap::new();
        remote.insert(
            "https://t.example/banner.png".to_string(),
            "data:image/png;base64,iVBORw0KGgo=".to_string(),
        );
        let doc = build_srcdoc(
            r#"<table><tr><td style="background-image:url(https://t.example/banner.png);color:red">x</td></tr></table>"#,
            &[],
            &remote,
            false,
            None,
        )
        .html;

        assert!(doc.contains("background-image:url(data:image/png;base64,iVBORw0KGgo=)"));
        assert!(doc.contains("color:red"));
    }

    #[test]
    fn keeps_presentational_table_attributes() {
        // The legacy table-layout vocabulary of HTML email — none of it can
        // reference a URL, all of it is layout.
        let doc = srcdoc(
            r##"<table bgcolor="#eeeeee" width="600" cellpadding="10" cellspacing="0" border="0" align="center">
                <tr valign="top"><td width="50%" height="40" colspan="2" nowrap="nowrap">hi</td></tr></table>"##,
            &[],
        );

        assert!(doc.contains(r##"bgcolor="#eeeeee""##));
        assert!(doc.contains(r#"width="600""#));
        assert!(doc.contains(r#"cellpadding="10""#));
        assert!(doc.contains(r#"valign="top""#));
        assert!(doc.contains(r#"height="40""#));
        assert!(doc.contains(r#"colspan="2""#));
    }

    #[test]
    fn keeps_legacy_font_tag() {
        let doc = srcdoc(
            r##"<font color="#333333" face="Arial" size="4">hi</font>"##,
            &[],
        );

        assert!(doc.contains("<font"));
        assert!(doc.contains(r##"color="#333333""##));
        assert!(doc.contains(r#"face="Arial""#));
    }

    #[test]
    fn background_attribute_survives_unresolved_like_img_src() {
        // `background="url"` on a table/cell is the attribute-level twin of
        // <img src> and, decided 2026-09-18, resolved by the exact same
        // policy (img_src): unresolved here (no `remote` entry), so it
        // survives inert for layout, backstopped by CSP, never fetched by
        // the sanitizer itself.
        let doc = srcdoc(
            r#"<table background="https://t.example/bg.png"><tr><td>hi</td></tr></table>"#,
            &[],
        );

        assert!(doc.contains(r#"background="https://t.example/bg.png""#));
    }

    #[test]
    fn resolves_background_attribute_when_remote_has_it() {
        let mut remote = HashMap::new();
        remote.insert(
            "https://t.example/bg.png".to_string(),
            "data:image/png;base64,iVBORw0KGgo=".to_string(),
        );
        let doc = build_srcdoc(
            r#"<table background="https://t.example/bg.png"><tr><td>hi</td></tr></table>"#,
            &[],
            &remote,
            false,
            None,
        )
        .html;

        assert!(doc.contains(r#"background="data:image/png;base64,iVBORw0KGgo=""#));
        assert!(!doc.contains("t.example"));
    }

    #[test]
    fn a_translation_replaces_the_sanitized_text_and_stays_text() {
        let untrusted = r#"<p onclick="x()">Hallo <script>evil()</script>Welt</p>"#;
        let clean = sanitize_fragment(untrusted, &[]);
        assert_eq!(crate::mail::translate::segments(&clean), ["Hallo Welt"]);

        let translations = HashMap::from([(
            "Hallo Welt".to_string(),
            "<b onclick=x()>Ahoj</b> svet".to_string(),
        )]);
        let doc = build_srcdoc(untrusted, &[], &HashMap::new(), false, Some(&translations)).html;

        assert!(doc.contains("<p>&lt;b onclick=x()&gt;Ahoj&lt;/b&gt; svet</p>"));
        assert!(!doc.contains("Hallo"));
    }

    #[test]
    fn background_attribute_ignored_outside_table_cells() {
        // The attribute is only meaningful (and only resolved) on
        // table/tr/td/th — matching bgcolor's existing scope. Anywhere else
        // ammonia strips it before attribute_filter ever sees it, unchanged
        // from before this feature.
        let doc = srcdoc(
            r#"<body background="https://t.example/bg.png">hi</body>"#,
            &[],
        );

        assert!(!doc.contains("background="));
        assert!(!doc.contains("t.example"));
    }

    #[test]
    fn hides_a_class_based_preheader_via_injected_style() {
        // The reported bug: preview text hidden by a <style> class showed in
        // the body because we stripped <style>. Now the sanitized rule is
        // injected into the head and the element keeps its class to match.
        let doc = srcdoc(
            r#"<style>.preheader{display:none}</style><span class="preheader">Preview</span><p>Body</p>"#,
            &[],
        );

        let (head, body) = doc.split_once("<body>").unwrap();
        assert!(head.contains(".preheader"));
        assert!(head.contains("display: none"));
        assert!(body.contains(r#"class="preheader""#));
        // The original <style> must not leak into the body as text.
        assert!(!body.contains("<style"));
    }

    #[test]
    fn keeps_class_and_id_for_stylesheet_selectors() {
        let doc = srcdoc(r#"<div class="col" id="hero">hi</div>"#, &[]);

        assert!(doc.contains(r#"class="col""#));
        assert!(doc.contains(r#"id="hero""#));
    }

    #[test]
    fn sanitizes_injected_stylesheets_like_inline_styles() {
        let doc = srcdoc(
            r#"<style>@import url(https://t.example/x.css);
               .a{color:red;background-image:url(https://t.example/p.png);position:fixed}</style>
               <p class="a">x</p>"#,
            &[],
        );

        assert!(!doc.contains("@import"));
        assert!(!doc.contains("t.example"));
        assert!(!doc.to_lowercase().contains("position:fixed"));
        assert!(doc.contains("color: red"));
    }

    // DPD's template engine wraps every variable in <text>, and the PIN in
    // <hide>. ammonia unwraps unknown tags (keeping their text) — except an
    // HTML element whose NAME is an SVG/MathML element's ("text", "set",
    // "view", "list", …), which its namespace check removes with its content.
    // The name, PIN and deadline of a parcel notice vanished that way.
    #[test]
    fn keeps_text_inside_tags_named_like_svg_or_mathml_elements() {
        let doc = srcdoc(
            "<td>Dobrý deň <text>Dominik,</text><br>\
             <span style=\"color:#DC0032\"><text>PIN: <hide>360005</hide></text></span>\
             <TEXT>20.9.2026</TEXT> <set>a</set> <list>b</list></td>",
            &[],
        );

        assert!(doc.contains("Dobrý deň <span>Dominik,</span>"));
        assert!(doc.contains("PIN: 360005"));
        assert!(doc.contains("<span>20.9.2026</span>"));
        assert!(doc.contains("<span>a</span> <span>b</span>"));
        assert!(!doc.to_lowercase().contains("<text"));
    }

    #[test]
    fn remote_image_urls_sees_images_inside_such_tags() {
        let urls = remote_image_urls(r#"<text><img src="https://t.example/x.png"></text>"#);

        assert_eq!(urls, vec!["https://t.example/x.png".to_string()]);
    }

    #[test]
    fn remote_image_urls_collects_background_attribute_and_style() {
        let urls = remote_image_urls(
            r#"<table background="https://t.example/bg.png"><tr><td
               style="background-image:url(https://t.example/banner.png)">hi</td></tr></table>"#,
        );

        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://t.example/bg.png".to_string()));
        assert!(urls.contains(&"https://t.example/banner.png".to_string()));
    }

    #[test]
    fn real_svg_still_never_renders() {
        let doc = srcdoc(
            "<p>a</p><svg><text>x</text><script>alert(1)</script></svg>",
            &[],
        );

        assert!(!doc.to_lowercase().contains("<svg"));
        assert!(!doc.to_lowercase().contains("<script"));
        assert!(doc.contains("<p>a</p>"));
    }

    #[test]
    fn renames_only_whole_tag_names() {
        let html = "<textarea><text-block><text class=\"x\">a</text ><TEXT/>\
                    <image src=\"i\"> 1 < 2 &lt;text&gt;";

        assert_eq!(
            rename_foreign_named_tags(html),
            "<textarea><text-block><span class=\"x\">a</span ><span/>\
             <image src=\"i\"> 1 < 2 &lt;text&gt;"
        );
    }
}
