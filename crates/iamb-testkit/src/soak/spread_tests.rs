//! Tests of [`super::spread`], kept out of its file because `iamb-service`'s log test compiles
//! that file by path.

use std::fmt::Write as _;

use super::rng::SplitMix64;
use super::spread::*;

const CANARY: &[u8] = b"qzkqjetdphptplmw";

fn strided_hits(spread: &Spread, hay: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    spread.strided(hay, hay.len(), &mut |c, p, s| out.push((c, p, s)));
    out
}

fn line_hits(spread: &Spread, hay: &[u8], chunk: usize) -> Vec<(usize, u64)> {
    let mut out = Vec::new();
    let mut lines = spread.lines();
    for part in hay.chunks(chunk.max(1)) {
        lines.feed(part, &mut |c, o| out.push((c, o)));
    }
    lines.finish(&mut |c, o| out.push((c, o)));
    out
}

/// Lower-case letters from a fixed generator, as a stand-in for ordinary text.
fn letters(n: usize, seed: u64) -> Vec<u8> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| b'a' + u8::try_from(rng.next_u64() % 26).unwrap_or(0))
        .collect()
}

#[test]
fn utf32_records_and_char_debug_are_strided_matches() {
    let spread = Spread::new(&[CANARY]);
    let little: Vec<u8> = CANARY.iter().flat_map(|&b| [b, 0, 0, 0]).collect();
    let big: Vec<u8> = CANARY.iter().flat_map(|&b| [0, 0, 0, b]).collect();
    let wide: Vec<u8> = CANARY.iter().flat_map(|&b| [0, b]).collect();
    let debug = format!(
        "{:?}",
        CANARY.iter().map(|&b| char::from(b)).collect::<Vec<_>>()
    );
    let mut records = vec![0_u8; 96 * 20];
    for (k, &b) in CANARY.iter().enumerate() {
        records[96 * (k + 2) + 44] = b;
    }
    for (name, hay, stride) in [
        ("utf32le", little, 4),
        ("utf32be", big, 4),
        ("utf16be", wide, 2),
        ("char debug", debug.into_bytes(), 5),
        ("96-byte records", records, 96),
    ] {
        let hits = strided_hits(&spread, &hay);
        assert!(
            hits.iter().any(|&(c, _, s)| c == 0 && s == stride),
            "{name}: {hits:?}"
        );
    }
}

#[test]
fn one_key_per_log_line_is_a_line_chain() {
    let spread = Spread::new(&[CANARY]);
    let mut log = String::new();
    log.push_str("2026-11-18T22:35:55.076Z INFO worker[4103]: worker ready\n");
    for (k, &b) in CANARY.iter().enumerate() {
        // The sequence number changes the line's width from 9 to 10.
        let _ = writeln!(
            log,
            "2026-11-18T22:35:55.{:03}Z INFO worker[4103]: key '{}' seq {}",
            k * 7,
            char::from(b),
            k + 4
        );
        if k == 7 {
            // Other lines in between.
            log.push_str(
                "2026-11-18T22:35:55.140Z INFO worker[4103]: ipc in get_status 55 bytes\n",
            );
            log.push_str(
                "2026-11-18T22:35:55.140Z INFO worker[4103]: ipc out status 243 bytes 48 us\n",
            );
        }
    }
    assert!(
        !log.contains("qzkqjetdphptplmw"),
        "no whole canary in the file"
    );
    for chunk in [1, 7, 4096] {
        let hits = line_hits(&spread, log.as_bytes(), chunk);
        assert_eq!(hits.len(), 1, "chunk {chunk}: {hits:?}");
        assert_eq!(hits[0].0, 0);
        // The offset is that of the line with the first character.
        let first = log.find("key 'q'").expect("first key");
        let line_start = log[..first].rfind('\n').map_or(0, |n| n + 1);
        assert_eq!(hits[0].1, u64::try_from(line_start).expect("fits"));
    }
    // Without the last character the chain is incomplete.
    let cut = log.rfind("key 'w'").expect("last key");
    assert!(line_hits(&spread, &log.as_bytes()[..cut], 4096).is_empty());
}

#[test]
fn a_chain_ends_at_a_wrong_character_of_its_template_or_a_long_gap() {
    let spread = Spread::new(&[CANARY]);
    let line = |b: u8| format!("x: key '{}'\n", char::from(b));
    // A wrong key in the middle.
    let mut log = String::new();
    for (k, &b) in CANARY.iter().enumerate() {
        log.push_str(&line(if k == 8 { b'a' } else { b }));
    }
    assert!(line_hits(&spread, log.as_bytes(), 64).is_empty());
    // Too many lines of other templates in between.
    let mut log = String::new();
    for (k, &b) in CANARY.iter().enumerate() {
        log.push_str(&line(b));
        if k == 3 {
            for _ in 0..=MAX_GAP_LINES {
                log.push_str("something else\n");
            }
        }
    }
    assert!(line_hits(&spread, log.as_bytes(), 64).is_empty());
}

#[test]
fn random_text_and_random_bytes_do_not_trigger_either_detector() {
    // Every canary starts with `qz`, the way the soak draws them.
    let canaries: Vec<Vec<u8>> = (0..15)
        .map(|i| {
            let mut c = b"qz".to_vec();
            c.extend(letters(14, 1000 + i));
            c
        })
        .collect();
    let spread = Spread::new(&canaries);
    let text = letters(1 << 20, 7);
    assert!(strided_hits(&spread, &text).is_empty());
    // Lines of random words, and lines that repeat a template with one varying letter.
    let mut lines = Vec::new();
    for (k, chunk) in text.chunks(40).enumerate() {
        lines.extend_from_slice(chunk);
        lines.push(b'\n');
        lines.extend_from_slice(format!("x {k}: key '").as_bytes());
        lines.push(chunk[0]);
        lines.extend_from_slice(b"'\n");
    }
    assert!(line_hits(&spread, &lines, 1 << 16).is_empty());
    let mut rng = SplitMix64::new(99);
    let bytes: Vec<u8> = (0..1 << 20)
        .map(|_| rng.next_u64().to_le_bytes()[3])
        .collect();
    assert!(strided_hits(&spread, &bytes).is_empty());
    assert!(line_hits(&spread, &bytes, 1 << 16).is_empty());
}

#[test]
fn short_canaries_are_left_out() {
    let spread = Spread::new(&[b"qzab".as_slice()]);
    assert!(strided_hits(&spread, b"q.z.a.b.").is_empty());
    assert_eq!(spread.span(), 1);
}
