//! Translating a message body in place, the markup side: the body cut into
//! the pieces of text a reader takes as one (a paragraph, a table cell, a
//! list item, a button label), and their translations put back.
//!
//! SECURITY-CRITICAL (CLAUDE.md hard rule): the HTML functions here run on
//! the sanitizer's OUTPUT, post-sanitize and pre-embed — the same spot as
//! the quote fold in `build_srcdoc`. Tags and attributes pass through byte
//! for byte; only the text between them changes, and a translation goes
//! back escaped, as text. Model output can never become markup, whatever
//! it says.
//!
//! why a scanner and not a DOM: the input is html5ever's own serialization
//! (ammonia re-serializes everything it keeps), so it is regular — text is
//! escaped (`&amp;` `&lt;` `&gt;` `&nbsp;`), attribute values are always
//! double-quoted with `"` escaped, comments are gone. Walking it as tags and
//! text is exact here, and mail::quote already works on it the same way.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Elements that end one piece of text and start the next. Everything else
/// (a, b, span, font, img…) is inline: its text belongs to the piece around it.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "caption",
    "center",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

#[derive(Debug)]
enum Token<'a> {
    /// A tag (or anything else between `<` and `>`), kept verbatim. `name`
    /// is lowercase, empty for `<!…>`.
    Tag {
        raw: &'a str,
        name: String,
        closing: bool,
    },
    /// Escaped text between two tags.
    Text(&'a str),
}

impl Token<'_> {
    fn raw(&self) -> &str {
        match self {
            Token::Tag { raw, .. } | Token::Text(raw) => raw,
        }
    }
}

fn tokenize(html: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        match rest.find('<') {
            Some(0) => {
                // A tag that never closes is kept whole as a tag — passed
                // through untouched, never translated.
                let end = tag_end(rest).unwrap_or(rest.len());
                tokens.push(tag(&rest[..end]));
                rest = &rest[end..];
            }
            Some(at) => {
                tokens.push(Token::Text(&rest[..at]));
                rest = &rest[at..];
            }
            None => {
                tokens.push(Token::Text(rest));
                break;
            }
        }
    }
    tokens
}

/// Length of the tag at the start of `html` (which begins with `<`), its
/// `>` included. A `>` inside a double-quoted attribute value — the
/// serializer leaves it unescaped there — does not end the tag.
fn tag_end(html: &str) -> Option<usize> {
    if html.starts_with("<!--") {
        return html.find("-->").map(|at| at + 3);
    }
    let mut quoted = false;
    for (i, byte) in html.bytes().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b'>' if !quoted => return Some(i + 1),
            _ => {}
        }
    }
    None
}

fn tag(raw: &str) -> Token<'_> {
    let inner = &raw[1..];
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, inner),
    };
    let name = inner
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    Token::Tag { raw, name, closing }
}

/// One piece of text: the tokens between the block boundaries around it,
/// and which of them are its text.
struct Piece {
    span: Range<usize>,
    texts: Vec<usize>,
}

/// The pieces of text, in reading order. Quoted history (`<blockquote>`) is
/// left out: it repeats earlier mail and would double the work.
fn pieces(tokens: &[Token]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut texts = Vec::new();
    let mut quote_depth = 0usize;
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Tag { name, closing, .. } => {
                if BLOCKS.contains(&name.as_str()) {
                    if !texts.is_empty() {
                        pieces.push(Piece {
                            span: start..i,
                            texts: std::mem::take(&mut texts),
                        });
                    }
                    start = i + 1;
                }
                if name == "blockquote" {
                    quote_depth = if *closing {
                        quote_depth.saturating_sub(1)
                    } else {
                        quote_depth + 1
                    };
                }
            }
            Token::Text(_) if quote_depth == 0 => texts.push(i),
            Token::Text(_) => {}
        }
    }
    if !texts.is_empty() {
        pieces.push(Piece {
            span: start..tokens.len(),
            texts,
        });
    }
    pieces
}

/// What the reader sees of a piece: its text unescaped, whitespace runs
/// (non-breaking spaces too) collapsed to one space.
fn piece_text(tokens: &[Token], piece: &Piece) -> String {
    let joined: String = piece
        .texts
        .iter()
        .map(|&i| unescape(tokens[i].raw()))
        .collect();
    collapse(&joined)
}

/// Inline elements that hold no content of their own.
const VOIDS: &[&str] = &["img", "wbr", "area", "col", "input", "source", "track"];

/// What one marker in a model-facing text stands for.
#[derive(Debug, Clone, Copy)]
enum Mark {
    /// An inline element with content: its opening and closing tag.
    Pair(usize, usize),
    /// A void element (an image): its one tag.
    Void(usize),
}

/// A piece whose inline elements are shown to the model as markers —
/// `Lies ⟦1⟧unseren Blog⟦/1⟧ heute.` — so a translation can move a link or
/// a bold word along with the words it belongs to.
struct Marked {
    /// The tokens a valid translation replaces: first text to last, widened
    /// over the elements that straddle either end. An element wrapping the
    /// whole piece (a button's `<a><b>`) stays outside, unmarked.
    region: Range<usize>,
    /// Marker n is `marks[n - 1]`.
    marks: Vec<Mark>,
    /// The piece's text with the markers in place, as the model gets it.
    text: String,
}

/// The piece with its inline elements marked; `None` when they cannot be —
/// a tag in the middle whose partner lies outside the piece. Such a piece
/// is translated as plain text.
fn mark(tokens: &[Token], piece: &Piece) -> Option<Marked> {
    let blank = |i: &usize| unescape(tokens[*i].raw()).trim().is_empty();
    let first = *piece.texts.iter().find(|i| !blank(i))?;
    let last = *piece.texts.iter().rev().find(|i| !blank(i))?;

    // Pair the inline tags of the piece; a tag without its partner here
    // (the element continues past a <br>) is noted as unpaired.
    let mut pairs = Vec::new();
    let mut voids = Vec::new();
    let mut unpaired = Vec::new();
    let mut open: Vec<(usize, &str)> = Vec::new();
    for i in piece.span.clone() {
        let Token::Tag { name, closing, .. } = &tokens[i] else {
            continue;
        };
        if VOIDS.contains(&name.as_str()) {
            voids.push(i);
        } else if !*closing {
            open.push((i, name));
        } else if open.last().is_some_and(|(_, n)| n == name) {
            if let Some((o, _)) = open.pop() {
                pairs.push((o, i));
            }
        } else {
            unpaired.push(i);
        }
    }
    unpaired.extend(open.iter().map(|(i, _)| *i));

    // Widen [lo, hi] over every pair that straddles an end of it; a pair
    // around all of it stays outside.
    let (mut lo, mut hi) = (first, last);
    loop {
        let mut widened = false;
        for &(o, c) in &pairs {
            let wraps = o < lo && c > hi;
            let touches = (lo..=hi).contains(&o) || (lo..=hi).contains(&c);
            if touches && !wraps && (o < lo || c > hi) {
                lo = lo.min(o);
                hi = hi.max(c);
                widened = true;
            }
        }
        if !widened {
            break;
        }
    }
    if unpaired.iter().any(|i| (lo..=hi).contains(i)) {
        return None;
    }

    let mut marks = Vec::new();
    let mut ids: HashMap<usize, usize> = HashMap::new();
    let mut text = String::new();
    for (i, token) in tokens.iter().enumerate().take(hi + 1).skip(lo) {
        match token {
            Token::Text(raw) => text.push_str(&unescape(raw)),
            Token::Tag { closing: true, .. } => {
                if let Some(id) = ids.get(&i) {
                    text.push_str(&format!("⟦/{id}⟧"));
                }
            }
            Token::Tag { .. } => {
                if voids.contains(&i) {
                    marks.push(Mark::Void(i));
                    text.push_str(&format!("⟦{}/⟧", marks.len()));
                } else if let Some(&(o, c)) = pairs.iter().find(|(o, _)| *o == i) {
                    marks.push(Mark::Pair(o, c));
                    ids.insert(c, marks.len());
                    text.push_str(&format!("⟦{}⟧", marks.len()));
                }
            }
        }
    }
    Some(Marked {
        region: lo..hi + 1,
        marks,
        text: collapse(&text),
    })
}

/// The text the model gets for a piece, and its markers if it has any.
fn source(tokens: &[Token], piece: &Piece) -> (String, Option<Marked>) {
    match mark(tokens, piece) {
        Some(marked) if !marked.marks.is_empty() => (marked.text.clone(), Some(marked)),
        _ => (piece_text(tokens, piece), None),
    }
}

/// One part of a marked translation.
#[derive(Debug, PartialEq)]
enum Part<'a> {
    Text(&'a str),
    Open(usize),
    Close(usize),
    Void(usize),
}

/// A marked translation split into text and markers; `None` when a `⟦`
/// does not start a well-formed marker.
fn parts(translation: &str) -> Option<Vec<Part<'_>>> {
    let mut parts = Vec::new();
    let mut rest = translation;
    while let Some(at) = rest.find('⟦') {
        if at > 0 {
            parts.push(Part::Text(&rest[..at]));
        }
        let after = &rest[at + '⟦'.len_utf8()..];
        let end = after.find('⟧')?;
        let inner = after[..end].trim();
        parts.push(if let Some(id) = inner.strip_prefix('/') {
            Part::Close(id.trim().parse().ok()?)
        } else if let Some(id) = inner.strip_suffix('/') {
            Part::Void(id.trim().parse().ok()?)
        } else {
            Part::Open(inner.parse().ok()?)
        });
        rest = &after[end + '⟧'.len_utf8()..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest));
    }
    Some(parts)
}

/// The translated region with the piece's own tags put back where the
/// model's markers stand; `None` unless every marker came back exactly
/// once, of the right kind, properly nested.
fn rebuild(tokens: &[Token], marked: &Marked, translation: &str) -> Option<String> {
    let parts = parts(translation)?;
    let mut seen = vec![false; marked.marks.len()];
    let mut open: Vec<usize> = Vec::new();
    let mut out = String::new();
    for part in parts {
        match part {
            Part::Text(text) => out.push_str(&escape(text)),
            Part::Open(id) | Part::Void(id) if id == 0 || id > seen.len() || seen[id - 1] => {
                return None;
            }
            Part::Open(id) => {
                let Mark::Pair(o, _) = marked.marks[id - 1] else {
                    return None;
                };
                seen[id - 1] = true;
                open.push(id);
                out.push_str(tokens[o].raw());
            }
            Part::Void(id) => {
                let Mark::Void(v) = marked.marks[id - 1] else {
                    return None;
                };
                seen[id - 1] = true;
                out.push_str(tokens[v].raw());
            }
            Part::Close(id) => {
                if open.pop() != Some(id) {
                    return None;
                }
                let Mark::Pair(_, c) = marked.marks[id - 1] else {
                    return None;
                };
                out.push_str(tokens[c].raw());
            }
        }
    }
    (open.is_empty() && seen.iter().all(|s| *s)).then_some(out)
}

/// A translation with any markers dropped — for the plain fallback.
fn unmarked(translation: &str) -> String {
    match parts(translation) {
        Some(parts) => collapse(
            &parts
                .iter()
                .map(|part| match part {
                    Part::Text(text) => *text,
                    _ => " ",
                })
                .collect::<String>(),
        ),
        None => translation.to_string(),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Only text with letters is worth a model call — not "12:30", "|", "→".
fn worth_translating(text: &str) -> bool {
    text.chars().any(char::is_alphabetic)
}

fn unescape(raw: &str) -> String {
    // `&amp;` last, so "&amp;lt;" comes out as the literal text "&lt;".
    raw.replace("&nbsp;", "\u{a0}")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The distinct texts of a sanitized body worth translating, in reading
/// order — a newsletter's tenth "Read more" costs nothing extra.
pub fn segments(clean_html: &str) -> Vec<String> {
    let tokens = tokenize(clean_html);
    distinct(pieces(&tokens).iter().map(|piece| source(&tokens, piece).0))
}

/// The sanitized body with every piece found in `translations` (keyed by
/// the texts `segments` returned) replaced. A marked piece whose markers all
/// came back gets its own tags around the translated words; any other
/// piece gets its translation in its first text and the rest of its text
/// emptied, so a heading or a button keeps its formatting and its link,
/// while a paragraph whose markers the model broke reads as plain text.
pub fn apply(clean_html: &str, translations: &HashMap<String, String>) -> String {
    let tokens = tokenize(clean_html);
    let mut replaced: HashMap<usize, String> = HashMap::new();
    let mut dropped: HashSet<usize> = HashSet::new();
    for piece in pieces(&tokens) {
        let (key, marked) = source(&tokens, &piece);
        let Some(translation) = translations.get(&key) else {
            continue;
        };
        if let Some(marked) = &marked {
            if let Some(rebuilt) = rebuild(&tokens, marked, translation) {
                let (lo, hi) = (marked.region.start, marked.region.end - 1);
                let lead = lead_space(&tokens[lo]);
                let trail = trail_space(&tokens[hi]);
                replaced.insert(lo, format!("{lead}{rebuilt}{trail}"));
                dropped.extend(lo + 1..=hi);
                continue;
            }
        }
        let translation = unmarked(translation);
        let mut first = true;
        for &i in &piece.texts {
            let raw = tokens[i].raw();
            if unescape(raw).trim().is_empty() {
                continue;
            }
            let (lead, trail) = (lead_space(&tokens[i]), trail_space(&tokens[i]));
            let text = if first {
                escape(&translation)
            } else {
                String::new()
            };
            first = false;
            replaced.insert(i, format!("{lead}{text}{trail}"));
        }
    }
    tokens
        .iter()
        .enumerate()
        .filter(|(i, _)| !dropped.contains(i))
        .map(|(i, token)| replaced.get(&i).map_or(token.raw(), String::as_str))
        .collect()
}

/// Whitespace at the start of a text token; nothing for a tag.
fn lead_space<'a>(token: &Token<'a>) -> &'a str {
    match token {
        Token::Text(raw) => &raw[..raw.len() - raw.trim_start().len()],
        Token::Tag { .. } => "",
    }
}

/// Whitespace at the end of a text token; nothing for a tag.
fn trail_space<'a>(token: &Token<'a>) -> &'a str {
    match token {
        Token::Text(raw) => &raw[raw.trim_end().len()..],
        Token::Tag { .. } => "",
    }
}

/// The paragraphs of a plain-text body (lines up to a blank line, joined),
/// the pieces of a text mail.
pub fn text_segments(text: &str) -> Vec<String> {
    distinct(
        paragraphs(text)
            .iter()
            .map(|lines| collapse(&lines.join(" "))),
    )
}

/// A plain-text body with its translated paragraphs replaced; paragraphs
/// stay apart by one blank line.
pub fn apply_text(text: &str, translations: &HashMap<String, String>) -> String {
    paragraphs(text)
        .iter()
        .map(|lines| {
            translations
                .get(&collapse(&lines.join(" ")))
                .cloned()
                .unwrap_or_else(|| lines.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn paragraphs(text: &str) -> Vec<Vec<&str>> {
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    paragraphs
}

fn distinct(texts: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    texts
        .filter(|text| worth_translating(text) && seen.insert(text.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn blocks_split_pieces_and_inline_tags_do_not() {
        let html =
            "<h1>Hallo Welt</h1><p>Lies <a href=\"https://x.test/\">unseren Blog</a> heute.</p>\
                    <table><tbody><tr><td>Zelle eins</td><td>Zelle zwei</td></tr></tbody></table>";
        assert_eq!(
            segments(html),
            [
                "Hallo Welt",
                "Lies ⟦1⟧unseren Blog⟦/1⟧ heute.",
                "Zelle eins",
                "Zelle zwei"
            ]
        );
    }

    #[test]
    fn line_breaks_split_and_whitespace_collapses() {
        let html = "<div>\n  Erste   Zeile<br>Zweite&nbsp;Zeile\n</div>";
        assert_eq!(segments(html), ["Erste Zeile", "Zweite Zeile"]);
    }

    #[test]
    fn repeated_and_letterless_texts_are_skipped() {
        let html = "<p>Mehr lesen</p><p>12:30 | →</p><p>Mehr lesen</p><p>&nbsp;</p>";
        assert_eq!(segments(html), ["Mehr lesen"]);
    }

    #[test]
    fn quoted_history_is_left_out() {
        let html = "<p>Danke!</p><blockquote><p>Alte Nachricht</p><blockquote><p>Noch älter</p></blockquote><p>Auch alt</p></blockquote><p>Gruß</p>";
        assert_eq!(segments(html), ["Danke!", "Gruß"]);
    }

    #[test]
    fn text_is_unescaped_for_the_model() {
        assert_eq!(
            segments("<p>Tom &amp; Jerry &lt;3 &amp;lt;</p>"),
            ["Tom & Jerry <3 &lt;"]
        );
    }

    #[test]
    fn a_gt_inside_an_attribute_does_not_end_the_tag() {
        let html = "<a title=\"a > b\" href=\"https://x.test/?q=1>2\">Klick</a>";
        assert_eq!(segments(html), ["Klick"]);
        assert_eq!(
            apply(html, &map(&[("Klick", "Klikni")])),
            "<a title=\"a > b\" href=\"https://x.test/?q=1>2\">Klikni</a>"
        );
    }

    #[test]
    fn a_single_text_piece_keeps_its_markup() {
        let html =
            "<td style=\"color:red\"><a href=\"https://x.test/\"><b>Jetzt bestellen</b></a></td>";
        assert_eq!(
            apply(html, &map(&[("Jetzt bestellen", "Objednať teraz")])),
            "<td style=\"color:red\"><a href=\"https://x.test/\"><b>Objednať teraz</b></a></td>"
        );
    }

    #[test]
    fn a_piece_whose_markers_broke_reads_as_plain_text_in_its_first_text() {
        let html = "<p>Lies <a href=\"https://x.test/\">unseren Blog</a> heute.</p>";
        let key = "Lies ⟦1⟧unseren Blog⟦/1⟧ heute.";
        for broken in [
            "Čítaj náš blog dnes.",
            "Čítaj ⟦1⟧náš blog dnes.",
            "Čítaj ⟦1⟧náš⟦/1⟧ ⟦1⟧blog⟦/1⟧ dnes.",
            "Čítaj ⟦/1⟧náš blog⟦1⟧ dnes.",
            "Čítaj ⟦2⟧náš blog⟦/2⟧ dnes.",
        ] {
            assert_eq!(
                apply(html, &map(&[(key, broken)])),
                "<p>Čítaj náš blog dnes. <a href=\"https://x.test/\"></a> </p>",
                "{broken}"
            );
        }
    }

    #[test]
    fn inline_elements_are_marked_for_the_model() {
        let cases = [
            (
                "<p><a href=\"https://x.test/\">Mehr</a> lesen</p>",
                "⟦1⟧Mehr⟦/1⟧ lesen",
            ),
            (
                "<p>Code <b><i>HERBST</i></b> nutzen</p>",
                "Code ⟦1⟧⟦2⟧HERBST⟦/2⟧⟦/1⟧ nutzen",
            ),
            (
                "<p>Hallo <img src=\"data:image/png;base64,AA==\" alt=\"\"> Welt</p>",
                "Hallo ⟦1/⟧ Welt",
            ),
            // A wrapper around the whole piece stays outside: no markers.
            (
                "<td><a href=\"https://x.test/\"><b>Jetzt bestellen</b></a></td>",
                "Jetzt bestellen",
            ),
        ];
        for (html, expected) in cases {
            assert_eq!(segments(html), [expected], "{html}");
        }
    }

    #[test]
    fn an_element_split_by_a_line_break_leaves_its_piece_unmarked() {
        let html = "<p><b>eins<br>zwei</b> drei</p>";
        assert_eq!(segments(html), ["eins", "zwei drei"]);
    }

    #[test]
    fn a_marked_translation_gets_the_original_tags_back() {
        let html = "<p>Lies <a href=\"https://x.test/\">unseren Blog</a> heute.</p>";
        let key = "Lies ⟦1⟧unseren Blog⟦/1⟧ heute.";
        assert_eq!(
            apply(html, &map(&[(key, "Čítaj ⟦1⟧náš blog⟦/1⟧ dnes.")])),
            "<p>Čítaj <a href=\"https://x.test/\">náš blog</a> dnes.</p>"
        );
        // Word order may change; the link moves with its words.
        assert_eq!(
            apply(html, &map(&[(key, "⟦1⟧Náš blog⟦/1⟧ si prečítaj dnes.")])),
            "<p><a href=\"https://x.test/\">Náš blog</a> si prečítaj dnes.</p>"
        );
    }

    #[test]
    fn nested_and_void_markers_come_back_with_their_tags() {
        let html = "<div>\n  Code <b><i>HERBST</i></b> <img src=\"data:image/png;base64,AA==\" alt=\"\"> nutzen\n</div>";
        let key = "Code ⟦1⟧⟦2⟧HERBST⟦/2⟧⟦/1⟧ ⟦3/⟧ nutzen";
        assert_eq!(segments(html), [key]);
        assert_eq!(
            apply(html, &map(&[(key, "Použi kód ⟦1⟧⟦2⟧HERBST⟦/2⟧⟦/1⟧ ⟦3/⟧")])),
            "<div>\n  Použi kód <b><i>HERBST</i></b> <img src=\"data:image/png;base64,AA==\" alt=\"\">\n</div>"
        );
    }

    #[test]
    fn marked_model_output_stays_text_and_brings_no_tags_of_its_own() {
        let html = "<p>Lies <a href=\"https://x.test/\">unseren Blog</a> heute.</p>";
        let key = "Lies ⟦1⟧unseren Blog⟦/1⟧ heute.";
        let hostile = "<script>x()</script> ⟦1⟧<a href=\"https://evil.test/\">blog</a>⟦/1⟧";
        assert_eq!(
            apply(html, &map(&[(key, hostile)])),
            "<p>&lt;script&gt;x()&lt;/script&gt; <a href=\"https://x.test/\">&lt;a href=\"https://evil.test/\"&gt;blog&lt;/a&gt;</a></p>"
        );
    }

    #[test]
    fn surrounding_whitespace_and_untranslated_pieces_stay() {
        let html = "<div>\n  Hallo\n</div><p>Bleibt</p>";
        assert_eq!(
            apply(html, &map(&[("Hallo", "Ahoj")])),
            "<div>\n  Ahoj\n</div><p>Bleibt</p>"
        );
    }

    #[test]
    fn model_output_goes_back_as_text_never_as_markup() {
        let html = "<p>Hallo</p>";
        let hostile = "<script>alert(1)</script><img src=x onerror=alert(1)> & co";
        assert_eq!(
            apply(html, &map(&[("Hallo", hostile)])),
            "<p>&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src=x onerror=alert(1)&gt; &amp; co</p>"
        );
    }

    #[test]
    fn tags_and_attributes_pass_through_byte_for_byte() {
        let html = "<div class=\"x\" style=\"font-size:12px\"><img src=\"data:image/png;base64,AA==\" alt=\"Logo\"><span>Hallo</span></div>";
        assert_eq!(
            apply(html, &map(&[("Hallo", "Ahoj")])),
            html.replace("Hallo", "Ahoj")
        );
    }

    #[test]
    fn text_bodies_translate_by_paragraph() {
        let text = "Hallo Anna,\n\nich schicke dir\ndie Rechnung.\n\n\n12:30\n";
        assert_eq!(
            text_segments(text),
            ["Hallo Anna,", "ich schicke dir die Rechnung."]
        );
        assert_eq!(
            apply_text(
                text,
                &map(&[
                    ("Hallo Anna,", "Ahoj Anna,"),
                    ("ich schicke dir die Rechnung.", "posielam ti faktúru.")
                ])
            ),
            "Ahoj Anna,\n\nposielam ti faktúru.\n\n12:30"
        );
    }
}
