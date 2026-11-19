//! The Templates group (architecture §6.2, ADR-0017): `ParseTemplate` and `RenderPreview`,
//! answered from the snapshot with `iamb-core`'s parser and renderer, the same code that
//! expands, so the editor never needs a second build of the core.
//!
//! - **Tokens** are what the editor shows as text and chips: each macro with its attributes,
//!   with UTF-16 offsets into the source (`iamb_ipc::messages`, "Positions in text").
//! - **Errors** carry their code and a span: from the offending position to the end of the
//!   macro it is in. A `{snippet}` is resolved in the snapshot's reference set, so a missing or
//!   looping one is reported as expansion would report it.
//! - **The preview** renders with the service's clock and the snapshot's date, time and locale
//!   settings. A text or area field without a value and without a default shows its name, as a
//!   sample (`sample: true`); the clipboard is never read: it shows the sample `Clipboard`.
//!
//! # Privacy
//!
//! The source is library content (or what the user is writing into the library); the values
//! are user-entered and travel only in the request and the answer. Nothing here logs: the
//! service's log records the message names only (§6.1).

use iamb_core::datetime::LocalDateTime;
use iamb_core::engine::Snapshot;
use iamb_core::template::{
    FieldKind, FormValues, Origin, RenderContext, RenderedAction, Template, TemplateError, Token,
    tokens,
};
use iamb_ipc::{
    PreviewBlank, PreviewCursor, PreviewPart, TemplateAttribute, TemplateErrorInfo, TemplateToken,
    WireField, WireValues,
};

/// What the preview shows for clipboard content.
pub const CLIPBOARD_SAMPLE: &str = "Clipboard";

/// Converts `char` offsets of one text into UTF-16 offsets, for increasing or decreasing
/// calls alike (each call walks from the start: texts here are templates).
struct Utf16<'a>(&'a str);

impl Utf16<'_> {
    fn at(&self, chars: usize) -> usize {
        self.0.chars().take(chars).map(char::len_utf16).sum()
    }
}

/// The UTF-16 offset of byte offset `byte` of `text`.
fn utf16_of_byte(text: &str, byte: usize) -> usize {
    text.get(..byte)
        .map_or(0, |head| head.encode_utf16().count())
}

/// The tokens of `source` on the wire.
fn wire_tokens(source: &str, list: &[Token]) -> Vec<TemplateToken> {
    let at = Utf16(source);
    list.iter()
        .map(|token| match token {
            Token::Text { range } => TemplateToken {
                kind: "text".to_owned(),
                start: at.at(range.start),
                end: at.at(range.end),
                attributes: Vec::new(),
            },
            Token::Macro {
                name,
                attributes,
                range,
            } => TemplateToken {
                kind: name.as_str().to_owned(),
                start: at.at(range.start),
                end: at.at(range.end),
                attributes: attributes
                    .iter()
                    .map(|a| TemplateAttribute {
                        name: a.name.to_owned(),
                        value: a.value.clone(),
                    })
                    .collect(),
            },
        })
        .collect()
}

/// `error` on the wire: its position in `source` (none when it is in an included snippet's
/// body or found only by rendering), extended to the end of the macro that holds it.
fn wire_error(source: &str, list: &[Token], error: &TemplateError) -> TemplateErrorInfo {
    let position = error.position().filter(|_| error.snippet().is_none());
    let at = Utf16(source);
    let (start, end) = match position {
        Some(p) => {
            let end = list
                .iter()
                .find_map(|t| match t {
                    Token::Macro { range, .. } if range.start <= p && p < range.end => {
                        Some(range.end)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    // No macro holds it (an unclosed macro, which has no tokens): to the end
                    // of the source for `E_UNCLOSED_MACRO`, else one character.
                    if error.code() == "E_UNCLOSED_MACRO" {
                        source.chars().count()
                    } else {
                        (p + 1).min(source.chars().count())
                    }
                });
            (Some(at.at(p)), Some(at.at(end.max(p))))
        }
        None => (None, None),
    };
    TemplateErrorInfo {
        code: error.code().to_owned(),
        start,
        end,
        snippet: error.snippet().map(str::to_owned),
    }
}

/// `ParseTemplate`: the tokens and the first parse or resolve error, against `snapshot`.
#[must_use]
pub fn parse(snapshot: &Snapshot, source: &str) -> (Vec<TemplateToken>, Option<TemplateErrorInfo>) {
    let list = tokens(source);
    let error = Template::parse(source)
        .and_then(|t| t.resolve(snapshot).map(|_| ()))
        .err()
        .map(|e| wire_error(source, &list, &e));
    (wire_tokens(source, &list), error)
}

/// What `RenderPreview` answers.
#[derive(Debug, Default)]
pub struct PreviewAnswer {
    /// The parts.
    pub parts: Vec<PreviewPart>,
    /// The cursor.
    pub cursor: Option<PreviewCursor>,
    /// The fields.
    pub fields: Vec<WireField>,
    /// The error.
    pub error: Option<TemplateErrorInfo>,
}

/// `RenderPreview`: `source` rendered against `snapshot` at `now` with `values` (module docs).
#[must_use]
pub fn preview(
    snapshot: &Snapshot,
    source: &str,
    values: &WireValues,
    now: LocalDateTime,
) -> PreviewAnswer {
    let list = tokens(source);
    let fail = |e: &TemplateError| PreviewAnswer {
        error: Some(wire_error(source, &list, e)),
        ..PreviewAnswer::default()
    };
    let resolved = match Template::parse(source).and_then(|t| t.resolve(snapshot)) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    let form = resolved.form();
    let mut given = FormValues::new();
    let mut sample = vec![false; form.len()];
    for (i, field) in form.iter().enumerate() {
        match values.get(&field.name) {
            Some(v) => {
                given.insert(field.name.clone(), v);
            }
            None if field.default.is_empty()
                && matches!(field.kind, FieldKind::Text | FieldKind::Area) =>
            {
                given.insert(field.name.clone(), &field.name);
                sample[i] = true;
            }
            None => {}
        }
    }
    let settings = snapshot.settings();
    let ctx = RenderContext {
        now,
        locale: settings.locale,
        date_format: &settings.date_format,
        time_format: &settings.time_format,
        clipboard: Some(CLIPBOARD_SAMPLE),
    };
    let rendered = match resolved.render_preview(&ctx, &given) {
        Ok(p) => p,
        Err(e) => {
            let mut answer = fail(&e);
            answer.fields = form.iter().map(WireField::from).collect();
            return answer;
        }
    };
    let actions = &rendered.rendered.actions;
    let parts = actions
        .iter()
        .enumerate()
        .map(|(i, action)| match action {
            RenderedAction::Text { text, .. } => {
                let mut blanks = Vec::new();
                let mut clipboard = Vec::new();
                for o in rendered.origins.iter().filter(|o| o.action == i) {
                    let (start, end) = (
                        utf16_of_byte(text, o.range.start),
                        utf16_of_byte(text, o.range.end),
                    );
                    match o.origin {
                        Origin::Field(f) => blanks.push(PreviewBlank {
                            start,
                            end,
                            field: form.get(f).map(|s| s.name.clone()).unwrap_or_default(),
                            sample: sample.get(f).copied().unwrap_or(false),
                        }),
                        Origin::Clipboard => clipboard.push([start, end]),
                    }
                }
                PreviewPart::Text {
                    text: text.clone(),
                    blanks,
                    clipboard,
                }
            }
            RenderedAction::Key { key, shift, times } => PreviewPart::Key {
                key: key.as_str().to_owned(),
                shift: *shift,
                times: *times,
            },
            RenderedAction::Wait { ms } => PreviewPart::Wait { ms: *ms },
        })
        .collect();
    let cursor = rendered.cursor.map(|(part, byte)| PreviewCursor {
        part,
        offset: match actions.get(part) {
            Some(RenderedAction::Text { text, .. }) => utf16_of_byte(text, byte),
            _ => 0,
        },
    });
    PreviewAnswer {
        parts,
        cursor,
        fields: form.iter().map(WireField::from).collect(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use iamb_core::engine::EngineSettings;
    use iamb_core::model::{Abbreviation, Id, Snippet, SyncMeta, Timestamp};

    use super::*;

    fn now() -> LocalDateTime {
        LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("date")
    }

    fn library() -> Snapshot {
        let sig = Snippet::new_defaults(
            SyncMeta::new(Id::from_bytes([1; 16]), Timestamp(1)),
            Id::from_bytes([9; 16]),
            Abbreviation::new(";sig").expect("abbreviation"),
            "",
            "Kind regards",
            None,
        );
        Snapshot::build([sig], EngineSettings::default())
    }

    #[test]
    fn tokens_carry_attributes_and_utf16_offsets() {
        let (tokens, error) = parse(&library(), "é😀 {field name=\"Ünï\" default=x}{cursor}");
        assert_eq!(error, None);
        assert_eq!(tokens.len(), 3);
        // "é" is one UTF-16 unit, the emoji two, the space one.
        assert_eq!(
            (tokens[0].kind.as_str(), tokens[0].start, tokens[0].end),
            ("text", 0, 4)
        );
        assert_eq!(tokens[1].kind, "field");
        assert_eq!(tokens[1].start, 4);
        assert_eq!(
            tokens[1].attributes,
            vec![
                TemplateAttribute {
                    name: "name".to_owned(),
                    value: "Ünï".to_owned()
                },
                TemplateAttribute {
                    name: "default".to_owned(),
                    value: "x".to_owned()
                },
            ]
        );
        assert_eq!(tokens[2].kind, "cursor");
        assert_eq!(tokens[1].end, tokens[2].start);
    }

    #[test]
    fn errors_have_a_code_and_a_span_and_snippets_are_looked_up() {
        let (tokens, error) = parse(&library(), "Hi {date fmt=\"%Q\"} {snippet abbr=\";sig\"}");
        assert_eq!(tokens.len(), 4, "the chips are there despite the error");
        let error = error.expect("an error");
        assert_eq!(error.code, "E_BAD_FORMAT");
        // From the value to the end of the macro.
        assert_eq!((error.start, error.end), (Some(13), Some(18)));

        let (_, error) = parse(&library(), "{snippet abbr=\";nope\"}");
        let error = error.expect("an error");
        assert_eq!(error.code, "E_SNIPPET_NOT_FOUND");
        assert_eq!((error.start, error.end), (Some(0), Some(22)));
        assert_eq!(parse(&library(), "{snippet abbr=\";sig\"}").1, None);

        let (tokens, error) = parse(&library(), "ab {date");
        assert!(tokens.is_empty());
        let error = error.expect("an error");
        assert_eq!(error.code, "E_UNCLOSED_MACRO");
        assert_eq!((error.start, error.end), (Some(3), Some(8)));
    }

    #[test]
    fn a_preview_has_parts_blanks_samples_the_cursor_and_the_fields() {
        let source = "Hi {field name=Name}, {clipboard}{key name=tab times=2}{wait ms=500}\
                      {field name=Size type=choice options=\"S|M\" default=M}{cursor}!";
        let values = BTreeMap::new();
        let answer = preview(&library(), source, &values, now());
        assert_eq!(answer.error, None);
        assert_eq!(answer.fields.len(), 2);
        assert_eq!(answer.parts.len(), 4);
        let PreviewPart::Text {
            text,
            blanks,
            clipboard,
        } = &answer.parts[0]
        else {
            panic!("text first");
        };
        assert_eq!(text, "Hi Name, Clipboard");
        assert_eq!(
            blanks,
            &[PreviewBlank {
                start: 3,
                end: 7,
                field: "Name".to_owned(),
                sample: true
            }]
        );
        assert_eq!(clipboard, &[[9, 18]]);
        assert_eq!(
            answer.parts[1],
            PreviewPart::Key {
                key: "tab".to_owned(),
                shift: false,
                times: 2
            }
        );
        assert_eq!(answer.parts[2], PreviewPart::Wait { ms: 500 });
        // The choice's default, then the cursor before the "!".
        assert!(matches!(&answer.parts[3], PreviewPart::Text { text, .. } if text == "M!"));
        assert_eq!(answer.cursor, Some(PreviewCursor { part: 3, offset: 1 }));

        // A given value is not a sample, and its range follows its length in UTF-16.
        let values = BTreeMap::from([("Name".to_owned(), "Zoë 😀".to_owned())]);
        let answer = preview(&library(), "Hi {field name=Name}.", &values, now());
        let PreviewPart::Text { text, blanks, .. } = &answer.parts[0] else {
            panic!("text");
        };
        assert_eq!(text, "Hi Zoë 😀.");
        assert_eq!(
            (blanks[0].start, blanks[0].end, blanks[0].sample),
            (3, 9, false)
        );
    }

    #[test]
    fn a_preview_of_a_broken_template_is_its_error() {
        let answer = preview(
            &library(),
            "{if field=X}never closed",
            &BTreeMap::new(),
            now(),
        );
        assert!(answer.parts.is_empty());
        assert_eq!(
            answer.error.map(|e| e.code),
            Some("E_UNBALANCED_IF".to_owned())
        );
        // A render-time error has no position, and keeps the fields.
        let answer = preview(
            &library(),
            "{field name=A}{date shift=\"+9000y\"}",
            &BTreeMap::new(),
            now(),
        );
        let error = answer.error.expect("an error");
        assert_eq!(error.code, "E_DATE_RANGE");
        assert_eq!(error.start, None);
        assert_eq!(answer.fields.len(), 1);
    }
}
