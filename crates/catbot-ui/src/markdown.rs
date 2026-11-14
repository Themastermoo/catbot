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

/// Only addresses that open a web page or a mail: no `javascript:`, no `data:`, nothing local.
fn safe_href(href: &str) -> bool {
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
    fn formatted_replies() {
        assert!(is_formatted("Here:\n- one\n- two"));
        assert!(is_formatted("# Plan"));
        assert!(is_formatted("```\ncode\n```"));
        assert!(is_formatted("| a | b |\n|---|---|"));
        assert!(!is_formatted("Just a sentence with a - dash and #hashtag."));
        assert!(!is_formatted("-not a list"));
    }
}
