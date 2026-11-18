//! The clipboard's HTML format ("HTML Format", often called `CF_HTML`).
//!
//! The data is UTF-8: a header of `Name:value` lines, then an HTML document in which two
//! comments mark the fragment. The header gives byte offsets, from the start of the data, of
//! the document (`StartHTML`, `EndHTML`) and of the fragment between the two comments
//! (`StartFragment`, `EndFragment`). Readers slice by these offsets, so they must be exact.
//! The offsets are written with ten digits each, so the header's length does not depend on
//! them and can be computed first.

/// The registered name of the format.
pub(crate) const FORMAT_NAME: &str = "HTML Format";

const START_COMMENT: &str = "<!--StartFragment-->";
const END_COMMENT: &str = "<!--EndFragment-->";
const DOC_BEFORE: &str = "<html><body>\r\n";
const DOC_AFTER: &str = "\r\n</body></html>";

/// The four offsets of a `CF_HTML` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Offsets {
    pub(crate) start_html: usize,
    pub(crate) end_html: usize,
    pub(crate) start_fragment: usize,
    pub(crate) end_fragment: usize,
}

fn header(offsets: &Offsets) -> String {
    format!(
        "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n",
        offsets.start_html, offsets.end_html, offsets.start_fragment, offsets.end_fragment
    )
}

/// The clipboard data for an HTML fragment, followed by one NUL byte (not counted by the
/// offsets; readers that treat the data as a C string need it).
pub(crate) fn encode(fragment: &str) -> Vec<u8> {
    let zero = Offsets {
        start_html: 0,
        end_html: 0,
        start_fragment: 0,
        end_fragment: 0,
    };
    let header_len = header(&zero).len();
    let start_fragment = header_len + DOC_BEFORE.len() + START_COMMENT.len();
    let end_fragment = start_fragment + fragment.len();
    let offsets = Offsets {
        start_html: header_len,
        end_html: end_fragment + END_COMMENT.len() + DOC_AFTER.len(),
        start_fragment,
        end_fragment,
    };
    let mut out = header(&offsets).into_bytes();
    debug_assert_eq!(out.len(), header_len);
    out.reserve(offsets.end_html - header_len + 1);
    out.extend_from_slice(DOC_BEFORE.as_bytes());
    out.extend_from_slice(START_COMMENT.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(END_COMMENT.as_bytes());
    out.extend_from_slice(DOC_AFTER.as_bytes());
    debug_assert_eq!(out.len(), offsets.end_html);
    out.push(0);
    out
}

/// The offsets in a `CF_HTML` header, if it has all four and they are consistent with the
/// data's length.
#[cfg(test)]
pub(crate) fn offsets(data: &[u8]) -> Option<Offsets> {
    let field = |name: &str| -> Option<usize> {
        let key = format!("{name}:");
        let at = data.windows(key.len()).position(|w| w == key.as_bytes())?;
        let digits: Vec<u8> = data[at + key.len()..]
            .iter()
            .copied()
            .take_while(u8::is_ascii_digit)
            .collect();
        std::str::from_utf8(&digits).ok()?.parse().ok()
    };
    let offsets = Offsets {
        start_html: field("StartHTML")?,
        end_html: field("EndHTML")?,
        start_fragment: field("StartFragment")?,
        end_fragment: field("EndFragment")?,
    };
    let ordered = offsets.start_html <= offsets.start_fragment
        && offsets.start_fragment <= offsets.end_fragment
        && offsets.end_fragment <= offsets.end_html
        && offsets.end_html <= data.len();
    ordered.then_some(offsets)
}

/// The fragment of `CF_HTML` data.
#[cfg(test)]
pub(crate) fn fragment(data: &[u8]) -> Option<&str> {
    let o = offsets(data)?;
    std::str::from_utf8(&data[o.start_fragment..o.end_fragment]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_point_at_the_document_and_the_fragment() {
        for fragment in [
            "",
            "<b>bold</b>",
            "<p>café 👍🏽 漢字</p><ul><li>one</li></ul>",
            "<img src=\"data:image/png;base64,iVBORw0KGgo=\">",
        ] {
            let data = encode(fragment);
            assert_eq!(data.last(), Some(&0));
            let o = offsets(&data).expect("a valid header");
            let header = std::str::from_utf8(&data[..o.start_html]).expect("ASCII");
            assert!(header.starts_with("Version:0.9\r\n"));
            assert!(header.ends_with("\r\n"));
            assert_eq!(&data[o.start_html..o.start_html + 6], b"<html>");
            assert_eq!(o.end_html, data.len() - 1);
            assert!(data[..o.end_html].ends_with(b"</html>"));
            assert_eq!(&data[o.start_fragment..o.end_fragment], fragment.as_bytes());
            assert!(data[..o.start_fragment].ends_with(START_COMMENT.as_bytes()));
            assert!(data[o.end_fragment..].starts_with(END_COMMENT.as_bytes()));
            assert_eq!(super::fragment(&data), Some(fragment));
        }
    }

    #[test]
    fn the_header_has_a_fixed_layout() {
        let data = encode("<i>x</i>");
        let text = std::str::from_utf8(&data[..data.len() - 1]).expect("UTF-8");
        let lines: Vec<&str> = text.split("\r\n").take(5).collect();
        assert_eq!(lines[0], "Version:0.9");
        assert_eq!(lines[1], "StartHTML:0000000105");
        assert!(lines[2].starts_with("EndHTML:") && lines[2].len() == "EndHTML:".len() + 10);
        assert_eq!(lines[3], "StartFragment:0000000139");
        assert_eq!(lines[4], "EndFragment:0000000147");
    }

    #[test]
    fn offsets_are_bytes_not_characters() {
        let data = encode("é");
        let o = offsets(&data).expect("valid");
        assert_eq!(o.end_fragment - o.start_fragment, 2);
    }

    #[test]
    fn broken_headers_are_refused() {
        assert_eq!(offsets(b""), None);
        assert_eq!(offsets(b"Version:0.9\r\nStartHTML:10\r\n"), None);
        assert_eq!(
            offsets(b"StartHTML:0\r\nEndHTML:999\r\nStartFragment:1\r\nEndFragment:2\r\n"),
            None
        );
        assert_eq!(
            offsets(b"StartHTML:0\r\nEndHTML:60\r\nStartFragment:9\r\nEndFragment:2\r\n"),
            None
        );
        let mut data = encode("<b>x</b>");
        data.truncate(50);
        assert_eq!(offsets(&data), None);
    }
}
