//! Shared helpers for the store's integration tests.

#![allow(dead_code)]

use iamb_core::model::{
    Abbreviation, Attachment, Folder, FolderDefaults, FolderShare, Id, Membership, Permission,
    Profile, ProfileKind, Role, Sha256, Snippet, SnippetAttachment, SnippetTag, SyncMeta, Tag,
    Timestamp,
};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A fixed "now" for tests.
pub const T0: Timestamp = Timestamp(1_763_000_000_000);

/// `T0` plus `ms` milliseconds.
pub const fn t(ms: i64) -> Timestamp {
    Timestamp(T0.0 + ms)
}

/// Deterministic, unique UUID version 7 ids.
#[derive(Debug, Default)]
pub struct Ids(u64);

impl Ids {
    pub fn next(&mut self) -> Id {
        self.0 += 1;
        id_from(self.0)
    }
}

/// The id numbered `n` (distinct `n` give distinct ids).
pub fn id_from(n: u64) -> Id {
    let b = n.to_be_bytes();
    Id::v7(
        1_700_000_000_000 + n,
        [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], 0x5a, 0xa5],
    )
}

pub fn abbr(s: &str) -> Abbreviation {
    Abbreviation::new(s).unwrap_or_else(|e| panic!("bad test abbreviation {s:?}: {e}"))
}

pub fn profile(id: Id, owner: Id) -> Profile {
    Profile {
        meta: SyncMeta::new(id, T0),
        kind: ProfileKind::Personal,
        name: "Personal".to_owned(),
        owner_user_id: owner,
    }
}

pub fn group(id: Id, owner: Id) -> Profile {
    Profile {
        meta: SyncMeta::new(id, T0),
        kind: ProfileKind::Company,
        name: "Acme".to_owned(),
        owner_user_id: owner,
    }
}

pub fn membership(id: Id, profile_id: Id, user_id: Id) -> Membership {
    Membership {
        meta: SyncMeta::new(id, T0),
        profile_id,
        user_id,
        role: Role::Member,
    }
}

pub fn folder(id: Id, profile_id: Id, parent_id: Option<Id>) -> Folder {
    Folder {
        meta: SyncMeta::new(id, T0),
        profile_id,
        parent_id,
        name: format!("Folder {id}"),
        position: 0,
        member_permission: None,
        defaults: FolderDefaults::default(),
    }
}

pub fn folder_share(id: Id, folder_id: Id, profile_id: Id) -> FolderShare {
    FolderShare {
        meta: SyncMeta::new(id, T0),
        folder_id,
        profile_id,
        permission: Permission::View,
    }
}

pub fn snippet(id: Id, folder_id: Id, abbreviation: &str) -> Snippet {
    Snippet::new_defaults(
        SyncMeta::new(id, T0),
        folder_id,
        abbr(abbreviation),
        format!("Label {abbreviation}"),
        format!("Body of {abbreviation}"),
        None,
    )
}

pub fn tag(id: Id, profile_id: Id, name: &str) -> Tag {
    Tag {
        meta: SyncMeta::new(id, T0),
        profile_id,
        name: name.to_owned(),
        color: Some("#aabbcc".to_owned()),
    }
}

pub fn snippet_tag(id: Id, snippet_id: Id, tag_id: Id) -> SnippetTag {
    SnippetTag {
        meta: SyncMeta::new(id, T0),
        snippet_id,
        tag_id,
    }
}

pub fn attachment(id: Id, profile_id: Id) -> Attachment {
    Attachment {
        meta: SyncMeta::new(id, T0),
        profile_id,
        sha256: Sha256::from_bytes([0xab; 32]),
        mime: "image/png".to_owned(),
        size: 1234,
        file_name: "logo.png".to_owned(),
    }
}

pub fn snippet_attachment(id: Id, snippet_id: Id, attachment_id: Id) -> SnippetAttachment {
    SnippetAttachment {
        meta: SyncMeta::new(id, T0),
        snippet_id,
        attachment_id,
    }
}

/// A fresh, empty temporary directory, removed when the guard drops.
pub fn temp_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("iamb-store-test-")
        .tempdir()
        .unwrap_or_else(|e| panic!("cannot create temp dir: {e}"))
}
