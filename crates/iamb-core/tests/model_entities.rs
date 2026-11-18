//! Serialization, spec-coverage and defaults tests for the data model.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use iamb_core::model::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn id(n: u64) -> Id {
    Id::v7(
        1_700_000_000_000 + n,
        [u8::try_from(n % 256).unwrap_or(0); 10],
    )
}

fn full_meta(n: u64) -> SyncMeta {
    SyncMeta {
        id: id(n),
        created_at: Timestamp(1_700_000_000_000),
        updated_at: Timestamp(1_700_000_001_000),
        updated_by: Some(id(900)),
        rev: 42,
        deleted_at: Some(Timestamp(1_700_000_002_000)),
    }
}

fn min_meta(n: u64) -> SyncMeta {
    SyncMeta::new(id(n), Timestamp(5))
}

fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(v: &T) -> Value {
    let json = serde_json::to_value(v).unwrap();
    let back: T = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(&back, v);
    let text = serde_json::to_string(v).unwrap();
    assert_eq!(&serde_json::from_str::<T>(&text).unwrap(), v);
    json
}

fn abbr(s: &str) -> Abbreviation {
    Abbreviation::new(s).unwrap()
}

const COMMON: [&str; 6] = [
    "id",
    "created_at",
    "updated_at",
    "updated_by",
    "rev",
    "deleted_at",
];

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn check_spec(v: &Value, spec: &[&str]) -> (usize, usize) {
    let expected: BTreeSet<String> = COMMON
        .iter()
        .chain(spec.iter())
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(keys(v), expected);
    (expected.len(), expected.len())
}

fn profile(meta: SyncMeta) -> Profile {
    Profile {
        meta,
        kind: ProfileKind::Family,
        name: "Home".into(),
        owner_user_id: id(1),
    }
}
fn membership(meta: SyncMeta) -> Membership {
    Membership {
        meta,
        profile_id: id(2),
        user_id: id(3),
        role: Role::Admin,
    }
}
fn folder_full(meta: SyncMeta) -> Folder {
    Folder {
        meta,
        profile_id: id(2),
        parent_id: Some(id(4)),
        name: "Work".into(),
        position: -3,
        member_permission: Some(Permission::Edit),
        defaults: FolderDefaults {
            trigger: Some(Trigger::Immediate),
            case_mode: Some(CaseMode::Adaptive),
            abbreviation_prefix: Some(";".into()),
        },
    }
}
fn folder_min(meta: SyncMeta) -> Folder {
    Folder {
        meta,
        profile_id: id(2),
        parent_id: None,
        name: "W".into(),
        position: 0,
        member_permission: None,
        defaults: FolderDefaults::default(),
    }
}
fn share(meta: SyncMeta) -> FolderShare {
    FolderShare {
        meta,
        folder_id: id(4),
        profile_id: id(2),
        permission: Permission::View,
    }
}
fn snippet(meta: SyncMeta) -> Snippet {
    Snippet {
        meta,
        folder_id: id(4),
        abbreviation: abbr("ÄBC😀"),
        label: "Greeting".into(),
        kind: SnippetKind::Rich,
        body: "Hello {cursor}".into(),
        trigger: Trigger::Immediate,
        case_mode: CaseMode::Exact,
        word_start: false,
        enabled: false,
    }
}
fn tag(meta: SyncMeta, color: Option<&str>) -> Tag {
    Tag {
        meta,
        profile_id: id(2),
        name: "urgent".into(),
        color: color.map(Into::into),
    }
}
fn snippet_tag(meta: SyncMeta) -> SnippetTag {
    SnippetTag {
        meta,
        snippet_id: id(5),
        tag_id: id(6),
    }
}
fn attachment(meta: SyncMeta) -> Attachment {
    Attachment {
        meta,
        profile_id: id(2),
        sha256: Sha256::from_bytes([0xab; 32]),
        mime: "image/png".into(),
        size: u64::MAX,
        file_name: "logo.png".into(),
    }
}
fn snippet_attachment(meta: SyncMeta) -> SnippetAttachment {
    SnippetAttachment {
        meta,
        snippet_id: id(5),
        attachment_id: id(7),
    }
}

#[test]
fn roundtrip_every_entity_full_and_minimal() {
    for meta in [full_meta(10), min_meta(11)] {
        roundtrip(&meta);
        roundtrip(&profile(meta.clone()));
        roundtrip(&membership(meta.clone()));
        roundtrip(&folder_full(meta.clone()));
        roundtrip(&folder_min(meta.clone()));
        roundtrip(&share(meta.clone()));
        roundtrip(&snippet(meta.clone()));
        roundtrip(&tag(meta.clone(), Some("#ff0000")));
        roundtrip(&tag(meta.clone(), None));
        roundtrip(&snippet_tag(meta.clone()));
        roundtrip(&attachment(meta.clone()));
        roundtrip(&snippet_attachment(meta));
    }
}

#[test]
fn spec_field_coverage_is_complete() {
    // Field lists copied from docs/architecture.md section 7 ("Fields beyond the common ones").
    let m = full_meta(20);
    let mut covered = 0;
    let mut total = 0;
    let mut add = |(c, t): (usize, usize)| {
        covered += c;
        total += t;
    };
    add(check_spec(
        &roundtrip(&profile(m.clone())),
        &["kind", "name", "owner_user_id"],
    ));
    add(check_spec(
        &roundtrip(&membership(m.clone())),
        &["profile_id", "user_id", "role"],
    ));
    let f = roundtrip(&folder_full(m.clone()));
    add(check_spec(
        &f,
        &[
            "profile_id",
            "parent_id",
            "name",
            "position",
            "member_permission",
            "defaults",
        ],
    ));
    let defaults_keys = keys(&f["defaults"]);
    let want: BTreeSet<String> = ["trigger", "case_mode", "abbreviation_prefix"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(defaults_keys, want);
    add(check_spec(
        &roundtrip(&share(m.clone())),
        &["folder_id", "profile_id", "permission"],
    ));
    add(check_spec(
        &roundtrip(&snippet(m.clone())),
        &[
            "folder_id",
            "abbreviation",
            "label",
            "kind",
            "body",
            "trigger",
            "case_mode",
            "word_start",
            "enabled",
        ],
    ));
    add(check_spec(
        &roundtrip(&tag(m.clone(), Some("red"))),
        &["profile_id", "name", "color"],
    ));
    add(check_spec(
        &roundtrip(&snippet_tag(m.clone())),
        &["snippet_id", "tag_id"],
    ));
    add(check_spec(
        &roundtrip(&attachment(m.clone())),
        &["profile_id", "sha256", "mime", "size", "file_name"],
    ));
    add(check_spec(
        &roundtrip(&snippet_attachment(m)),
        &["snippet_id", "attachment_id"],
    ));
    assert_eq!(covered, total);
}

#[test]
fn json_is_flat_with_expected_encodings() {
    let v = serde_json::to_value(attachment(full_meta(30))).unwrap();
    assert_eq!(v["sha256"], "ab".repeat(32));
    assert_eq!(v["created_at"], 1_700_000_000_000_i64);
    assert_eq!(v["rev"], 42);
    assert_eq!(v["id"].as_str().unwrap().len(), 36);
    let s = serde_json::to_value(snippet(min_meta(31))).unwrap();
    assert_eq!(s["kind"], "rich");
    assert_eq!(s["trigger"], "immediate");
    assert_eq!(s["case_mode"], "exact");
    assert_eq!(s["updated_by"], Value::Null);
    assert_eq!(s["abbreviation"], "ÄBC😀");
}

#[test]
fn enums_are_snake_case_and_reject_unknown() {
    assert_eq!(
        serde_json::to_value(ProfileKind::Company).unwrap(),
        json!("company")
    );
    assert_eq!(serde_json::to_value(Role::Owner).unwrap(), json!("owner"));
    assert_eq!(
        serde_json::to_value(CaseMode::Insensitive).unwrap(),
        json!("insensitive")
    );
    assert!(serde_json::from_value::<ProfileKind>(json!("enterprise")).is_err());
    assert!(serde_json::from_value::<Role>(json!("Owner")).is_err());
    assert!(serde_json::from_value::<Permission>(json!("write")).is_err());
    assert!(serde_json::from_value::<Trigger>(json!("")).is_err());
    assert!(serde_json::from_value::<CaseMode>(json!("loose")).is_err());
    assert!(serde_json::from_value::<SnippetKind>(json!("markdown")).is_err());

    let mut v = serde_json::to_value(snippet(min_meta(32))).unwrap();
    v["trigger"] = json!("sometimes");
    assert!(serde_json::from_value::<Snippet>(v).is_err());
}

#[test]
fn invalid_abbreviation_rejected_on_deserialize() {
    for bad in ["", "a b", "a\tb", "a\u{0}b", &"x".repeat(65)] {
        let mut v = serde_json::to_value(snippet(min_meta(33))).unwrap();
        v["abbreviation"] = json!(bad);
        assert!(serde_json::from_value::<Snippet>(v).is_err(), "{bad:?}");
    }
}

#[test]
fn bad_id_and_sha_rejected_on_deserialize() {
    let mut v = serde_json::to_value(attachment(min_meta(34))).unwrap();
    v["sha256"] = json!("zz");
    assert!(serde_json::from_value::<Attachment>(v.clone()).is_err());
    v["sha256"] = json!("00".repeat(32));
    v["id"] = json!("not-an-id");
    assert!(serde_json::from_value::<Attachment>(v).is_err());
}

#[test]
fn is_deleted_helper() {
    assert!(!min_meta(1).is_deleted());
    assert!(full_meta(1).is_deleted());
}

#[test]
fn new_defaults_word_start_follows_trigger() {
    let d = Snippet::new_defaults(min_meta(40), id(4), abbr("sig"), "Sig", "Bye", None);
    assert_eq!(d.trigger, Trigger::Delimiter);
    assert!(d.word_start);
    assert!(d.enabled);
    assert_eq!(d.kind, SnippetKind::Plain);
    assert_eq!(d.case_mode, CaseMode::Insensitive);

    let fd = FolderDefaults {
        trigger: Some(Trigger::Immediate),
        case_mode: Some(CaseMode::Adaptive),
        abbreviation_prefix: Some(";".into()),
    };
    let s = Snippet::new_defaults(min_meta(41), id(4), abbr(";sig"), "Sig", "Bye", Some(&fd));
    assert_eq!(s.trigger, Trigger::Immediate);
    assert_eq!(s.case_mode, CaseMode::Adaptive);
    assert!(!s.word_start);

    let only_case = FolderDefaults {
        case_mode: Some(CaseMode::Exact),
        ..FolderDefaults::default()
    };
    let s = Snippet::new_defaults(min_meta(42), id(4), abbr("x"), "", "", Some(&only_case));
    assert_eq!(s.trigger, Trigger::Delimiter);
    assert_eq!(s.case_mode, CaseMode::Exact);
}

#[test]
fn folder_prefix_applies_and_validates() {
    let fd = FolderDefaults {
        abbreviation_prefix: Some(";".into()),
        ..FolderDefaults::default()
    };
    assert_eq!(fd.prefixed_abbreviation("sig").unwrap().as_str(), ";sig");
    assert!(fd.prefixed_abbreviation("").is_ok()); // prefix alone is a valid abbreviation
    assert_eq!(
        FolderDefaults::default()
            .prefixed_abbreviation("")
            .unwrap_err(),
        ModelError::AbbreviationEmpty
    );
    let long = FolderDefaults {
        abbreviation_prefix: Some("p".repeat(64)),
        ..FolderDefaults::default()
    };
    assert!(long.prefixed_abbreviation("x").is_err());
}
