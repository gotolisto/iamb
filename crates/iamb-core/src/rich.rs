//! Rich snippets: Markdown to sanitised HTML and to plain text (task 3.8, ADR-0018).
//!
//! A rich snippet's body is a template whose literal text is Markdown (template language §6).
//! The template is rendered first, with every piece of text a macro inserts escaped by
//! [`escape_markdown`] so that it can never become formatting; each run of rendered text
//! between `key` and `wait` actions is then turned into one piece of formatted content by
//! [`render`], which gives the HTML for the clipboard and the plain-text version that every
//! application can take.
//!
//! # The Markdown
//!
//! `CommonMark` with tables and strikethrough (ADR-0018), parsed by `pulldown-cmark`. The HTML is
//! written here, element by element, from the parser's events: nothing the source says reaches
//! the output except through this writer.
//!
//! | Source | HTML | Plain text |
//! | --- | --- | --- |
//! | Paragraph | `<p>` | the text, a blank line between blocks |
//! | Line break in a paragraph (soft or hard) | `<br>` | a line break |
//! | `# Heading` | `<h1>` … `<h6>` | the text |
//! | `*em*`, `**strong**`, `~~del~~` | `<em>`, `<strong>`, `<del>` | the text |
//! | `` `code` ``, fenced or indented code | `<code>`, `<pre><code>` | the text |
//! | `- item`, `1. item` (nested too) | `<ul>`, `<ol start>`, `<li>` | `- item`, `1. item`, nested items indented |
//! | `> quote` | `<blockquote>` | `> ` before each line |
//! | Table | `<table>` with `<thead>`, `<tr>`, `<th>`, `<td>` (`style="text-align:…"` for aligned columns) | cells separated by tabs, one row per line |
//! | `---` | `<hr>` | `---` |
//! | `[text](https://…)` | `<a href="…">text</a>` | `text (https://…)`, or the address alone when the text is the address |
//! | `![alt](attachment:<sha256>)` | `<img src="attachment:<sha256>" alt="alt">` (the service embeds the image) | `alt` |
//!
//! Soft line breaks become `<br>`: a snippet is written line by line, and a signature or an
//! address must keep its lines in a mail app. `CommonMark` allows a renderer to treat soft breaks
//! as hard ones.
//!
//! # Sanitising (architecture §8 rule 10)
//!
//! - **Raw HTML is not interpreted.** An HTML block or inline tag in the source is written as
//!   text, escaped: `<script>` shows as the characters `<script>`.
//! - **Links** may use only `http`, `https` and `mailto` (an email autolink `<a@b.example>` is a
//!   `mailto` link). Any other destination (`javascript:`, `data:`, `file:`, a relative path, a
//!   destination with a control character) keeps its text and loses the link.
//! - **Images** are only attachments: `attachment:` followed by 64 hexadecimal digits. Any other
//!   image (a web address, `data:`, a path) is written as its alt text, so a pasted snippet never
//!   makes a mail client fetch anything.
//! - **Attributes** are only those listed above, with values escaped (`&`, `<`, `>`, `"`, `'`).
//!   No element carries an event handler, a class, an id or a style other than the alignment.
//! - **Nesting** is limited to [`MAX_NESTING`] elements: deeper elements are left out and their
//!   content is kept, so an emphasis or quote "bomb" yields bounded, well-formed HTML.
//! - **Size**: the HTML and the plain text are each limited to [`MAX_RENDERED_BYTES`]; a source
//!   that expands past it (reference links repeating a long address) fails with
//!   [`RichError::TooLarge`], which the engine reports as `E_OUTPUT_TOO_LARGE`.
//!
//! Every Start event of the parser pushes the closing tag it opened (or nothing), and every End
//! event pops it, so the output is balanced whatever the input.

use std::fmt;
use std::str::FromStr;

use pulldown_cmark::{
    Alignment, CowStr, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};

use crate::model::Sha256;

/// The most elements open at once in the HTML; deeper ones are left out (their content stays).
pub const MAX_NESTING: usize = 32;

/// The most bytes the HTML, or the plain text, of one piece of content may have.
pub const MAX_RENDERED_BYTES: usize = 8 * 1024 * 1024;

/// The scheme of an attachment reference: `attachment:<sha256>`.
pub const ATTACHMENT_SCHEME: &str = "attachment:";

/// Why Markdown could not be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RichError {
    /// The HTML or the plain text passed [`MAX_RENDERED_BYTES`].
    TooLarge,
}

impl fmt::Display for RichError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("the rendered content is too large"),
        }
    }
}

impl std::error::Error for RichError {}

/// One piece of formatted content.
///
/// The text can contain field values and clipboard content, so `Debug` prints sizes only.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RichContent {
    /// The HTML fragment. Images reference attachments as `attachment:<sha256>`; the service
    /// replaces them with the data before it puts the HTML on the clipboard.
    pub html: String,
    /// The plain-text version (the fallback).
    pub plain: String,
    /// Every attachment an image references, in order of first appearance, each once.
    pub attachments: Vec<Sha256>,
    /// The attachment, when the content is nothing but one attachment image (ADR-0018: such a
    /// snippet is also offered as a file).
    pub lone_attachment: Option<Sha256>,
}

impl RichContent {
    /// True when there is nothing to paste.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.html.is_empty() && self.plain.is_empty()
    }
}

impl fmt::Debug for RichContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RichContent")
            .field("html_len", &self.html.len())
            .field("plain_len", &self.plain.len())
            .field("attachments", &self.attachments.len())
            .field("lone_attachment", &self.lone_attachment.is_some())
            .finish()
    }
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
}

/// Renders Markdown to sanitised HTML and plain text (module docs).
///
/// # Errors
///
/// [`RichError::TooLarge`] when the HTML or the plain text would pass [`MAX_RENDERED_BYTES`].
pub fn render(markdown: &str) -> Result<RichContent, RichError> {
    let mut html = HtmlWriter::default();
    let mut plain = PlainWriter::default();
    let mut lone = LoneTracker::default();
    let mut attachments: Vec<Sha256> = Vec::new();
    for event in Parser::new_ext(markdown, options()) {
        if let Event::Start(Tag::Image { dest_url, .. }) = &event
            && let Some(sha) = attachment_of(dest_url)
            && !attachments.contains(&sha)
        {
            attachments.push(sha);
        }
        lone.see(&event);
        plain.event(&event)?;
        html.event(event)?;
    }
    Ok(RichContent {
        html: html.finish(),
        plain: plain.finish(),
        attachments,
        lone_attachment: lone.result(),
    })
}

/// The plain-text version of Markdown alone (a plain snippet that includes a rich one receives
/// it; template language §6).
///
/// # Errors
///
/// [`RichError::TooLarge`] as for [`render`].
pub fn to_plain(markdown: &str) -> Result<String, RichError> {
    let mut plain = PlainWriter::default();
    for event in Parser::new_ext(markdown, options()) {
        plain.event(&event)?;
    }
    Ok(plain.finish())
}

/// Every attachment that an image of `markdown` references, in order, each once. The store
/// keeps the snippet's attachment links from it. Macros are not rendered: a reference written
/// in the template's literal text is found; one a field would insert cannot exist, because
/// macro output is escaped.
#[must_use]
pub fn attachment_refs(markdown: &str) -> Vec<Sha256> {
    let mut out = Vec::new();
    for event in Parser::new_ext(markdown, options()) {
        if let Event::Start(Tag::Image { dest_url, .. }) = event
            && let Some(sha) = attachment_of(&dest_url)
            && !out.contains(&sha)
        {
            out.push(sha);
        }
    }
    out
}

/// The attachment an image destination names: `attachment:` and 64 hexadecimal digits.
#[must_use]
pub fn attachment_of(dest: &str) -> Option<Sha256> {
    let rest = strip_prefix_ignore_case(dest.trim(), ATTACHMENT_SCHEME)?;
    if rest.len() != 64 {
        return None;
    }
    Sha256::from_str(rest).ok()
}

/// The Markdown that shows an attachment image: `![alt](attachment:<sha256>)`, with the alt
/// text escaped. What the editor inserts for an image.
#[must_use]
pub fn image_markdown(alt: &str, sha: &Sha256) -> String {
    let mut out = String::from("![");
    escape_markdown_into(alt, &mut out);
    out.push_str("](");
    out.push_str(ATTACHMENT_SCHEME);
    out.push_str(&sha.to_string());
    out.push(')');
    out
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// Escapes text for Markdown so that it reads as exactly that text (template language §6:
/// macro output never becomes formatting).
///
/// Every ASCII punctuation character gets a backslash (`CommonMark` lets any of them be escaped),
/// and spaces and tabs at the start of each line become character references, so that they
/// cannot start an indented code block. Line breaks stay line breaks.
#[must_use]
pub fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    escape_markdown_into(text, &mut out);
    out
}

/// [`escape_markdown`], appending to `out`.
pub fn escape_markdown_into(text: &str, out: &mut String) {
    let mut line_start = true;
    for c in text.chars() {
        match c {
            '\n' => {
                out.push('\n');
                line_start = true;
                continue;
            }
            ' ' if line_start => out.push_str("&#32;"),
            '\t' if line_start => out.push_str("&#9;"),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
        line_start = line_start && (c == ' ' || c == '\t');
    }
}

/// Whether a link destination may be a link, and the `href` to write for it.
fn allowed_href(link_type: LinkType, dest: &str) -> Option<String> {
    if dest.chars().any(char::is_control) {
        return None;
    }
    let dest = dest.trim();
    if link_type == LinkType::Email {
        return Some(attribute_url(&format!("mailto:{dest}")));
    }
    let scheme_end = dest.find(':')?;
    let scheme = &dest[..scheme_end];
    let ok = ["http", "https", "mailto"]
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s));
    ok.then(|| attribute_url(dest))
}

/// A URL as an attribute value: spaces as `%20`, then HTML-escaped.
fn attribute_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            ' ' => out.push_str("%20"),
            c => escape_html_char(c, &mut out),
        }
    }
    out
}

fn escape_html_char(c: char, out: &mut String) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        c => out.push(c),
    }
}

fn escape_html(text: &str, out: &mut String) {
    for c in text.chars() {
        escape_html_char(c, out);
    }
}

/// The text a link shows in plain text, compared with its address: an email or `mailto:`
/// address without the scheme.
fn shown_address(link_type: LinkType, dest: &str) -> String {
    let dest = dest.trim();
    if link_type == LinkType::Email {
        return dest.to_owned();
    }
    strip_prefix_ignore_case(dest, "mailto:").map_or_else(|| dest.to_owned(), str::to_owned)
}

// ---- HTML -------------------------------------------------------------------------------------

/// What an End event closes.
enum Close {
    /// Nothing was opened (a sanitised or too-deep element).
    Nothing,
    /// This closing markup.
    Tag(&'static str),
    /// The end of an image: write it from the collected alt text.
    Image { src: Option<Sha256> },
}

#[derive(Default)]
struct HtmlWriter {
    out: String,
    stack: Vec<Close>,
    /// Elements open in the output (for [`MAX_NESTING`]).
    open: usize,
    /// Inside an image: its alt text is collected here instead of written. An image inside
    /// the alt text (`![a ![b](x)](y)`) adds only its own alt text.
    alt: Option<String>,
    in_head: bool,
    alignments: Vec<Alignment>,
    column: usize,
}

impl HtmlWriter {
    fn push_str(&mut self, s: &str) -> Result<(), RichError> {
        if self.out.len() + s.len() > MAX_RENDERED_BYTES {
            return Err(RichError::TooLarge);
        }
        self.out.push_str(s);
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), RichError> {
        if let Some(alt) = &mut self.alt {
            alt.push_str(text);
            return Ok(());
        }
        let mut escaped = String::with_capacity(text.len());
        escape_html(text, &mut escaped);
        self.push_str(&escaped)
    }

    /// Opens an element unless that would pass the nesting limit.
    fn open(&mut self, markup: &str, close: &'static str) -> Result<(), RichError> {
        if self.alt.is_some() || self.open >= MAX_NESTING {
            self.stack.push(Close::Nothing);
            return Ok(());
        }
        self.open += 1;
        self.stack.push(Close::Tag(close));
        self.push_str(markup)
    }

    fn event(&mut self, event: Event<'_>) -> Result<(), RichError> {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(_) => self.end(),
            Event::Text(t) | Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t)
                if self.alt.is_some() =>
            {
                self.text(&t)
            }
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => self.text(&t),
            Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                self.open("<code>", "</code>")?;
                self.text(&t)?;
                self.end()
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(alt) = &mut self.alt {
                    alt.push(' ');
                    Ok(())
                } else {
                    self.push_str("<br>\n")
                }
            }
            Event::Rule => {
                if self.alt.is_some() {
                    Ok(())
                } else {
                    self.push_str("<hr>\n")
                }
            }
            Event::FootnoteReference(t) => self.text(&format!("[^{t}]")),
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
        }
    }

    fn start(&mut self, tag: Tag<'_>) -> Result<(), RichError> {
        match tag {
            Tag::Paragraph => self.open("<p>", "</p>\n"),
            Tag::Heading { level, .. } => {
                let (open, close) = match level {
                    HeadingLevel::H1 => ("<h1>", "</h1>\n"),
                    HeadingLevel::H2 => ("<h2>", "</h2>\n"),
                    HeadingLevel::H3 => ("<h3>", "</h3>\n"),
                    HeadingLevel::H4 => ("<h4>", "</h4>\n"),
                    HeadingLevel::H5 => ("<h5>", "</h5>\n"),
                    HeadingLevel::H6 => ("<h6>", "</h6>\n"),
                };
                self.open(open, close)
            }
            Tag::BlockQuote(_) => self.open("<blockquote>\n", "</blockquote>\n"),
            Tag::CodeBlock(_) => self.open("<pre><code>", "</code></pre>\n"),
            Tag::List(Some(start)) if start != 1 => {
                self.open(&format!("<ol start=\"{start}\">\n"), "</ol>\n")
            }
            Tag::List(Some(_)) => self.open("<ol>\n", "</ol>\n"),
            Tag::List(None) => self.open("<ul>\n", "</ul>\n"),
            Tag::Item => self.open("<li>", "</li>\n"),
            Tag::Table(alignments) => {
                self.alignments = alignments;
                self.open("<table>\n", "</table>\n")
            }
            Tag::TableHead => {
                self.in_head = true;
                self.column = 0;
                self.open("<thead><tr>", "</tr></thead>\n")
            }
            Tag::TableRow => {
                self.column = 0;
                self.open("<tr>", "</tr>\n")
            }
            Tag::TableCell => {
                let name = if self.in_head { "th" } else { "td" };
                let align = match self.alignments.get(self.column) {
                    Some(Alignment::Left) => " style=\"text-align:left\"",
                    Some(Alignment::Center) => " style=\"text-align:center\"",
                    Some(Alignment::Right) => " style=\"text-align:right\"",
                    Some(Alignment::None) | None => "",
                };
                self.column += 1;
                let close = if self.in_head { "</th>" } else { "</td>" };
                self.open(&format!("<{name}{align}>"), close)
            }
            Tag::Emphasis => self.open("<em>", "</em>"),
            Tag::Strong => self.open("<strong>", "</strong>"),
            Tag::Strikethrough => self.open("<del>", "</del>"),
            Tag::Superscript => self.open("<sup>", "</sup>"),
            Tag::Subscript => self.open("<sub>", "</sub>"),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                if let Some(href) = allowed_href(link_type, &dest_url) {
                    self.open(&format!("<a href=\"{href}\">"), "</a>")
                } else {
                    self.stack.push(Close::Nothing);
                    Ok(())
                }
            }
            Tag::Image { dest_url, .. } => {
                if self.alt.is_some() {
                    self.stack.push(Close::Nothing);
                } else {
                    self.alt = Some(String::new());
                    self.stack.push(Close::Image {
                        src: attachment_of(&dest_url),
                    });
                }
                Ok(())
            }
            // Raw HTML blocks are written as text (their lines arrive as `Html` events);
            // footnotes, definition lists and metadata are not enabled.
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {
                self.stack.push(Close::Nothing);
                Ok(())
            }
        }
    }

    fn end(&mut self) -> Result<(), RichError> {
        match self.stack.pop() {
            Some(Close::Tag(close)) => {
                self.open -= 1;
                if close.starts_with("</tr></thead>") {
                    self.in_head = false;
                }
                self.push_str(close)
            }
            Some(Close::Image { src }) => {
                let alt = self.alt.take().unwrap_or_default();
                match src {
                    Some(sha) if self.open < MAX_NESTING => {
                        let mut tag = format!("<img src=\"{ATTACHMENT_SCHEME}{sha}\" alt=\"");
                        escape_html(&alt, &mut tag);
                        tag.push_str("\">");
                        self.push_str(&tag)
                    }
                    _ => self.text(&alt),
                }
            }
            Some(Close::Nothing) | None => Ok(()),
        }
    }

    fn finish(mut self) -> String {
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out
    }
}

// ---- Plain text ---------------------------------------------------------------------------------

#[derive(Default)]
struct PlainWriter {
    out: String,
    /// Nothing written on the current line yet: the prefixes come first.
    line_start: bool,
    /// What starts each continuation line: list indentation and `> ` for quotes.
    prefixes: Vec<String>,
    /// The marker of a list item, written instead of its indentation on its first line.
    marker: Option<(usize, String)>,
    /// A block ended: the next block starts after a blank line.
    blank: bool,
    /// The number of the next item of each open list (`None`: a bullet list).
    lists: Vec<Option<u64>>,
    /// Where the text of each open link starts, its address and its type.
    links: Vec<(usize, Option<(LinkType, String)>)>,
    /// Inside an image: the alt text is written, nothing else.
    images: usize,
    /// The first cell of a table row is next.
    first_cell: bool,
}

impl PlainWriter {
    fn raw(&mut self, s: &str) -> Result<(), RichError> {
        if self.out.len() + s.len() > MAX_RENDERED_BYTES {
            return Err(RichError::TooLarge);
        }
        self.out.push_str(s);
        Ok(())
    }

    fn newline(&mut self) -> Result<(), RichError> {
        self.raw("\n")?;
        self.line_start = true;
        Ok(())
    }

    fn write_prefixes(&mut self, trim: bool) -> Result<(), RichError> {
        let mut prefix = String::new();
        let marker = self.marker.take();
        for (i, p) in self.prefixes.iter().enumerate() {
            match &marker {
                Some((at, m)) if *at == i => prefix.push_str(m),
                _ => prefix.push_str(p),
            }
        }
        if trim {
            let len = prefix.trim_end().len();
            prefix.truncate(len);
            self.marker = marker;
        }
        self.raw(&prefix)
    }

    fn text(&mut self, text: &str) -> Result<(), RichError> {
        let mut first = true;
        for line in text.split('\n') {
            if !first {
                self.newline()?;
            }
            first = false;
            if line.is_empty() {
                continue;
            }
            if self.line_start {
                self.write_prefixes(false)?;
                self.line_start = false;
            }
            self.raw(line)?;
        }
        Ok(())
    }

    fn start_block(&mut self) -> Result<(), RichError> {
        if !self.out.is_empty() {
            if !self.line_start {
                self.newline()?;
            }
            if self.blank {
                self.write_prefixes(true)?;
                self.newline()?;
            }
        }
        self.blank = false;
        self.line_start = true;
        Ok(())
    }

    fn end_block(&mut self) {
        self.blank = true;
    }

    fn event(&mut self, event: &Event<'_>) -> Result<(), RichError> {
        if self.images > 0 {
            return match event {
                Event::Start(Tag::Image { .. }) => {
                    self.images += 1;
                    Ok(())
                }
                Event::End(TagEnd::Image) => {
                    self.images -= 1;
                    Ok(())
                }
                Event::Text(t) | Event::Code(t) => self.text(t),
                Event::SoftBreak | Event::HardBreak => self.text(" "),
                _ => Ok(()),
            };
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(*end),
            Event::Text(t)
            | Event::Code(t)
            | Event::Html(t)
            | Event::InlineHtml(t)
            | Event::InlineMath(t)
            | Event::DisplayMath(t) => self.text(t),
            Event::SoftBreak | Event::HardBreak => self.newline(),
            Event::Rule => {
                self.start_block()?;
                self.text("---")?;
                self.end_block();
                Ok(())
            }
            Event::FootnoteReference(t) => self.text(&format!("[^{t}]")),
            Event::TaskListMarker(done) => self.text(if *done { "[x] " } else { "[ ] " }),
        }
    }

    fn start(&mut self, tag: &Tag<'_>) -> Result<(), RichError> {
        match tag {
            Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::Table(_) => self.start_block(),
            Tag::BlockQuote(_) => {
                self.start_block()?;
                self.prefixes.push("> ".to_owned());
                Ok(())
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.start_block()?;
                } else if !self.line_start {
                    self.newline()?;
                }
                self.lists.push(*start);
                Ok(())
            }
            Tag::Item => {
                if !self.out.is_empty() && !self.line_start {
                    self.newline()?;
                }
                self.blank = false;
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "- ".to_owned(),
                };
                self.prefixes.push(" ".repeat(marker.len()));
                self.marker = Some((self.prefixes.len() - 1, marker));
                self.line_start = true;
                Ok(())
            }
            Tag::TableHead | Tag::TableRow => {
                if !self.line_start {
                    self.newline()?;
                }
                self.first_cell = true;
                Ok(())
            }
            Tag::TableCell => {
                if !self.first_cell {
                    if self.line_start {
                        self.write_prefixes(false)?;
                        self.line_start = false;
                    }
                    self.raw("\t")?;
                }
                self.first_cell = false;
                Ok(())
            }
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                let address = allowed_href(*link_type, dest_url)
                    .map(|_| (*link_type, dest_url.trim().to_owned()));
                self.links.push((self.out.len(), address));
                Ok(())
            }
            Tag::Image { .. } => {
                self.images = 1;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn end(&mut self, end: pulldown_cmark::TagEnd) -> Result<(), RichError> {
        use pulldown_cmark::TagEnd;
        match end {
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::Table => {
                self.end_block();
                Ok(())
            }
            TagEnd::BlockQuote(_) => {
                self.prefixes.pop();
                self.end_block();
                Ok(())
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.end_block();
                Ok(())
            }
            TagEnd::Item => {
                let at = self.prefixes.len().saturating_sub(1);
                if self.line_start && self.marker.as_ref().is_some_and(|(i, _)| *i == at) {
                    // An empty item: its marker alone.
                    self.write_prefixes(false)?;
                    self.line_start = false;
                }
                self.marker = None;
                self.prefixes.pop();
                Ok(())
            }
            TagEnd::Link => {
                if let Some((start, Some((link_type, dest)))) = self.links.pop() {
                    let shown = self.out.get(start..).unwrap_or_default().trim().to_owned();
                    let address = shown_address(link_type, &dest);
                    if shown != address && shown != dest {
                        if shown.is_empty() {
                            self.text(&address)?;
                        } else {
                            self.text(&format!(" ({address})"))?;
                        }
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn finish(mut self) -> String {
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out
    }
}

// ---- Lone attachment ----------------------------------------------------------------------------

/// Whether a document is one attachment image and nothing else (white space aside).
#[derive(Default)]
struct LoneTracker {
    found: Option<Sha256>,
    other: bool,
    in_image: usize,
}

impl LoneTracker {
    fn see(&mut self, event: &Event<'_>) {
        if self.other {
            return;
        }
        if self.in_image > 0 {
            match event {
                Event::Start(Tag::Image { .. }) => self.in_image += 1,
                Event::End(TagEnd::Image) => self.in_image -= 1,
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(Tag::Paragraph)
            | Event::End(TagEnd::Paragraph)
            | Event::SoftBreak
            | Event::HardBreak => {}
            Event::Start(Tag::Image { dest_url, .. }) => match attachment_of(dest_url) {
                Some(sha) if self.found.is_none() => {
                    self.found = Some(sha);
                    self.in_image = 1;
                }
                _ => self.other = true,
            },
            Event::Text(t) if is_blank(t) => {}
            _ => self.other = true,
        }
    }

    fn result(&self) -> Option<Sha256> {
        if self.other { None } else { self.found }
    }
}

fn is_blank(t: &CowStr<'_>) -> bool {
    t.chars().all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aa00000000000000000000000000000000000000000000000000000000000001";

    fn html(md: &str) -> String {
        render(md).expect("renders").html
    }

    fn plain(md: &str) -> String {
        render(md).expect("renders").plain
    }

    #[test]
    fn basic_formatting() {
        assert_eq!(
            html("**bold** and *it*"),
            "<p><strong>bold</strong> and <em>it</em></p>"
        );
        assert_eq!(plain("**bold** and *it*"), "bold and it");
        assert_eq!(html("~~gone~~"), "<p><del>gone</del></p>");
        assert_eq!(html("# Title"), "<h1>Title</h1>");
        assert_eq!(html("a\nb"), "<p>a<br>\nb</p>");
        assert_eq!(plain("a\nb\n\nc"), "a\nb\n\nc");
    }

    #[test]
    fn lists_nest() {
        let md = "- one\n- two\n  - inner\n  - more\n- three";
        assert_eq!(
            html(md),
            "<ul>\n<li>one</li>\n<li>two<ul>\n<li>inner</li>\n<li>more</li>\n</ul>\n</li>\n<li>three</li>\n</ul>"
        );
        assert_eq!(plain(md), "- one\n- two\n  - inner\n  - more\n- three");
        assert_eq!(plain("3. c\n4. d"), "3. c\n4. d");
        assert_eq!(
            html("3. c\n4. d"),
            "<ol start=\"3\">\n<li>c</li>\n<li>d</li>\n</ol>"
        );
        assert_eq!(
            plain("text\n\n- a\n- b\n\nafter"),
            "text\n\n- a\n- b\n\nafter"
        );
    }

    #[test]
    fn quotes_and_code() {
        assert_eq!(plain("> one\n> two"), "> one\n> two");
        assert_eq!(html("`x<y`"), "<p><code>x&lt;y</code></p>");
        assert_eq!(html("```\na<b\n```"), "<pre><code>a&lt;b\n</code></pre>");
        assert_eq!(plain("```\nline 1\nline 2\n```"), "line 1\nline 2");
    }

    #[test]
    fn links_are_limited_to_three_schemes() {
        assert_eq!(
            html("[site](https://example.com/a?b=1&c=2)"),
            "<p><a href=\"https://example.com/a?b=1&amp;c=2\">site</a></p>"
        );
        assert_eq!(
            plain("[site](https://example.com)"),
            "site (https://example.com)"
        );
        assert_eq!(plain("<https://example.com>"), "https://example.com");
        assert_eq!(
            html("<kim@example.com>"),
            "<p><a href=\"mailto:kim@example.com\">kim@example.com</a></p>"
        );
        assert_eq!(
            plain("[kim@example.com](mailto:kim@example.com)"),
            "kim@example.com"
        );
        for bad in [
            "[x](javascript:alert(1))",
            "[x](JavaScript:alert(1))",
            "[x](data:text/html,hi)",
            "[x](file:///etc/passwd)",
            "[x](relative/path)",
            "[x](vbscript:msgbox)",
        ] {
            let out = html(bad);
            assert_eq!(out, "<p>x</p>", "{bad}");
            assert_eq!(plain(bad), "x", "{bad}");
        }
    }

    #[test]
    fn raw_html_is_text() {
        assert_eq!(
            html("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
        assert_eq!(
            html("hi <img src=x onerror=alert(1)> there"),
            "<p>hi &lt;img src=x onerror=alert(1)&gt; there</p>"
        );
        assert_eq!(plain("hi <b>x</b>"), "hi <b>x</b>");
    }

    #[test]
    fn images_are_attachments_only() {
        let md = format!("![Logo \"x\"](attachment:{SHA})");
        let out = render(&md).expect("renders");
        assert_eq!(
            out.html,
            format!("<p><img src=\"attachment:{SHA}\" alt=\"Logo &quot;x&quot;\"></p>")
        );
        assert_eq!(out.plain, "Logo \"x\"");
        assert_eq!(out.attachments.len(), 1);
        assert_eq!(out.lone_attachment, out.attachments.first().copied());
        assert_eq!(html("![pic](https://tracker.example/p.gif)"), "<p>pic</p>");
        assert_eq!(html("![pic](data:image/png;base64,AAAA)"), "<p>pic</p>");
        let upper = format!("![a](ATTACHMENT:{})", SHA.to_uppercase());
        assert!(html(&upper).contains(&format!("attachment:{SHA}")));
        let two = format!("hi ![a](attachment:{SHA})");
        assert_eq!(render(&two).expect("renders").lone_attachment, None);
    }

    #[test]
    fn tables() {
        let md = "| a | b |\n|:--|--:|\n| 1 | 2 |";
        assert_eq!(
            html(md),
            "<table>\n<thead><tr><th style=\"text-align:left\">a</th><th style=\"text-align:right\">b</th></tr></thead>\n<tr><td style=\"text-align:left\">1</td><td style=\"text-align:right\">2</td></tr>\n</table>"
        );
        assert_eq!(plain(md), "a\tb\n1\t2");
    }

    #[test]
    fn escaping_makes_text_literal() {
        for text in [
            "**not bold**",
            "# not a heading",
            "- not a list",
            "1. not a list",
            "    not code",
            "[a](https://x)",
            "<b>tag</b>",
            "a\n===",
            "`tick`",
            "| a | b |",
            "back\\slash",
            "&amp;",
        ] {
            let escaped = escape_markdown(text);
            let out = render(&escaped).expect("renders");
            assert_eq!(out.plain, text, "{text:?} → {escaped:?}");
            assert!(
                !out.html.contains("<strong>") && !out.html.contains("<a "),
                "{text}"
            );
        }
    }

    #[test]
    fn nesting_is_bounded() {
        let md = format!("{}x{}", "*".repeat(200), "*".repeat(200));
        let out = html(&md);
        assert!(out.matches("<em>").count() + out.matches("<strong>").count() <= MAX_NESTING);
        let quotes = format!("{}deep", "> ".repeat(500));
        let out = html(&quotes);
        assert!(out.matches("<blockquote>").count() <= MAX_NESTING);
        assert_eq!(
            out.matches("<blockquote>").count(),
            out.matches("</blockquote>").count()
        );
    }

    #[test]
    fn output_size_is_bounded() {
        // The parser itself stops reference links from repeating a long address many times.
        let url = format!("https://example.com/{}", "a".repeat(100_000));
        let md = format!("[r]: {url}\n\n{}", "[r] ".repeat(1000));
        match render(&md) {
            Ok(out) => assert!(out.html.len() <= MAX_RENDERED_BYTES),
            Err(e) => assert_eq!(e, RichError::TooLarge),
        }
        let huge = "a&".repeat(MAX_RENDERED_BYTES / 4 + 1);
        assert_eq!(render(&huge), Err(RichError::TooLarge));
    }

    #[test]
    fn debug_shows_sizes_only() {
        let out = render("secret words").expect("renders");
        let shown = format!("{out:?}");
        assert!(!shown.contains("secret"));
    }
}
