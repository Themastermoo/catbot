//! A bot's reply as HTML, in the classes OpenMausBot's `src/components/ChatMarkdown.tsx`
//! (Apache-2.0, see NOTICE) gives each element, so the stylesheet's `.chat-md` rules apply.
//!
//! Everything a bot writes is escaped: raw HTML in a reply shows as text, and only web and
//! mail links become links. The result is safe to put in the page as HTML.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Their link look (`LINK_CLASS` in `ChatLinks.tsx`).
const LINK: &str = "break-words text-accent underline decoration-accent/40 hover:decoration-accent [unicode-bidi:isolate]";

/// Markdown → HTML for the inside of a `.chat-md` block.
pub fn to_html(text: &str) -> String {
    render(text, false)
}

/// Cached `(is_formatted, html)` for historical chat replies so re-renders never re-parse
/// unchanged Markdown. Bounded to 512 entries per UI thread.
pub fn to_html_cached(text: &str) -> (bool, String) {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<String, (bool, String)>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cell| {
        if let Some(hit) = cell.borrow().get(text) {
            return hit.clone();
        }
        let formatted = is_formatted(text);
        let html = render(text, false);
        let mut map = cell.borrow_mut();
        if map.len() >= 512 {
            map.clear();
        }
        map.insert(text.to_string(), (formatted, html.clone()));
        (formatted, html)
    })
}

/// A memory note as HTML: like a reply, with its `[[wikilinks]]` as links to other notes
/// (`<a data-note="target">`, opened by the memory viewer).
pub fn note_html(text: &str) -> String {
    render(&wikilinks(text), true)
}

fn render(text: &str, notes: bool) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = String::with_capacity(text.len() * 2);
    // Links whose address wasn't safe: their closing tag is skipped too.
    let mut dropped_links = Vec::<bool>::new();
    let mut table: Vec<Alignment> = Vec::new();
    let mut in_head = false;
    let mut cell = 0;
    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => out.push_str(heading(level)),
                Tag::BlockQuote(_) => out.push_str(r#"<blockquote class="border-s-2 border-hairline ps-3 text-ink-secondary">"#),
                Tag::CodeBlock(kind) => {
                    let language = match kind {
                        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    out.push_str(r#"<div dir="ltr" class="chat-code-block my-2 overflow-hidden rounded-lg border border-hairline/40 bg-inset">"#);
                    if !language.is_empty() {
                        out.push_str(r#"<div class="flex items-center gap-2 border-b border-hairline/30 bg-raised/30 px-3 py-1.5 text-xs"><span class="min-w-0 truncate rounded border border-hairline/40 bg-raised px-1.5 py-0.5 text-[11px] font-medium tracking-wide text-ink select-none">"#);
                        escape_into(&mut out, &language);
                        out.push_str("</span></div>");
                    }
                    out.push_str(r#"<pre class="overflow-x-auto p-3 text-[13px] leading-relaxed text-ink"><code>"#);
                }
                Tag::List(Some(start)) => {
                    out.push_str(r#"<ol class="list-decimal space-y-1 ps-5""#);
                    if start != 1 {
                        out.push_str(&format!(r#" start="{start}""#));
                    }
                    out.push('>');
                }
                Tag::List(None) => out.push_str(r#"<ul class="list-disc space-y-1 ps-5">"#),
                Tag::Item => out.push_str("<li>"),
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::Strikethrough => out.push_str("<del>"),
                Tag::Link { dest_url, .. } if notes && dest_url.starts_with(NOTE) => {
                    out.push_str(r##"<a href="#" data-note=""##);
                    escape_into(&mut out, &dest_url[NOTE.len()..]);
                    out.push_str(&format!(r#"" dir="auto" class="{LINK}">"#));
                    dropped_links.push(false);
                }
                Tag::Link { dest_url, .. } => {
                    let safe = safe_href(&dest_url);
                    if safe {
                        out.push_str(r#"<a href=""#);
                        escape_into(&mut out, &dest_url);
                        out.push_str(&format!(r#"" target="_blank" rel="noreferrer" dir="auto" class="{LINK}">"#));
                    }
                    dropped_links.push(!safe);
                }
                // A picture by its description: the app shows no outside images.
                Tag::Image { .. } => out.push_str(r#"<span class="text-ink-secondary">"#),
                Tag::Table(alignments) => {
                    table = alignments;
                    out.push_str(r#"<div class="chat-table my-2 overflow-x-auto"><table class="w-full border-collapse text-[14px]">"#);
                }
                Tag::TableHead => {
                    in_head = true;
                    cell = 0;
                    out.push_str("<thead><tr>");
                }
                Tag::TableRow => {
                    cell = 0;
                    out.push_str("<tr>");
                }
                Tag::TableCell => {
                    let align = match table.get(cell) {
                        Some(Alignment::Center) => " text-center",
                        Some(Alignment::Right) => " text-end",
                        _ => "",
                    };
                    if in_head {
                        out.push_str(&format!(r#"<th class="border-b border-hairline/40 px-2 py-1.5 text-start font-semibold{align}">"#));
                    } else {
                        out.push_str(&format!(r#"<td class="border-b border-hairline/20 px-2 py-1.5 align-top{align}">"#));
                    }
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => out.push_str("</p>"),
                TagEnd::Heading(_) => out.push_str("</div>"),
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>"),
                TagEnd::CodeBlock => out.push_str("</code></pre></div>"),
                TagEnd::List(true) => out.push_str("</ol>"),
                TagEnd::List(false) => out.push_str("</ul>"),
                TagEnd::Item => out.push_str("</li>"),
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::Strikethrough => out.push_str("</del>"),
                TagEnd::Link => {
                    if dropped_links.pop() == Some(false) {
                        out.push_str("</a>");
                    }
                }
                TagEnd::Image => out.push_str("</span>"),
                TagEnd::Table => out.push_str("</tbody></table></div>"),
                TagEnd::TableHead => {
                    in_head = false;
                    out.push_str("</tr></thead><tbody>");
                }
                TagEnd::TableRow => out.push_str("</tr>"),
                TagEnd::TableCell => {
                    cell += 1;
                    out.push_str(if in_head { "</th>" } else { "</td>" });
                }
                _ => {}
            },
            Event::Text(text) => escape_into(&mut out, &text),
            Event::Code(code) => {
                out.push_str(r#"<code dir="ltr" class="ui-code-chip break-words [unicode-bidi:isolate]">"#);
                escape_into(&mut out, &code);
                out.push_str("</code>");
            }
            // Raw HTML is shown as the text it is, never run.
            Event::Html(html) | Event::InlineHtml(html) => escape_into(&mut out, &html),
            // A single line break stays a line break, as their `remarkSoftBreaks` makes it: poems keep their lines.
            Event::SoftBreak => out.push_str("<br>"),
            Event::HardBreak => out.push_str("<br>"),
            Event::Rule => out.push_str(r#"<hr class="border-hairline/40">"#),
            Event::TaskListMarker(done) => out.push_str(if done { "☑ " } else { "☐ " }),
            Event::FootnoteReference(name) => escape_into(&mut out, &format!("[{name}]")),
            _ => {}
        }
    }
    out
}

fn heading(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => r#"<div class="md-heading mt-3 text-[20px] font-semibold leading-[26px] tracking-[-0.01em]">"#,
        HeadingLevel::H2 => r#"<div class="md-heading mt-2 text-[17px] font-semibold leading-6 tracking-[-0.005em]">"#,
        HeadingLevel::H3 | HeadingLevel::H4 => r#"<div class="mt-1.5 font-semibold">"#,
        HeadingLevel::H5 => r#"<div class="mt-1.5 text-[14px] font-semibold">"#,
        HeadingLevel::H6 => r#"<div class="mt-1.5 text-[14px] font-semibold text-ink-secondary">"#,
    }
}

/// How a wikilink's target is carried through the Markdown parser (percent-encoded after it).
const NOTE: &str = "note:";

/// Turns `[[target]]`, `[[target|label]]` and `![[target]]` into Markdown links to `note:target`,
/// outside code. The label defaults to the target's last part, without any `#heading`.
fn wikilinks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        if fenced || trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        let mut in_code = false;
        while !rest.is_empty() {
            if rest.starts_with('`') {
                in_code = !in_code;
                out.push('`');
                rest = &rest[1..];
                continue;
            }
            if !in_code
                && rest.starts_with("[[")
                && let Some(end) = rest[2..].find("]]").filter(|&end| !rest[2..2 + end].contains(['\n', '[']))
            {
                let inner = &rest[2..2 + end];
                let (target, label) = match inner.split_once('|') {
                    Some((target, label)) => (target.trim(), label.trim().to_string()),
                    None => {
                        let target = inner.trim();
                        let shown = target.split('#').next().unwrap_or(target);
                        (target, shown.rsplit('/').next().unwrap_or(shown).to_string())
                    }
                };
                if out.ends_with('!') {
                    out.pop();
                }
                let label = if label.is_empty() { target.to_string() } else { label };
                out.push('[');
                out.push_str(&label.replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]"));
                out.push_str("](");
                out.push_str(NOTE);
                out.push_str(&encode(target));
                out.push(')');
                rest = &rest[2 + end + 2..];
                continue;
            }
            let c = rest.chars().next().unwrap_or_default();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Percent-encodes all but letters, digits and `-_.~/`.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Undoes `encode`.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Only addresses that open a web page or a mail: no `javascript:`, no `data:`, nothing local.
pub fn safe_href(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
}

/// Whether a reply reads as a document (a list, heading, code, quote or table) rather than a
/// chat line; their `isFormattedReply`. A document is drawn without a bubble, at prose width.
pub fn is_formatted(text: &str) -> bool {
    text.lines().any(|line| {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > 3 {
            return false;
        }
        let line = line.trim_start_matches(' ');
        let list = {
            let digits = line.chars().take_while(char::is_ascii_digit).count();
            let after = &line[digits..];
            (["- ", "* ", "+ "].iter().any(|m| line.starts_with(m)) && line.chars().nth(2).is_some_and(|c| !c.is_whitespace()))
                || ((1..=3).contains(&digits)
                    && (after.starts_with(". ") || after.starts_with(") "))
                    && after.chars().nth(2).is_some_and(|c| !c.is_whitespace()))
        };
        let heading = {
            let hashes = line.chars().take_while(|&c| c == '#').count();
            (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') && !line[hashes..].trim().is_empty()
        };
        let fence = line.starts_with("```") || line.starts_with("~~~");
        let quote = line.starts_with('>') && line[1..].trim_start().chars().next().is_some_and(|c| !c.is_whitespace());
        let table = {
            let row = line.trim_start_matches('|').trim_start();
            let row = row.trim_start_matches(':');
            row.starts_with("---") && row.contains('|')
        };
        list || heading || fence || quote || table
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_get_their_classes() {
        let html = to_html("## Title\n\n- one\n- **two**\n\n`x`");
        assert!(html.contains(r#"<div class="md-heading mt-2 text-[17px]"#), "{html}");
        assert!(html.contains(r#"<ul class="list-disc space-y-1 ps-5"><li>one</li><li><strong>two</strong></li></ul>"#), "{html}");
        assert!(html.contains(r#"class="ui-code-chip"#), "{html}");
    }

    #[test]
    fn nothing_a_bot_writes_runs() {
        let html = to_html("<script>alert(1)</script>\n\n[x](javascript:alert(1)) <img src=x onerror=alert(1)>");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("href=\"javascript"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        let link = to_html("[site](https://example.com/?a=\"b\")");
        assert!(link.contains(r#"href="https://example.com/?a=&quot;b&quot;" target="_blank""#), "{link}");
    }

    #[test]
    fn single_line_breaks_stay() {
        assert_eq!(to_html("one\ntwo"), "<p>one<br>two</p>");
        assert!(to_html("> a\n> b").contains("a<br>b"));
    }

    #[test]
    fn code_and_tables() {
        let html = to_html("```rust\nfn main() {}\n```\n\n| a | b |\n|---|--:|\n| 1 | 2 |");
        assert!(html.contains(">rust</span>") && html.contains("<code>fn main() {}\n</code>"), "{html}");
        assert!(html.contains(r#"<td class="border-b border-hairline/20 px-2 py-1.5 align-top text-end">2</td>"#), "{html}");
    }

    #[test]
    fn notes_link_to_notes() {
        let html = note_html("See [[Bots/Scout]], [[Codex/20 Projects/Catbot|Catbot]] and ![[Garden#Soil]].\n\n`[[not a link]]`");
        assert!(html.contains(r##"<a href="#" data-note="Bots/Scout" dir="auto""##) && html.contains(">Scout</a>"), "{html}");
        assert!(html.contains(r#"data-note="Codex/20%20Projects/Catbot""#) && html.contains(">Catbot</a>"), "{html}");
        assert!(html.contains(r#"data-note="Garden%23Soil""#) && html.contains(">Garden</a>") && !html.contains('!'), "{html}");
        assert!(html.contains("<code") && html.contains("[[not a link]]"), "{html}");
        assert_eq!(decode("Codex/20%20Projects/Catbot"), "Codex/20 Projects/Catbot");
        // In a reply, nothing is a note link.
        assert!(!to_html("[x](note:Scout)").contains("data-note"));
        let fenced = note_html("```\n[[Scout]]\n```");
        assert!(!fenced.contains("data-note"), "{fenced}");
    }

    #[test]
    fn formatted_replies() {
        assert!(is_formatted("Here:\n- one\n- two"));
        assert!(is_formatted("# Plan"));
        assert!(is_formatted("```\ncode\n```"));
        assert!(is_formatted("| a | b |\n|---|---|"));
        assert!(!is_formatted("Just a sentence with a - dash and #hashtag."));
        assert!(!is_formatted("-not a list"));
    }
}
