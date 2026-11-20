//! The palette's search: fuzzy matching of a query against snippets by abbreviation, label,
//! content and tag (task 3.6; `docs/ux/flows/5-search.md`, `docs/ux/spec.md` §4.5).
//!
//! Pure and stateless: the caller hands over the snippets (borrowed) and gets the best hits back,
//! each with the field that decided its score and the matched ranges in UTF-16 code units of
//! that field, so that the front end can mark the same characters the ranking used. Nothing is
//! kept between queries (the query is the user's text: never stored, never logged, architecture
//! §8), so there is no index to build, to hold in memory (service idle memory budget) or to
//! invalidate when the library changes.
//!
//! # Ranking
//!
//! The query is split at white space into tokens (at most [`MAX_TOKENS`], the whole query at
//! most [`MAX_QUERY_CHARS`] characters). Every token must match the snippet somewhere; each
//! token takes the best score of any field, where a field is looked at only when no field of a
//! better tier matched: **abbreviation, then label, then content, then tag, then a typo**. All
//! comparisons fold case like the matcher does ([`fold_char`]).
//!
//! | Tier | Match | Score |
//! | --- | --- | --- |
//! | Abbreviation | equal | 1000 |
//! | | starts with the token | 900, less 1 per extra character (at most 99) |
//! | | contains it | 800, less 1 per other character (at most 99) |
//! | | the token's characters in order (two or more) | 700, less 1 per character skipped (at most 99) |
//! | Label | starts with it | 650, less 1 per extra character (at most 49) |
//! | | a word starts with it | 590, less 1 per character before it (at most 49) |
//! | | contains it | 540, less 1 per character before it (at most 49) |
//! | | the token's characters in order (three or more), within one and a half times their number | 500, less 1 per character skipped (at most 49) |
//! | Content | contains it at the start of a word | 400, less 1 per 32 characters before it (at most 49) |
//! | | contains it | 350, less the same |
//! | Tag | equal / starts with / contains | 300 / 280 / 260 |
//! | Typo | the token (four or more characters) is one edit from, or from the start of, the abbreviation, a word of the label or of a tag (two edits from eight characters on) | 190 / 150 / 110, less 30 for the second edit |
//!
//! A query of several tokens scores the mean of its tokens' scores and reports the field and
//! ranges of its best token (ranges of the other tokens that matched the same field are added).
//! Ties keep the order the snippets were handed over in, so the same query always gives the
//! same list.
//!
//! An edit is an insertion, a deletion, a substitution or the swap of two neighbours (the
//! restricted Damerau-Levenshtein distance), which is what typing mistakes are made of.
//!
//! # Speed
//!
//! A query visits every snippet once and allocates nothing per snippet: the short fields are
//! folded into two scratch buffers, an ASCII body is searched in place, byte by byte. A
//! snippet that matched in a better tier is not looked at in the lower ones. Ranges are worked
//! out for the returned hits only. At 10,000 snippets of up to 500 characters it takes a few
//! milliseconds in a release build (`crates/iamb-bench/benches/search.rs`,
//! `docs/test-strategy.md` §8.7).

use std::cmp::Reverse;

use crate::model::{Id, fold_char};

/// The most characters of a query that count; the rest is ignored.
pub const MAX_QUERY_CHARS: usize = 64;

/// The most tokens (words) of a query that count; the rest is ignored.
pub const MAX_TOKENS: usize = 6;

/// The shortest token that is forgiven a typo.
const FUZZY_MIN: usize = 4;

/// From this many characters on a token is forgiven two edits.
const FUZZY_TWO_EDITS: usize = 8;

/// The most characters of a word that are compared when forgiving a typo.
const WORD_CAP: usize = 48;

/// Which field of a snippet decided a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// The abbreviation.
    Abbreviation,
    /// The label.
    Label,
    /// The content (the template source).
    Body,
    /// The name of a tag.
    Tag,
}

/// A snippet to search, borrowed from the caller.
#[derive(Debug, Clone, Copy)]
pub struct Doc<'a> {
    /// The snippet.
    pub id: Id,
    /// What the user types.
    pub abbreviation: &'a str,
    /// The label (may be empty).
    pub label: &'a str,
    /// The content (template source).
    pub body: &'a str,
    /// The snippet's tags, resolved to names by the `tag_name` function of [`search`].
    pub tags: &'a [Id],
}

/// One result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The snippet.
    pub id: Id,
    /// Higher is better (see the module docs).
    pub score: u32,
    /// The field that decided the score.
    pub field: Field,
    /// The matched ranges `[start, end)` in that field, in UTF-16 code units, ascending and not
    /// overlapping (for a tag, in the tag's name).
    pub ranges: Vec<[usize; 2]>,
}

/// A query, folded and split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    tokens: Vec<Token>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    chars: Vec<char>,
    /// The token as lower-case ASCII bytes, when it is all ASCII (the fast path for bodies).
    ascii: Option<Vec<u8>>,
}

impl Query {
    /// Parses `text`; `None` when it holds nothing to look for.
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut current: Vec<char> = Vec::new();
        let mut taken = 0;
        for c in text.chars() {
            if c.is_whitespace() {
                if !current.is_empty() {
                    tokens.push(Token::new(std::mem::take(&mut current)));
                    if tokens.len() == MAX_TOKENS {
                        break;
                    }
                }
                continue;
            }
            taken += 1;
            if taken > MAX_QUERY_CHARS {
                break;
            }
            current.extend(fold_char(c));
        }
        if !current.is_empty() && tokens.len() < MAX_TOKENS {
            tokens.push(Token::new(current));
        }
        (!tokens.is_empty()).then_some(Self { tokens })
    }
}

impl Token {
    fn new(chars: Vec<char>) -> Self {
        let ascii = chars
            .iter()
            .all(char::is_ascii)
            .then(|| chars.iter().map(|&c| c as u8).collect());
        Self { chars, ascii }
    }
}

/// How well a token matched, and where (in characters of the folded field).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    score: u32,
    field: Field,
    /// Ranges in characters of the folded field; empty unless asked for.
    ranges: Vec<[usize; 2]>,
}

/// Scratch space reused for every snippet.
#[derive(Default)]
struct Scratch {
    abbreviation: Vec<char>,
    label: Vec<char>,
    tag: Vec<char>,
    body: Vec<char>,
    rows: [Vec<u16>; 3],
}

fn fold_into(text: &str, out: &mut Vec<char>) {
    out.clear();
    out.extend(text.chars().flat_map(fold_char));
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// The first position at which `needle` occurs in `hay`.
fn find(hay: &[char], needle: &[char]) -> Option<usize> {
    let (&first, rest) = needle.split_first()?;
    let last_start = hay.len().checked_sub(needle.len())?;
    (0..=last_start).find(|&i| hay[i] == first && hay[i + 1..i + needle.len()] == *rest)
}

/// The first position at which `needle` occurs in `hay` at the start of a word, else the first
/// at all; and whether it was at a word start.
fn find_preferring_word_start(hay: &[char], needle: &[char]) -> Option<(usize, bool)> {
    let first = find(hay, needle)?;
    let mut at = first;
    loop {
        if at == 0 || !is_word_char(hay[at - 1]) {
            return Some((at, true));
        }
        match find(&hay[at + 1..], needle) {
            Some(next) => at += 1 + next,
            None => return Some((first, false)),
        }
    }
}

/// The characters of `needle` in order in `hay`, as the leftmost match: the span it covers and
/// the positions.
fn subsequence(hay: &[char], needle: &[char], positions: Option<&mut Vec<usize>>) -> Option<usize> {
    let mut at = 0;
    let mut first = 0;
    let mut last = 0;
    let mut found = Vec::new();
    for (k, &c) in needle.iter().enumerate() {
        let i = at + hay[at..].iter().position(|&h| h == c)?;
        if k == 0 {
            first = i;
        }
        last = i;
        found.push(i);
        at = i + 1;
    }
    if let Some(out) = positions {
        *out = found;
    }
    Some(last + 1 - first)
}

/// Positions merged into ranges.
fn runs(positions: &[usize]) -> Vec<[usize; 2]> {
    let mut out: Vec<[usize; 2]> = Vec::new();
    for &p in positions {
        match out.last_mut() {
            Some(last) if last[1] == p => last[1] = p + 1,
            _ => out.push([p, p + 1]),
        }
    }
    out
}

fn penalty(n: usize, cap: usize) -> u32 {
    u32::try_from(n.min(cap)).unwrap_or(0)
}

/// The restricted Damerau-Levenshtein distance between `a` and `b` when it is at most `max`.
fn osa(a: &[char], b: &[char], max: usize, rows: &mut [Vec<u16>; 3]) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let width = b.len() + 1;
    for row in rows.iter_mut() {
        row.clear();
        row.resize(width, 0);
    }
    let [two_back, back, cur] = rows;
    for (j, cell) in back.iter_mut().enumerate() {
        *cell = u16::try_from(j).unwrap_or(u16::MAX);
    }
    for i in 1..=a.len() {
        cur[0] = u16::try_from(i).unwrap_or(u16::MAX);
        let mut row_min = cur[0];
        for j in 1..=b.len() {
            let cost = u16::from(a[i - 1] != b[j - 1]);
            let mut best = (back[j] + 1).min(cur[j - 1] + 1).min(back[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(two_back[j - 2] + 1);
            }
            cur[j] = best;
            row_min = row_min.min(best);
        }
        if usize::from(row_min) > max {
            return None;
        }
        std::mem::swap(two_back, back);
        std::mem::swap(back, cur);
    }
    let d = usize::from(back[b.len()]);
    (d <= max).then_some(d)
}

/// How many edits a token of `len` characters is forgiven.
fn edits_for(len: usize) -> usize {
    if len >= FUZZY_TWO_EDITS {
        2
    } else {
        usize::from(len >= FUZZY_MIN)
    }
}

/// The distance from `tok` to `word` or to the start of it, within the token's allowance, and
/// the number of the word's characters it covers.
fn typo(tok: &[char], word: &[char], rows: &mut [Vec<u16>; 3]) -> Option<(usize, usize)> {
    let max = edits_for(tok.len());
    if max == 0 {
        return None;
    }
    let word = &word[..word.len().min(WORD_CAP)];
    let whole = osa(tok, word, max, rows).map(|d| (d, word.len()));
    // A mistyped start of a longer word: compare with as many characters as were typed.
    let start = (word.len() > tok.len())
        .then(|| osa(tok, &word[..tok.len()], max, rows).map(|d| (d, tok.len())))
        .flatten();
    match (whole, start) {
        (Some(w), Some(s)) => Some(if s.0 < w.0 { s } else { w }),
        (w, s) => w.or(s),
    }
}

/// The words of `text` (runs of letters and digits) as `(start, end)` character ranges.
fn words(text: &[char]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < text.len() && !is_word_char(text[i]) {
            i += 1;
        }
        if i >= text.len() {
            return None;
        }
        let start = i;
        while i < text.len() && is_word_char(text[i]) {
            i += 1;
        }
        Some((start, i))
    })
}

fn abbreviation_match(tok: &Token, abbr: &[char], want: bool) -> Option<Found> {
    let n = tok.chars.len();
    let found = |score: u32, ranges: Vec<[usize; 2]>| Found {
        score,
        field: Field::Abbreviation,
        ranges: if want { ranges } else { Vec::new() },
    };
    if abbr == tok.chars.as_slice() {
        return Some(found(1000, vec![[0, n]]));
    }
    if abbr.starts_with(&tok.chars) {
        return Some(found(900 - penalty(abbr.len() - n, 99), vec![[0, n]]));
    }
    if let Some(at) = find(abbr, &tok.chars) {
        return Some(found(800 - penalty(abbr.len() - n, 99), vec![[at, at + n]]));
    }
    if n >= 2 {
        let mut positions = Vec::new();
        if let Some(span) = subsequence(abbr, &tok.chars, want.then_some(&mut positions)) {
            return Some(found(700 - penalty(span - n, 99), runs(&positions)));
        }
    }
    None
}

fn label_match(tok: &Token, label: &[char], want: bool) -> Option<Found> {
    let n = tok.chars.len();
    let found = |score: u32, ranges: Vec<[usize; 2]>| Found {
        score,
        field: Field::Label,
        ranges: if want { ranges } else { Vec::new() },
    };
    if label.starts_with(&tok.chars) {
        return Some(found(650 - penalty(label.len() - n, 49), vec![[0, n]]));
    }
    if let Some((at, word_start)) = find_preferring_word_start(label, &tok.chars) {
        let base = if word_start { 590 } else { 540 };
        return Some(found(base - penalty(at, 49), vec![[at, at + n]]));
    }
    if n >= 3 {
        let mut positions = Vec::new();
        // Not scattered over the whole label: at most half as many characters again as typed.
        if let Some(span) = subsequence(label, &tok.chars, want.then_some(&mut positions))
            && span <= n + n / 2 + 1
        {
            return Some(found(500 - penalty(span - n, 49), runs(&positions)));
        }
    }
    None
}

/// The position of the first occurrence of `needle` in `hay`, ignoring ASCII case; `needle` is
/// lower case.
fn find_ascii(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let (&first, rest) = needle.split_first()?;
    let last_start = hay.len().checked_sub(needle.len())?;
    let upper = first.to_ascii_uppercase();
    let mut i = 0;
    while i <= last_start {
        // Skip to the next byte that could start the needle (a tight loop the compiler can
        // vectorise), then compare the rest.
        i += hay[i..=last_start]
            .iter()
            .position(|&b| b == first || b == upper)?;
        if hay[i + 1..i + needle.len()]
            .iter()
            .zip(rest)
            .all(|(h, n)| h.to_ascii_lowercase() == *n)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn body_score(at: usize, word_start: bool) -> u32 {
    let base = if word_start { 400 } else { 350 };
    base - penalty(at / 32, 49)
}

/// The content: an ASCII body is searched in place; any other is folded first. Positions are in
/// characters of the folded body (the same as bytes for an ASCII one).
fn body_match(tok: &Token, body: &str, scratch: &mut Scratch, want: bool) -> Option<Found> {
    let n = tok.chars.len();
    let found = |at: usize, word_start: bool| Found {
        score: body_score(at, word_start),
        field: Field::Body,
        ranges: if want { vec![[at, at + n]] } else { Vec::new() },
    };
    if body.is_ascii() {
        let needle = tok.ascii.as_deref()?;
        let bytes = body.as_bytes();
        let first = find_ascii(bytes, needle)?;
        let mut at = first;
        loop {
            if at == 0 || !bytes[at - 1].is_ascii_alphanumeric() {
                return Some(found(first, true));
            }
            match find_ascii(&bytes[at + 1..], needle) {
                Some(next) => at += 1 + next,
                None => return Some(found(first, false)),
            }
        }
    }
    fold_into(body, &mut scratch.body);
    find_preferring_word_start(&scratch.body, &tok.chars).map(|(at, ws)| found(at, ws))
}

fn tag_match(tok: &Token, name: &[char], want: bool) -> Option<Found> {
    let n = tok.chars.len();
    let (score, at) = if name == tok.chars.as_slice() {
        (300, 0)
    } else if name.starts_with(&tok.chars) {
        (280, 0)
    } else {
        (260, find(name, &tok.chars)?)
    };
    Some(Found {
        score,
        field: Field::Tag,
        ranges: if want { vec![[at, at + n]] } else { Vec::new() },
    })
}

/// A token's best match in `doc`, looking in the better tiers first (module docs).
fn token_match<'a>(
    tok: &Token,
    doc: &Doc<'_>,
    tag_name: &dyn Fn(Id) -> Option<&'a str>,
    scratch: &mut Scratch,
    want: bool,
) -> Option<Found> {
    if let Some(f) = abbreviation_match(tok, &scratch.abbreviation, want) {
        return Some(f);
    }
    if let Some(f) = label_match(tok, &scratch.label, want) {
        return Some(f);
    }
    if let Some(f) = body_match(tok, doc.body, scratch, want) {
        return Some(f);
    }
    let mut best: Option<Found> = None;
    for &id in doc.tags {
        let Some(name) = tag_name(id) else { continue };
        fold_into(name, &mut scratch.tag);
        if let Some(f) = tag_match(tok, &scratch.tag, want)
            && best.as_ref().is_none_or(|b| f.score > b.score)
        {
            best = Some(f);
        }
    }
    if best.is_some() {
        return best;
    }
    typo_match(tok, doc, tag_name, scratch, want)
}

fn typo_score(base: u32, edits: usize) -> u32 {
    base - 30 * u32::try_from(edits.saturating_sub(1)).unwrap_or(0)
}

/// The last tier: a token forgiven a typo against the abbreviation, a word of the label or a
/// word of a tag.
fn typo_match<'a>(
    tok: &Token,
    doc: &Doc<'_>,
    tag_name: &dyn Fn(Id) -> Option<&'a str>,
    scratch: &mut Scratch,
    want: bool,
) -> Option<Found> {
    if edits_for(tok.chars.len()) == 0 {
        return None;
    }
    let found = |score: u32, field: Field, start: usize, len: usize| Found {
        score,
        field,
        ranges: if want {
            vec![[start, start + len]]
        } else {
            Vec::new()
        },
    };
    if let Some((d, len)) = typo(&tok.chars, &scratch.abbreviation, &mut scratch.rows) {
        return Some(found(typo_score(190, d), Field::Abbreviation, 0, len));
    }
    let label = std::mem::take(&mut scratch.label);
    let mut best = None;
    for (start, end) in words(&label) {
        if let Some((d, len)) = typo(&tok.chars, &label[start..end], &mut scratch.rows) {
            let f = found(typo_score(150, d), Field::Label, start, len);
            if best.as_ref().is_none_or(|b: &Found| f.score > b.score) {
                best = Some(f);
            }
        }
    }
    scratch.label = label;
    if best.is_some() {
        return best;
    }
    for &id in doc.tags {
        let Some(name) = tag_name(id) else { continue };
        fold_into(name, &mut scratch.tag);
        let tag = std::mem::take(&mut scratch.tag);
        for (start, end) in words(&tag) {
            if let Some((d, len)) = typo(&tok.chars, &tag[start..end], &mut scratch.rows) {
                let f = found(typo_score(110, d), Field::Tag, start, len);
                if best.as_ref().is_none_or(|b: &Found| f.score > b.score) {
                    best = Some(f);
                }
            }
        }
        scratch.tag = tag;
    }
    best
}

/// All of `query`'s tokens against `doc`: the mean score, and the field and ranges of the best
/// token. `None` when a token matched nowhere.
fn doc_match<'a>(
    query: &Query,
    doc: &Doc<'_>,
    tag_name: &dyn Fn(Id) -> Option<&'a str>,
    scratch: &mut Scratch,
    want: bool,
) -> Option<Found> {
    fold_into(doc.abbreviation, &mut scratch.abbreviation);
    fold_into(doc.label, &mut scratch.label);
    let mut total = 0_u32;
    let mut best: Option<Found> = None;
    let mut others: Vec<Found> = Vec::new();
    for tok in &query.tokens {
        let f = token_match(tok, doc, tag_name, scratch, want)?;
        total += f.score;
        match &mut best {
            Some(b) if f.score > b.score => others.push(std::mem::replace(b, f)),
            Some(_) => others.push(f),
            None => best = Some(f),
        }
    }
    let mut best = best?;
    best.score = total / u32::try_from(query.tokens.len()).unwrap_or(1);
    if want {
        for o in others.into_iter().filter(|o| o.field == best.field) {
            best.ranges.extend(o.ranges);
        }
        best.ranges.sort_unstable();
        // Merge ranges that touch or overlap.
        let mut merged: Vec<[usize; 2]> = Vec::new();
        for r in best.ranges.drain(..) {
            match merged.last_mut() {
                Some(last) if r[0] <= last[1] => last[1] = last[1].max(r[1]),
                _ => merged.push(r),
            }
        }
        best.ranges = merged;
    }
    Some(best)
}

/// `ranges` in characters of the folded `text`, as ranges in UTF-16 code units of `text`
/// itself (folding can turn one character into several, so each folded character maps back to
/// the character it came from).
#[must_use]
pub fn utf16_ranges(text: &str, ranges: &[[usize; 2]]) -> Vec<[usize; 2]> {
    if ranges.is_empty() {
        return Vec::new();
    }
    let mut owner: Vec<[usize; 2]> = Vec::new();
    let mut at = 0;
    for c in text.chars() {
        let end = at + c.len_utf16();
        owner.extend(fold_char(c).map(|_| [at, end]));
        at = end;
    }
    let mut out: Vec<[usize; 2]> = Vec::new();
    for r in ranges {
        if r[0] >= r[1] || r[1] > owner.len() {
            continue;
        }
        let range = [owner[r[0]][0], owner[r[1] - 1][1]];
        match out.last_mut() {
            Some(last) if range[0] <= last[1] => last[1] = last[1].max(range[1]),
            _ => out.push(range),
        }
    }
    out
}

/// The text of the field a hit matched, for its ranges ([`utf16_ranges`]).
fn field_text<'a>(
    doc: &Doc<'a>,
    field: Field,
    tag_name: &dyn Fn(Id) -> Option<&'a str>,
    query: &Query,
    scratch: &mut Scratch,
) -> Option<&'a str> {
    match field {
        Field::Abbreviation => Some(doc.abbreviation),
        Field::Label => Some(doc.label),
        Field::Body => Some(doc.body),
        Field::Tag => {
            // The tag whose name the best match came from: the first that matches again.
            doc.tags.iter().filter_map(|&id| tag_name(id)).find(|name| {
                fold_into(name, &mut scratch.tag);
                let name_chars = std::mem::take(&mut scratch.tag);
                let hit = query.tokens.iter().any(|t| {
                    tag_match(t, &name_chars, false).is_some()
                        || words(&name_chars).any(|(s, e)| {
                            typo(&t.chars, &name_chars[s..e], &mut scratch.rows).is_some()
                        })
                });
                scratch.tag = name_chars;
                hit
            })
        }
    }
}

/// The best `limit` snippets of `docs` for `query`, best first. `tag_name` gives a tag's name.
///
/// Empty for a query without anything to look for.
pub fn search<'a>(
    query: &str,
    limit: usize,
    docs: impl IntoIterator<Item = Doc<'a>>,
    tag_name: &dyn Fn(Id) -> Option<&'a str>,
) -> Vec<Hit> {
    let Some(query) = Query::new(query) else {
        return Vec::new();
    };
    if limit == 0 {
        return Vec::new();
    }
    let mut scratch = Scratch::default();
    let mut found: Vec<(u32, usize, Doc<'a>)> = Vec::new();
    for (seq, doc) in docs.into_iter().enumerate() {
        if let Some(f) = doc_match(&query, &doc, tag_name, &mut scratch, false) {
            found.push((f.score, seq, doc));
        }
    }
    let order = |a: &(u32, usize, Doc<'a>), b: &(u32, usize, Doc<'a>)| {
        (Reverse(a.0), a.1).cmp(&(Reverse(b.0), b.1))
    };
    if found.len() > limit {
        found.select_nth_unstable_by(limit - 1, order);
        found.truncate(limit);
    }
    found.sort_unstable_by(order);
    found
        .into_iter()
        .filter_map(|(_, _, doc)| {
            // Again, for the ranges of the few that are returned.
            let f = doc_match(&query, &doc, tag_name, &mut scratch, true)?;
            let ranges = field_text(&doc, f.field, tag_name, &query, &mut scratch)
                .map(|text| utf16_ranges(text, &f.ranges))
                .unwrap_or_default();
            Some(Hit {
                id: doc.id,
                score: f.score,
                field: f.field,
                ranges,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> Id {
        Id::from_bytes([n; 16])
    }

    fn names(_: Id) -> Option<&'static str> {
        None
    }

    fn doc<'a>(n: u8, abbreviation: &'a str, label: &'a str, body: &'a str) -> Doc<'a> {
        Doc {
            id: id(n),
            abbreviation,
            label,
            body,
            tags: &[],
        }
    }

    fn top(query: &str, docs: &[Doc<'static>]) -> Vec<u8> {
        search(query, 10, docs.iter().copied(), &names)
            .iter()
            .map(|h| h.id.as_bytes()[0])
            .collect()
    }

    #[test]
    fn a_query_without_letters_finds_nothing() {
        let docs = [doc(1, "brb", "", "be right back")];
        assert!(top("", &docs).is_empty());
        assert!(top("  \t ", &docs).is_empty());
        assert!(search("brb", 0, docs.iter().copied(), &names).is_empty());
    }

    #[test]
    fn the_tiers_rank_abbreviation_label_content_tag() {
        let tags = [id(90)];
        let tag_names = |t: Id| (t == id(90)).then_some("greeting");
        let docs = [
            Doc {
                tags: &tags,
                ..doc(1, ";x1", "", "")
            },
            doc(2, ";x2", "", "say greeting twice"),
            doc(3, ";x3", "A greeting", ""),
            doc(4, ";greeting", "", ""),
        ];
        let hits = search("greeting", 10, docs.iter().copied(), &tag_names);
        let order: Vec<(u8, Field)> = hits.iter().map(|h| (h.id.as_bytes()[0], h.field)).collect();
        assert_eq!(
            order,
            [
                (4, Field::Abbreviation),
                (3, Field::Label),
                (2, Field::Body),
                (1, Field::Tag)
            ]
        );
    }

    #[test]
    fn an_exact_abbreviation_beats_a_prefix_which_beats_a_substring() {
        let docs = [
            doc(1, ";xretx", "", ""),
            doc(2, ";retail", "", ""),
            doc(3, ";ret", "", ""),
        ];
        assert_eq!(
            top(";ret", &docs),
            [3, 2, 1],
            "the last by its letters in order"
        );
        assert_eq!(
            top("ret", &docs),
            [3, 1, 2],
            "the fewest other characters first"
        );
    }

    #[test]
    fn letters_in_order_find_an_abbreviation() {
        let docs = [doc(1, ";rret", "", ""), doc(2, ";other", "", "")];
        let hits = search("rt", 5, docs.iter().copied(), &names);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].field, Field::Abbreviation);
        assert_eq!(hits[0].ranges, [[1, 2], [4, 5]]);
    }

    #[test]
    fn a_typo_is_forgiven_last() {
        let docs = [
            doc(1, ";a", "Return label", ""),
            doc(2, ";b", "", "return the label"),
        ];
        // Swapped letters: only the label word is one edit away; the content does not match.
        assert_eq!(top("retrun", &docs), [1]);
        // Too short to forgive anything.
        assert!(top("rtq", &docs).is_empty());
        // The exact content match wins over the forgiven label.
        let hits = search("return", 5, docs.iter().copied(), &names);
        assert_eq!(hits[0].id, id(1), "the label starts with it");
    }

    #[test]
    fn two_edits_are_forgiven_from_eight_characters() {
        let docs = [doc(1, ";a", "Workshop address", "")];
        assert!(top("worksohp", &docs).contains(&1), "a swap is one edit");
        assert!(top("workshap adress", &docs).contains(&1));
        assert!(top("wxrkshxp", &docs).contains(&1), "two edits");
        assert!(top("wxrkshxpq", &docs).is_empty(), "three");
    }

    #[test]
    fn every_token_must_match() {
        let docs = [
            doc(1, ";a", "Late delivery", "Hi {field name=Name}"),
            doc(2, ";b", "Late reply", ""),
        ];
        assert_eq!(top("late deliv", &docs), [1]);
        assert_eq!(top("deliv late", &docs), [1]);
        assert!(top("late warranty", &docs).is_empty());
    }

    #[test]
    fn ties_keep_the_order_of_the_input() {
        let docs = [
            doc(3, ";aa", "", ""),
            doc(1, ";ab", "", ""),
            doc(2, ";ac", "", ""),
        ];
        assert_eq!(top("a", &docs), [3, 1, 2]);
        assert_eq!(top("a", &docs), top("a", &docs));
    }

    #[test]
    fn ranges_are_in_utf16_code_units_of_the_original_text() {
        // A surrogate pair (two code units) before the match, and a letter that folds to two.
        let docs = [doc(1, ";a", "😀 Résumé Straße", "")];
        let hits = search("sum", 5, docs.iter().copied(), &names);
        assert_eq!(hits[0].field, Field::Label);
        let label = "😀 Résumé Straße";
        let r = hits[0].ranges[0];
        let units: Vec<u16> = label.encode_utf16().collect();
        assert_eq!(String::from_utf16_lossy(&units[r[0]..r[1]]), "sum");
        let hits = search("ß", 5, docs.iter().copied(), &names);
        let r = hits[0].ranges[0];
        assert_eq!(String::from_utf16_lossy(&units[r[0]..r[1]]), "ß");
    }

    #[test]
    fn a_body_that_is_not_ascii_is_searched_too() {
        let docs = [doc(1, ";a", "", "Grüße aus Köln, bis später")];
        let hits = search("KÖLN", 5, docs.iter().copied(), &names);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].field, Field::Body);
        assert_eq!(hits[0].ranges, [[10, 14]]);
    }

    #[test]
    fn a_word_start_in_the_content_beats_the_middle_of_a_word() {
        let docs = [
            doc(1, ";a", "", "nonrefundable parts"),
            doc(2, ";b", "", "a refund follows"),
        ];
        assert_eq!(top("refund", &docs), [2, 1]);
    }

    #[test]
    fn only_the_best_limit_come_back_in_order() {
        let docs: Vec<Doc<'static>> = (1..=30_u8)
            .map(|n| doc(n, "x", "", if n % 2 == 0 { "match" } else { "unmatched" }))
            .collect();
        let hits = search("match", 5, docs.iter().copied(), &names);
        let got: Vec<u8> = hits.iter().map(|h| h.id.as_bytes()[0]).collect();
        assert_eq!(got, [2, 4, 6, 8, 10], "word starts first, then input order");
    }

    #[test]
    fn a_long_query_is_cut() {
        let q = "a ".repeat(40);
        let parsed = Query::new(&q).expect("tokens");
        assert_eq!(parsed.tokens.len(), MAX_TOKENS);
        let long = "b".repeat(200);
        assert_eq!(
            Query::new(&long).expect("token").tokens[0].chars.len(),
            MAX_QUERY_CHARS
        );
    }

    #[test]
    fn the_distance_counts_swaps_as_one() {
        let mut rows: [Vec<u16>; 3] = Default::default();
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(osa(&c("return"), &c("retrun"), 2, &mut rows), Some(1));
        assert_eq!(osa(&c("return"), &c("returns"), 2, &mut rows), Some(1));
        assert_eq!(osa(&c("return"), &c("rtrn"), 2, &mut rows), Some(2));
        assert_eq!(osa(&c("return"), &c("rtn"), 2, &mut rows), None);
        assert_eq!(osa(&c("abc"), &c("abc"), 0, &mut rows), Some(0));
    }
}
