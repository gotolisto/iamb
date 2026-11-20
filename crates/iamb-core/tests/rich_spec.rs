//! The rich-content examples of `spec/rich/examples.toml` (task 3.8): Markdown to the HTML the
//! clipboard carries and the plain-text fallback, hostile input included.

use iamb_core::rich;
use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    example: Vec<Example>,
}

#[derive(Deserialize)]
struct Example {
    name: String,
    markdown: String,
    html: String,
    plain: String,
    #[serde(default)]
    attachments: Vec<String>,
}

#[test]
fn every_example_renders_as_written() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/rich/examples.toml");
    let text = std::fs::read_to_string(path).expect("examples file");
    let file: File = toml::from_str(&text).expect("examples parse");
    assert!(
        file.example.len() >= 20,
        "only {} examples",
        file.example.len()
    );
    let mut failures = Vec::new();
    for e in &file.example {
        let out = rich::render(&e.markdown).expect("renders");
        let attachments: Vec<String> = out.attachments.iter().map(ToString::to_string).collect();
        if out.html != e.html || out.plain != e.plain || attachments != e.attachments {
            failures.push(format!(
                "--- {}\n html:  {:?}\n want:  {:?}\n plain: {:?}\n want:  {:?}\n attachments: {attachments:?}",
                e.name, out.html, e.html, out.plain, e.plain
            ));
        }
        assert_eq!(
            rich::to_plain(&e.markdown).expect("renders"),
            out.plain,
            "{}",
            e.name
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
