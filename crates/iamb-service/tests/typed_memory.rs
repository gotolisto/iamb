//! What stays in the worker's memory after typing (architecture §8 rule 1; the 2.8 review's
//! findings 3 and 18). Linux only: the test reads its own process's memory through
//! `/proc/self/mem`, which needs no `unsafe`.
//!
//! The worker runs in this process on the mock platform. A canary of 16 different characters
//! is typed into a password field, one key at a time, so that the test itself never holds it
//! as an array. Every writable mapping is then searched for the canary's characters as 4-byte
//! code points at a fixed distance from each other (8 to 512 bytes): the shape a queue of
//! events leaves behind, one character per slot. A plain character array (distance 4) is not
//! looked at, because the mock's own text field is one.

#![cfg(target_os = "linux")]

mod common;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use common::{Harness, LIBRARY};
use iamb_ipc::{RequestBody, ResponseBody, Role};
use iamb_platform::mock::{MockKey, MockWindow};

/// The canary: Georgian capital letters, which nothing else in this process holds.
fn canary() -> impl Iterator<Item = char> {
    (0x10A0..0x10B0).filter_map(char::from_u32)
}

const CANARY_LEN: usize = 16;

/// The writable mappings of this process.
fn writable_regions() -> Vec<(usize, usize)> {
    let maps = std::fs::read_to_string("/proc/self/maps").expect("maps");
    maps.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let (range, perms) = (parts.next()?, parts.next()?);
            let name = parts.nth(3).unwrap_or("");
            if !perms.starts_with("rw") || matches!(name, "[vvar]" | "[vsyscall]" | "[vdso]") {
                return None;
            }
            let (lo, hi) = range.split_once('-')?;
            Some((
                usize::from_str_radix(lo, 16).ok()?,
                usize::from_str_radix(hi, 16).ok()?,
            ))
        })
        .collect()
}

/// The longest run of consecutive canary characters found in writable memory at one distance
/// from 8 to 512 bytes, with that distance.
fn longest_spaced_run(units: &[[u8; 4]]) -> (usize, usize) {
    const CHUNK: usize = 16 << 20;
    const OVERLAP: usize = CANARY_LEN * 512 + 8;
    // One buffer for the whole scan; its own bytes are skipped, nothing else.
    let mut buf = vec![0_u8; CHUNK];
    let own = (buf.as_ptr().addr(), buf.as_ptr().addr() + buf.len());
    let mut mem = File::open("/proc/self/mem").expect("own memory");
    let mut best = (0, 0);
    for (lo, hi) in writable_regions() {
        // The region without the scan buffer: up to two pieces.
        let pieces = if own.0 < hi && lo < own.1 {
            vec![(lo, own.0.max(lo)), (own.1.min(hi), hi)]
        } else {
            vec![(lo, hi)]
        };
        for (lo, hi) in pieces {
            let mut base = lo;
            while base < hi {
                let len = (hi - base).min(CHUNK);
                let data = &mut buf[..len];
                let read = mem
                    .seek(SeekFrom::Start(base as u64))
                    .and_then(|_| mem.read_exact(data));
                if read.is_ok() {
                    scan(data, units, &mut best);
                }
                if base + len >= hi {
                    break;
                }
                base += len - OVERLAP;
            }
        }
    }
    best
}

fn scan(data: &[u8], units: &[[u8; 4]], best: &mut (usize, usize)) {
    for start in 0..units.len() - 2 {
        let first = units[start];
        let mut p = 0;
        while p + 4 <= data.len() {
            if data[p..p + 4] == first {
                for stride in (8..=512).step_by(4) {
                    let mut run = 1;
                    while start + run < units.len() {
                        let q = p + run * stride;
                        if q + 4 > data.len() || data[q..q + 4] != units[start + run] {
                            break;
                        }
                        run += 1;
                    }
                    if run > best.0 {
                        *best = (run, stride);
                    }
                }
            }
            p += 4;
        }
    }
}

#[test]
fn characters_typed_into_a_password_field_do_not_stay_in_memory() {
    let units: Vec<[u8; 4]> = canary().map(|c| u32::from(c).to_le_bytes()).collect();
    assert_eq!(units.len(), CANARY_LEN);
    let before = longest_spaced_run(&units);
    assert!(
        before.0 < 2,
        "the canary is in memory before it was typed: {before:?}"
    );

    let h = Harness::start(LIBRARY);
    let bank =
        h.os.open_window(MockWindow::new("org.example.bank", 200).secure());
    h.os.focus(bank).expect("window is open");
    h.worker().sync();
    for ch in canary() {
        h.os.play(std::iter::once(MockKey::Char(ch)));
    }
    h.settle();
    h.worker().sync();
    assert_eq!(
        h.text(bank).chars().count(),
        CANARY_LEN,
        "the field got every key"
    );
    let mut c = h.client(Role::Ui);
    match c.call(RequestBody::GetStatus {}).expect("status") {
        ResponseBody::Status { status } => {
            assert!(status.suspended, "nothing typed there is stored");
            assert!(status.memory_locked);
        }
        other => panic!("unexpected {other:?}"),
    }
    drop(c);

    // Before the fix of finding 3 the engine channel kept all 16, one every 96 bytes.
    let (run, stride) = longest_spaced_run(&units);
    assert!(
        run < 4,
        "{run} of {CANARY_LEN} typed characters stay in memory, one every {stride} bytes"
    );
    // The locked regions (key queue, keystroke buffer, hand-off) hold nothing of it either.
    let mut mem = File::open("/proc/self/mem").expect("own memory");
    for region in h.os.locked_regions() {
        let mut bytes = vec![0_u8; region.len()];
        mem.seek(SeekFrom::Start(region.addr() as u64))
            .and_then(|_| mem.read_exact(&mut bytes))
            .expect("readable");
        assert!(
            bytes.chunks_exact(4).all(|w| !units.iter().any(|u| u == w)),
            "a locked region of {} bytes still holds a typed character",
            region.len()
        );
    }
    h.stop();
}
