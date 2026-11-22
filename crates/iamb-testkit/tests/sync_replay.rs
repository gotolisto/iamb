//! The scenario replay of task 4.4: every scenario of `spec/sync/scenarios.toml` runs through the
//! real store (`iamb_store::Store`), the real agent (`iamb_sync::Agent`) and the model of the
//! server (`iamb_sync::model::Server`), and the state it ends in is compared with the scenario's
//! stated outcome: every row on the server (live, deleted or absent, and the step that wrote its
//! revision), the history, the conflicts (row, kind, winner, loser, the versions taken over,
//! state), and on every device every row, the kept and blocked changes and the outbox.
//!
//! What this derives that `sync_scenarios.rs` takes on trust: the answers, the rows a write takes
//! with it, and which rows a user may see. What it does not compare (listed in the report): what
//! a device shows (`shown`), which snippet holds an abbreviation (`expands`), reviews, broken
//! references and the permissions a device displays; invitations, which are not rows; and the
//! fields an outcome states beyond the version.
//!
//! The replay runs the steps as the scenario format says (`crates/iamb-testkit/src/
//! sync_scenarios.rs`, "How a replay reads it"): an online device syncs until nothing is left
//! after each step, in the order of the device ids; `offline`, `online`, `manual` and `auto`
//! control that; a push answered `lost` or `held` reaches the server and its answer is dropped.
//!
//! `IAMB_SYNC_REPLAY=S15` replays one scenario and prints what it found.

#![allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::needless_borrows_for_generic_args,
    clippy::map_unwrap_or,
    clippy::single_match_else,
    clippy::match_like_matches_macro,
    clippy::semicolon_if_nothing_returned,
    clippy::unused_self,
    clippy::uninlined_format_args,
    clippy::single_match,
    reason = "a test harness"
)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use iamb_core::model::{
    Abbreviation, Attachment, Folder, FolderDefaults, FolderShare, Id, Membership, Permission,
    Profile, ProfileKind, Role, Sha256, Snippet, SnippetAttachment, SnippetKind, SnippetTag,
    SyncMeta, Tag, Timestamp,
};
use iamb_core::sync::{AnyRow, RowKind, RowRef};
use iamb_store::Store;
use iamb_sync::api::{CallError, Transport};
use iamb_sync::model::{Caller, Server, SharedTransport, StoreDevice, Stored};
use iamb_sync::{Agent, AgentConfig};
use iamb_testkit::sync_scenarios::{Scenario, load, scenarios_path};
use serde_json::{Value, json};

const T0: i64 = 1_795_000_000_000;
const DAY: i64 = 86_400_000;

struct LinkState {
    online: bool,
    drop_answer: u32,
}

struct Link {
    inner: SharedTransport,
    state: Rc<RefCell<LinkState>>,
    /// The session of the tokens the installation holds (a sign-in replaces it).
    session: Rc<Cell<Id>>,
}

impl Transport for Link {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError> {
        if !self.state.borrow().online {
            return Err(CallError::Offline);
        }
        self.inner.caller.session = self.session.get();
        let answer = self.inner.call(function, args)?;
        let mut s = self.state.borrow_mut();
        if s.drop_answer > 0 && function == "push" {
            s.drop_answer -= 1;
            return Err(CallError::Network("answer lost".into()));
        }
        Ok(answer)
    }
}

struct Dev {
    name: String,
    user: Id,
    agent: Agent<Link, StoreDevice>,
    link: Rc<RefCell<LinkState>>,
    clock: Rc<Cell<i64>>,
    auto: bool,
    signed_in: bool,
    /// The version name of each row as the device holds it.
    version: BTreeMap<RowRef, String>,
    device_id: Option<Id>,
    session: Rc<Cell<Id>>,
    /// The data directory of a store on disk (a device whose directory is copied).
    dir: Option<tempfile::TempDir>,
}

struct Replay<'a> {
    sc: &'a Scenario,
    server: Rc<RefCell<Server>>,
    devs: Vec<Dev>,
    ids: BTreeMap<String, Id>,
    kinds: BTreeMap<String, RowKind>,
    users: BTreeMap<String, (Id, Id, String)>,
    profiles: BTreeMap<String, Id>,
    names: BTreeMap<(RowRef, u64), String>,
    ops: BTreeMap<Id, String>,
    processed_seen: usize,
    backups: BTreeMap<String, i64>,
    removals: BTreeMap<(Id, Id), String>,
    restored: bool,
    counter: u64,
    notes: Vec<String>,
}

/// Copies a data directory as a migration tool would (the `-shm` index is rebuilt on open).
fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let path = entry.path();
        if path.is_dir() {
            std::fs::create_dir_all(to.join(&name)).map_err(|e| e.to_string())?;
            copy_dir(&path, &to.join(&name))?;
        } else if !name.to_string_lossy().ends_with("-shm") {
            std::fs::copy(&path, to.join(&name)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn sha_of(text: &str) -> Sha256 {
    let mut b = [0u8; 32];
    for (i, c) in text.bytes().enumerate() {
        b[i % 32] ^= c.wrapping_mul(31).wrapping_add(i as u8);
    }
    Sha256::from_bytes(b)
}

/// A body such as `(500 kB of text)` stands for that much text.
fn body_of(b: Option<String>) -> Option<String> {
    let b = b?;
    if let Some(n) = b
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(" kB of text)"))
        .and_then(|n| n.parse::<usize>().ok())
    {
        return Some("x".repeat(n * 1_000));
    }
    Some(b)
}

fn s(v: Option<&toml::Value>) -> Option<String> {
    v.and_then(|v| v.as_str().map(str::to_owned))
}

impl<'a> Replay<'a> {
    fn new(sc: &'a Scenario) -> Self {
        Self {
            sc,
            server: Rc::new(RefCell::new(Server::new(T0, 7))),
            devs: Vec::new(),
            ids: BTreeMap::new(),
            kinds: BTreeMap::new(),
            users: BTreeMap::new(),
            profiles: BTreeMap::new(),
            names: BTreeMap::new(),
            ops: BTreeMap::new(),
            processed_seen: 0,
            backups: BTreeMap::new(),
            removals: BTreeMap::new(),
            restored: false,
            counter: 0,
            notes: Vec::new(),
        }
    }

    fn new_id(&mut self) -> Id {
        self.counter += 1;
        let mut b = [0u8; 10];
        b[..8].copy_from_slice(&self.counter.to_be_bytes());
        b[9] = 0x5a;
        Id::v7(T0 as u64, b)
    }

    fn id(&mut self, name: &str) -> Id {
        if let Some(i) = self.ids.get(name) {
            return *i;
        }
        let i = self.new_id();
        self.ids.insert(name.to_owned(), i);
        i
    }

    fn key(&mut self, name: &str) -> Option<RowRef> {
        let kind = *self.kinds.get(name)?;
        Some(RowRef {
            kind,
            id: self.id(name),
        })
    }

    fn profile_of_name(&self, name: &str) -> Id {
        self.profiles
            .get(name)
            .copied()
            .unwrap_or_else(|| Id::from_bytes([0; 16]))
    }

    /// Builds a row of `entity` from a scenario's references and fields.
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        entity: &str,
        name: &str,
        refs: &iamb_testkit::sync_scenarios::Refs<'_>,
        fields: &BTreeMap<String, toml::Value>,
        by: Option<Id>,
        now: i64,
    ) -> Option<AnyRow> {
        let id = self.id(name);
        let meta = SyncMeta {
            updated_by: by,
            ..SyncMeta::new(id, Timestamp(now))
        };
        let f = |k: &str| s(fields.get(k));
        Some(match entity {
            "folder" => AnyRow::Folder(Folder {
                meta,
                profile_id: self.profile_of_name(refs.profile?),
                parent_id: refs.parent.map(|p| self.id(p)),
                name: f("name").unwrap_or_else(|| name.to_owned()),
                position: fields
                    .get("position")
                    .and_then(toml::Value::as_integer)
                    .unwrap_or(0),
                member_permission: f("member_permission").map(|p| {
                    if p == "edit" {
                        Permission::Edit
                    } else {
                        Permission::View
                    }
                }),
                defaults: FolderDefaults::default(),
            }),
            "snippet" => {
                let abbr = f("abbreviation").unwrap_or_else(|| format!(";{name}"));
                let mut sn = Snippet::new_defaults(
                    meta,
                    self.id(refs.folder?),
                    Abbreviation::new(&abbr).ok()?,
                    &f("label").unwrap_or_default(),
                    &body_of(f("body")).unwrap_or_else(|| format!("{name} body")),
                    None,
                );
                if f("kind").as_deref() == Some("rich") {
                    sn.kind = SnippetKind::Rich;
                }
                AnyRow::Snippet(sn)
            }
            "tag" => AnyRow::Tag(Tag {
                meta,
                profile_id: self.profile_of_name(refs.profile?),
                name: f("name").unwrap_or_else(|| name.to_owned()),
                color: f("color"),
            }),
            "snippet_tag" => AnyRow::SnippetTag(SnippetTag {
                meta,
                snippet_id: self.id(refs.snippet?),
                tag_id: self.id(refs.tag?),
            }),
            "attachment" => AnyRow::Attachment(Attachment {
                meta,
                profile_id: self.profile_of_name(refs.profile?),
                sha256: sha_of(&f("sha256").unwrap_or_else(|| name.to_owned())),
                mime: f("mime").unwrap_or_else(|| "image/png".to_owned()),
                size: 10,
                file_name: format!("{name}.png"),
            }),
            "snippet_attachment" => AnyRow::SnippetAttachment(SnippetAttachment {
                meta,
                snippet_id: self.id(refs.snippet?),
                attachment_id: self.id(refs.attachment?),
            }),
            "folder_share" => AnyRow::FolderShare(FolderShare {
                meta,
                folder_id: self.id(refs.folder?),
                profile_id: self.profile_of_name(refs.group?),
                permission: if f("permission").as_deref() == Some("view") {
                    Permission::View
                } else {
                    Permission::Edit
                },
            }),
            "membership" => AnyRow::Membership(Membership {
                meta,
                profile_id: self.profile_of_name(refs.group?),
                user_id: self.users.get(refs.user?)?.0,
                role: match f("role").as_deref() {
                    Some("owner") => Role::Owner,
                    Some("admin") => Role::Admin,
                    _ => Role::Member,
                },
            }),
            _ => return None,
        })
    }

    fn setup(&mut self) -> Result<(), String> {
        let sc = self.sc;
        for u in &sc.start.users {
            let email = format!("{u}@example.com");
            let (id, personal, session) = self.server.borrow_mut().add_user(&email);
            self.users.insert(u.clone(), (id, personal, email));
            let _ = session;
            self.ids.insert(u.clone(), id);
        }
        for p in &sc.start.profiles {
            if p.local_to.is_some() {
                return Err("a device with a local library before its first sign-in (D9)".into());
            }
            let owner = self.users.get(&p.owner).ok_or("unknown owner")?.clone();
            if p.kind == "personal" {
                self.profiles.insert(p.id.clone(), owner.1);
                self.names.insert(
                    (
                        RowRef {
                            kind: RowKind::Profile,
                            id: owner.1,
                        },
                        1,
                    ),
                    "start".into(),
                );
                continue;
            }
            let gid = self.id(&p.id);
            self.profiles.insert(p.id.clone(), gid);
            let row = AnyRow::Profile(Profile {
                meta: SyncMeta {
                    updated_by: Some(owner.0),
                    rev: 1,
                    ..SyncMeta::new(gid, Timestamp(T0))
                },
                kind: if p.kind == "company" {
                    ProfileKind::Company
                } else {
                    ProfileKind::Family
                },
                name: p.id.clone(),
                owner_user_id: owner.0,
            });
            self.server.borrow_mut().st.rows.insert(
                row.key(),
                Stored {
                    row,
                    created_rev: 1,
                    updated_device: None,
                    updated_op: None,
                    lost_rev: None,
                    content_by: None,
                    content_at: None,
                    backed: true,
                },
            );
            self.names.insert(
                (
                    RowRef {
                        kind: RowKind::Profile,
                        id: gid,
                    },
                    1,
                ),
                "start".into(),
            );
        }
        let mut max_rev = 1;
        for r in &sc.start.rows {
            if r.entity == "invitation" {
                let id = self.id(&r.id);
                let email = r
                    .user
                    .as_deref()
                    .and_then(|u| self.users.get(u))
                    .map_or_else(String::new, |u| u.2.clone());
                let group = r
                    .group
                    .as_deref()
                    .map(|g| self.profile_of_name(g))
                    .unwrap_or(id);
                self.server.borrow_mut().st.invitations.insert(
                    id,
                    iamb_sync::model::Invitation {
                        group,
                        email,
                        role: s(r.fields.get("role")).unwrap_or_else(|| "member".into()),
                        status: s(r.fields.get("status")).unwrap_or_else(|| "pending".into()),
                    },
                );
                continue;
            }
            if !r.only_on.is_empty() {
                return Err("a row held by some devices only at the start".into());
            }
            let kind = RowKind::parse(&r.entity).ok_or("unknown entity")?;
            self.kinds.insert(r.id.clone(), kind);
            if r.rev == 0 {
                return Err("a row no server has seen at the start".into());
            }
            let by =
                r.by.as_deref()
                    .and_then(|u| self.users.get(u).map(|x| x.0))
                    .or_else(|| {
                        let p = r.profile.as_deref().or(r.group.as_deref())?;
                        sc.start
                            .profiles
                            .iter()
                            .find(|x| x.id == p)
                            .and_then(|x| self.users.get(&x.owner).map(|u| u.0))
                    })
                    .or_else(|| self.owner_of_refs(&r.refs()));
            let mut row = self
                .build(&r.entity, &r.id, &r.refs(), &r.fields, by, T0)
                .ok_or_else(|| format!("cannot build start row {}", r.id))?;
            {
                let m = row.meta_mut();
                m.rev = r.rev;
                if r.deleted {
                    m.deleted_at = Some(Timestamp(T0));
                }
            }
            let content_by = match s(r.fields.get("content_by")) {
                Some(u) if u == "nobody" => None,
                Some(u) => self.users.get(&u).map(|x| x.0),
                None => by,
            };
            max_rev = max_rev.max(r.rev);
            let key = row.key();
            self.names.insert((key, r.rev), "start".into());
            let profile = self.profile_hint(&row);
            let mut srv = self.server.borrow_mut();
            srv.st.rows.insert(
                key,
                Stored {
                    row,
                    created_rev: r.rev,
                    updated_device: None,
                    updated_op: None,
                    lost_rev: None,
                    content_by,
                    content_at: Some(T0),
                    backed: s(r.fields.get("backed")).as_deref() != Some("false"),
                },
            );
            if let Some(p) = profile {
                let h = srv.st.head_rev.entry(p).or_insert(0);
                *h = (*h).max(r.rev);
            }
        }
        {
            let mut srv = self.server.borrow_mut();
            srv.st.rev_seq = srv.st.rev_seq.max(max_rev) + 10;
        }
        let copied: BTreeSet<&str> = sc
            .start
            .devices
            .iter()
            .filter_map(|d| d.copy_of.as_deref())
            .collect();
        for d in &sc.start.devices {
            if d.protocol.is_some_and(|p| p != 1) {
                return Err("a device speaking another protocol version".into());
            }
            if !d.signed_in {
                return Err("a device that is not signed in at the start".into());
            }
            if d.copy_of.is_some() {
                continue;
            }
            let (user, _, _) = self.users.get(&d.user).ok_or("unknown user")?.clone();
            let session = self.server.borrow_mut().add_session();
            let dir = if copied.contains(d.id.as_str()) {
                let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
                let store = Store::open(dir.path()).map_err(|e| e.to_string())?;
                Some((store, dir))
            } else {
                None
            };
            self.add_dev(&d.id, user, Rc::new(Cell::new(session)), dir)?;
        }
        for r in &sc.steps {
            if let (Some(row), Some(e)) = (r.row.as_deref(), r.entity.as_deref())
                && let Some(k) = RowKind::parse(e)
            {
                self.kinds.insert(row.to_owned(), k);
            }
            if let Some(c) = r.creates.as_deref() {
                let _ = self.id(c);
            }
        }
        // Everyone signs in and is in step.
        self.settle();
        // A copied data directory (C25): a second installation with the original's device id,
        // position, outbox and tokens.
        for d in &sc.start.devices {
            let Some(original) = d.copy_of.as_deref() else {
                continue;
            };
            let o = self.dev(original).ok_or("unknown original")?;
            let src = self.devs[o]
                .dir
                .as_ref()
                .map(|x| x.path().to_path_buf())
                .ok_or("the original has no directory")?;
            let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
            copy_dir(&src, dir.path())?;
            let store = Store::open(dir.path()).map_err(|e| e.to_string())?;
            let user = self.devs[o].user;
            let session = Rc::new(Cell::new(self.devs[o].session.get()));
            self.add_dev(&d.id, user, session, Some((store, dir)))?;
            let n = self.devs.len() - 1;
            self.devs[n].version = self.devs[o].version.clone();
            self.devs[n].device_id = self.devs[o].device_id;
        }
        Ok(())
    }

    fn add_dev(
        &mut self,
        name: &str,
        user: Id,
        session: Rc<Cell<Id>>,
        disk: Option<(Store, tempfile::TempDir)>,
    ) -> Result<(), String> {
        let clock = Rc::new(Cell::new(T0.max(self.server.borrow().now)));
        let link = Rc::new(RefCell::new(LinkState {
            online: true,
            drop_answer: 0,
        }));
        let t = Link {
            inner: SharedTransport {
                server: self.server.clone(),
                caller: Caller {
                    user,
                    session: session.get(),
                },
            },
            state: link.clone(),
            session: session.clone(),
        };
        let (store, dir) = match disk {
            Some((store, dir)) => (store, Some(dir)),
            None => (Store::open_in_memory().map_err(|e| e.to_string())?, None),
        };
        let mut n = 0u64;
        let seed = name
            .bytes()
            .fold(7u64, |a, b| a.wrapping_mul(31).wrapping_add(u64::from(b)));
        let agent = Agent::new(
            t,
            StoreDevice::new(store, clock.clone()),
            AgentConfig::default(),
            Box::new(move || {
                n += 1;
                let mut b = [0u8; 10];
                b[..8].copy_from_slice(&(seed ^ n).to_be_bytes());
                Id::v7(T0 as u64, b)
            }),
        );
        self.devs.push(Dev {
            name: name.to_owned(),
            user,
            agent,
            link,
            clock,
            auto: true,
            signed_in: true,
            version: BTreeMap::new(),
            device_id: None,
            session,
            dir,
        });
        Ok(())
    }

    fn owner_of_refs(&self, refs: &iamb_testkit::sync_scenarios::Refs<'_>) -> Option<Id> {
        let sc = self.sc;
        let mut folder = refs.folder.map(str::to_owned);
        if let Some(sn) = refs.snippet {
            folder = sc
                .start
                .rows
                .iter()
                .find(|r| r.id == sn)
                .and_then(|r| r.folder.clone());
        }
        let mut cur = folder?;
        for _ in 0..8 {
            let r = sc.start.rows.iter().find(|r| r.id == cur)?;
            if let Some(p) = &r.profile {
                let prof = sc.start.profiles.iter().find(|x| &x.id == p)?;
                return self.users.get(&prof.owner).map(|u| u.0);
            }
            cur = r.parent.clone()?;
        }
        None
    }

    fn profile_hint(&self, row: &AnyRow) -> Option<Id> {
        let srv = self.server.borrow();
        row.own_profile().or_else(|| match row {
            AnyRow::Snippet(s) => match srv.st.rows.get(&RowRef {
                kind: RowKind::Folder,
                id: s.folder_id,
            }) {
                Some(Stored {
                    row: AnyRow::Folder(f),
                    ..
                }) => Some(f.profile_id),
                _ => None,
            },
            _ => None,
        })
    }

    // ---- running ------------------------------------------------------------------------

    fn sync_dev(&mut self, d: usize) {
        if !self.devs[d].signed_in {
            return;
        }
        let before = self.devs[d].version.clone();
        let r = self.devs[d].agent.sync();
        if std::env::var("IAMB_SYNC_REPLAY").is_ok() {
            eprintln!(
                "sync {}: {:?}",
                self.devs[d].name,
                r.as_ref()
                    .map(|r| (r.pages, r.rows, r.pushed, r.resynced, r.new_epoch))
            );
        }
        if matches!(r, Err(iamb_sync::RoundError::SignedOut { .. })) {
            self.devs[d].signed_in = false;
        }
        // The device id (a new one after `device_conflict`, C25).
        self.devs[d].device_id = self.devs[d]
            .agent
            .device
            .store
            .sync_info()
            .ok()
            .and_then(|i| i.device);
        self.learn(d, &before);
    }

    /// Names the server's new revisions after the steps that wrote them, and the device's rows
    /// after the versions they hold.
    fn learn(&mut self, d: usize, before: &BTreeMap<RowRef, String>) {
        let srv = self.server.borrow().clone();
        let device_id = self.devs[d].device_id;
        for p in &srv.processed[self.processed_seen..] {
            if Some(p.device) == device_id
                && let Some(name) = before.get(&p.key)
            {
                self.ops.entry(p.op_id).or_insert_with(|| name.clone());
            }
        }
        self.processed_seen = srv.processed.len();
        let mut all: Vec<(RowRef, &Stored)> = srv.st.rows.iter().map(|(k, v)| (*k, v)).collect();
        all.extend(srv.st.history.iter().map(|h| (h.key, &h.stored)));
        for (k, st) in all {
            let rev = st.row.meta().rev;
            if self.names.contains_key(&(k, rev)) {
                continue;
            }
            if let Some(op) = st.updated_op
                && let Some(n) = self.ops.get(&op)
            {
                self.names.insert((k, rev), n.clone());
            } else if let AnyRow::Membership(m) = &st.row
                && let Some(n) = self.removals.get(&(m.profile_id, m.user_id))
            {
                // A removal a device made again after a restore carries the name of the call.
                self.names.insert((k, rev), n.clone());
            }
        }
        for i in 0..self.devs.len() {
            let rows = self.devs[i]
                .agent
                .device
                .store
                .all_rows()
                .unwrap_or_default();
            let waiting: BTreeSet<RowRef> = self.devs[i]
                .agent
                .device
                .store
                .waiting_rows()
                .unwrap_or_default()
                .into_iter()
                .chain(
                    self.devs[i]
                        .agent
                        .device
                        .store
                        .blocked_rows()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(k, _)| k),
                )
                .collect();
            for r in rows {
                let k = r.key();
                if waiting.contains(&k) {
                    continue;
                }
                if let Some(n) = self.names.get(&(k, r.meta().rev)) {
                    self.devs[i].version.insert(k, n.clone());
                }
            }
        }
    }

    /// Syncs the device that acted and every device whose feed got something (a poke), in the
    /// order of the device ids, until nothing is left.
    fn settle_from(&mut self, acting: Option<usize>) {
        let mut due: BTreeSet<usize> = acting.into_iter().collect();
        let pokes: Vec<(Id, String, u64)> = std::mem::take(&mut self.server.borrow_mut().pokes);
        for (u, _, _) in pokes {
            for (i, d) in self.devs.iter().enumerate() {
                if d.user == u {
                    due.insert(i);
                }
            }
        }
        for _ in 0..12 {
            if due.is_empty() {
                return;
            }
            let now: Vec<usize> = std::mem::take(&mut due).into_iter().collect();
            for d in now {
                if self.devs[d].auto && self.devs[d].link.borrow().online {
                    self.sync_dev(d);
                }
            }
            let pokes: Vec<(Id, String, u64)> = std::mem::take(&mut self.server.borrow_mut().pokes);
            for (u, _, _) in pokes {
                for (i, d) in self.devs.iter().enumerate() {
                    if d.user == u {
                        due.insert(i);
                    }
                }
            }
        }
    }

    fn settle(&mut self) {
        let all: Vec<usize> = (0..self.devs.len()).collect();
        for d in all {
            self.settle_from(Some(d));
        }
    }

    fn dev(&self, name: &str) -> Option<usize> {
        self.devs.iter().position(|d| d.name == name)
    }

    fn now(&self) -> i64 {
        self.server.borrow().now
    }

    fn local_write(
        &mut self,
        d: usize,
        step_id: &str,
        row: &AnyRow,
        delete: bool,
    ) -> Result<(), String> {
        let now = Timestamp(self.now());
        self.devs[d].clock.set(now.0);
        let store = &mut self.devs[d].agent.device.store;
        let r = if delete {
            match row {
                AnyRow::Folder(_) => store.delete::<Folder>(row.id(), now),
                AnyRow::Snippet(_) => store.delete::<Snippet>(row.id(), now),
                AnyRow::Tag(_) => store.delete::<Tag>(row.id(), now),
                AnyRow::SnippetTag(_) => store.delete::<SnippetTag>(row.id(), now),
                AnyRow::Attachment(_) => store.delete::<Attachment>(row.id(), now),
                AnyRow::SnippetAttachment(_) => store.delete::<SnippetAttachment>(row.id(), now),
                AnyRow::FolderShare(_) => store.delete::<FolderShare>(row.id(), now),
                _ => return Err("cannot delete that locally".into()),
            }
        } else {
            match row {
                AnyRow::Folder(v) => store.put(v, now).map(drop),
                AnyRow::Snippet(v) => store.put(v, now).map(drop),
                AnyRow::Tag(v) => store.put(v, now).map(drop),
                AnyRow::SnippetTag(v) => store.put(v, now).map(drop),
                AnyRow::Attachment(v) => store.put(v, now).map(drop),
                AnyRow::SnippetAttachment(v) => store.put(v, now).map(drop),
                AnyRow::FolderShare(v) => store.put(v, now).map(drop),
                _ => return Err("cannot write that locally".into()),
            }
        };
        r.map_err(|e| format!("local write refused: {e}"))?;
        // The rows a folder's delete took with it on the device carry its name too.
        let k = row.key();
        self.devs[d].version.insert(k, step_id.to_owned());
        if delete && k.kind == RowKind::Folder {
            let rows = self.devs[d]
                .agent
                .device
                .store
                .all_rows()
                .unwrap_or_default();
            for r in rows {
                if r.meta().deleted_at == Some(now) {
                    self.devs[d].version.insert(r.key(), step_id.to_owned());
                }
            }
        }
        Ok(())
    }

    fn step(&mut self, i: usize) -> Result<(), String> {
        let st = &self.sc.steps[i];
        if st.at == "server" {
            match st.action.as_str() {
                "advance" => {
                    let days = i64::from(st.days.unwrap_or(1));
                    self.server.borrow_mut().advance(days * DAY);
                    for d in &self.devs {
                        d.clock.set(self.server.borrow().now);
                    }
                    for d in 0..self.devs.len() {
                        let cutoff = Timestamp(self.now() - 30 * DAY);
                        let _ = self.devs[d].agent.device.store.purge_deleted_before(cutoff);
                    }
                }
                "backup" => {
                    let name = st.id.clone().unwrap_or_else(|| "b".into());
                    self.server.borrow_mut().backup(&name);
                    let now = self.now();
                    self.backups.insert(name, now);
                    self.server.borrow_mut().now += 1;
                }
                "restore_backup" => {
                    let name = st.to.clone().ok_or("restore without a backup")?;
                    let last = self.server.borrow().log.last().map(|l| l.at);
                    self.server.borrow_mut().now += 1;
                    if !self.server.borrow_mut().restore(&name, last) {
                        return Err(format!("unknown backup {name}"));
                    }
                    self.restored = true;
                }
                "replay_calls" => {
                    let since = self.backups.values().copied().max().unwrap_or(T0);
                    let r0 = self.server.borrow().st.rev_seq;
                    self.server.borrow_mut().replay_calls(since);
                    self.name_new_revs(r0, st.id.as_deref().unwrap_or("replay"));
                }
                "set_protocol" => {
                    let (min, max) = (st.min.unwrap_or(1), st.max.unwrap_or(1));
                    self.server.borrow_mut().set_protocol(min, max);
                }
                "set_limit" => {
                    if let (Some(n), Some(v)) = (&st.name, st.value) {
                        self.server.borrow_mut().st.limits.insert(n.clone(), v);
                    }
                }
                other => return Err(format!("server step {other}")),
            }
            self.settle_from(None);
            return Ok(());
        }
        let d = self.dev(&st.at).ok_or("unknown device")?;
        let step_id = st.id.clone().unwrap_or_default();
        self.server.borrow_mut().now += 1_000;
        match st.action.as_str() {
            "offline" => self.devs[d].link.borrow_mut().online = false,
            "online" => {
                self.devs[d].link.borrow_mut().online = true;
                if matches!(st.answer.as_deref(), Some("lost" | "held")) {
                    self.devs[d].link.borrow_mut().drop_answer = 1;
                }
            }
            "manual" => self.devs[d].auto = false,
            "auto" => self.devs[d].auto = true,
            "pull" | "push" | "deliver" => {
                if matches!(st.answer.as_deref(), Some("lost" | "held")) {
                    self.devs[d].link.borrow_mut().drop_answer = 1;
                }
                if st.answer.as_deref() == Some("session_refused") {
                    // The authentication service no longer knows the device's tokens (after a
                    // restore, S78): the session has ended for this device.
                    let s = self.devs[d].session.get();
                    self.server.borrow_mut().end_session(s);
                }
                if st.fault.as_deref() == Some("duplicate") {
                    self.notes
                        .push("a push delivered twice is replayed as one push".into());
                }
                self.sync_dev(d);
            }
            "restart" => self.devs[d].agent.forget(),
            "sign_out" => {
                // D14: by hand. A device that answers syncs first, and the server's answer
                // confirms the sign-out; an offline one drops its tokens and confirms nothing.
                let online = self.devs[d].link.borrow().online;
                let mut confirmed = false;
                if online {
                    self.sync_dev(d);
                    confirmed = self.devs[d].signed_in;
                    let s = self.devs[d].session.get();
                    self.server.borrow_mut().end_session(s);
                }
                let now = Timestamp(self.now());
                self.devs[d]
                    .agent
                    .device
                    .store
                    .signed_out(confirmed, true, now)
                    .map_err(|e| e.to_string())?;
                self.devs[d].agent.forget();
                self.devs[d].signed_in = false;
            }
            "sign_in" => {
                if let Some(u) = st.user.as_deref()
                    && self.users.get(u).map(|x| x.0) != Some(self.devs[d].user)
                {
                    return Err("a sign-in with another account (D9)".into());
                }
                let s = self.server.borrow_mut().add_session();
                self.devs[d].session.set(s);
                self.devs[d].agent.forget();
                self.devs[d].signed_in = true;
            }
            "resolve" => {
                let row = st.row.clone().ok_or("resolve without a row")?;
                let key = self.key(&row).ok_or("unknown row")?;
                let (cid, rev) = {
                    let srv = self.server.borrow();
                    let c = srv
                        .st
                        .conflicts
                        .iter()
                        .rev()
                        .find(|c| c.key == key && c.state == "open")
                        .ok_or("no open conflict")?;
                    (c.id, c.versions.last().map_or(0, |v| v.rev))
                };
                let restore = st.choice.as_deref() == Some("restore");
                let args = json!({ "id": cid, "action": if restore { "restore" } else { "keep" }, "rev": rev });
                let r0 = self.server.borrow().st.rev_seq;
                let answer = self.devs[d].agent.raw_call("resolve_conflict", args);
                let got = match &answer {
                    Ok(_) => "ok".to_owned(),
                    Err(iamb_sync::RoundError::Protocol(e)) => {
                        format!("error:{}", e.trim_start_matches("unexpected refusal "))
                    }
                    Err(e) => e.to_string(),
                };
                if let Some(want) = &st.answer
                    && *want != got
                {
                    self.notes.push(format!(
                        "step {}: resolve answered {got}, the scenario says {want}",
                        i + 1
                    ));
                }
                self.name_new_revs(r0, &step_id);
            }
            "channel_down" | "channel_up" | "clock" => {}
            "create" | "edit" | "move" | "delete" | "undelete" => {
                let row_name = st.row.clone().ok_or("a write without a row")?;
                let key = self
                    .key(&row_name)
                    .ok_or_else(|| format!("unknown row {row_name}"))?;
                let user = self.devs[d].user;
                let current = self.devs[d]
                    .agent
                    .device
                    .store
                    .get_row(key)
                    .map_err(|e| e.to_string())?;
                let now = self.now();
                let row = match st.action.as_str() {
                    "create" => {
                        let entity = st.entity.clone().ok_or("create without entity")?;
                        let refs = st.refs();
                        self.build(&entity, &row_name, &refs, &st.fields, Some(user), now)
                            .ok_or("cannot build the new row")?
                    }
                    _ => {
                        let mut r =
                            current.ok_or_else(|| format!("{} does not hold {row_name}", st.at))?;
                        r.meta_mut().updated_by = Some(user);
                        if st.action == "edit" {
                            self.apply_fields(&mut r, &st.fields);
                        }
                        if st.action == "move" {
                            let to = st.to.clone().ok_or("move without to")?;
                            let target = (to != "top").then(|| self.id(&to));
                            match &mut r {
                                AnyRow::Snippet(sn) => {
                                    sn.folder_id = target.ok_or("snippet to top")?
                                }
                                AnyRow::Folder(f) => f.parent_id = target,
                                _ => return Err("move of that row".into()),
                            }
                        }
                        r
                    }
                };
                self.local_write(d, &step_id, &row, st.action == "delete")?;
            }
            "invite" | "accept" | "decline" | "withdraw" | "remove_member" | "set_role"
            | "leave" | "transfer_ownership" | "accept_ownership" | "delete_group"
            | "revoke_device" => {
                self.account(d, i)?;
            }
            other => return Err(format!("step {other}")),
        }
        self.settle_from(Some(d));
        Ok(())
    }

    fn apply_fields(&self, r: &mut AnyRow, fields: &BTreeMap<String, toml::Value>) {
        let f = |k: &str| s(fields.get(k));
        match r {
            AnyRow::Snippet(sn) => {
                if let Some(a) = f("abbreviation").and_then(|a| Abbreviation::new(&a).ok()) {
                    sn.abbreviation = a;
                }
                if let Some(v) = f("label") {
                    sn.label = v;
                }
                if let Some(v) = body_of(f("body")) {
                    sn.body = v;
                }
                if let Some(v) = f("kind") {
                    sn.kind = if v == "rich" {
                        SnippetKind::Rich
                    } else {
                        SnippetKind::Plain
                    };
                }
                if let Some(v) = fields.get("enabled").and_then(toml::Value::as_bool) {
                    sn.enabled = v;
                }
            }
            AnyRow::Folder(fo) => {
                if let Some(v) = f("name") {
                    fo.name = v;
                }
                if let Some(v) = fields.get("position").and_then(toml::Value::as_integer) {
                    fo.position = v;
                }
                if let Some(v) = f("member_permission") {
                    fo.member_permission = Some(if v == "edit" {
                        Permission::Edit
                    } else {
                        Permission::View
                    });
                }
            }
            AnyRow::Tag(t) => {
                if let Some(v) = f("name") {
                    t.name = v;
                }
                if let Some(v) = f("color") {
                    t.color = Some(v);
                }
            }
            AnyRow::FolderShare(x) => {
                if let Some(v) = f("permission") {
                    x.permission = if v == "view" {
                        Permission::View
                    } else {
                        Permission::Edit
                    };
                }
            }
            _ => {}
        }
    }

    fn name_new_revs(&mut self, r0: u64, name: &str) {
        let srv = self.server.borrow().clone();
        let mut all: Vec<(RowRef, u64)> = srv
            .st
            .rows
            .iter()
            .map(|(k, v)| (*k, v.row.meta().rev))
            .collect();
        all.extend(srv.st.history.iter().map(|h| (h.key, h.rev)));
        for (k, rev) in all {
            if rev > r0 {
                self.names
                    .entry((k, rev))
                    .or_insert_with(|| name.to_owned());
            }
        }
    }

    fn account(&mut self, d: usize, i: usize) -> Result<(), String> {
        let st = &self.sc.steps[i];
        // A call may name the membership row instead of the group and the person.
        let member_row = st.row.as_deref().and_then(|r| {
            self.sc
                .start
                .rows
                .iter()
                .find(|x| x.id == r && x.entity == "membership")
        });
        let group = st
            .group
            .as_deref()
            .or_else(|| member_row.and_then(|m| m.group.as_deref()))
            .map(|g| self.profile_of_name(g));
        let user = st
            .user
            .as_deref()
            .or_else(|| member_row.and_then(|m| m.user.as_deref()))
            .and_then(|u| self.users.get(u).map(|x| x.0));
        let (function, args, removal) = match st.action.as_str() {
            "invite" => {
                let id = self.id(st.creates.as_deref().or(st.row.as_deref()).unwrap_or("inv"));
                let email = st
                    .user
                    .as_deref()
                    .and_then(|u| self.users.get(u))
                    .map(|u| u.2.clone())
                    .unwrap_or_default();
                (
                    "invite",
                    json!({ "id": id, "group": group, "email": email, "role": st.role.clone().unwrap_or_else(|| "member".into()) }),
                    None,
                )
            }
            "accept" => (
                "accept_invitation",
                json!({ "id": self.id(st.row.as_deref().unwrap_or("inv")) }),
                None,
            ),
            "decline" => (
                "decline_invitation",
                json!({ "id": self.id(st.row.as_deref().unwrap_or("inv")) }),
                None,
            ),
            "withdraw" => {
                let id = self.id(st.row.as_deref().unwrap_or("inv"));
                (
                    "withdraw_invitation",
                    json!({ "id": id }),
                    Some((group, Some(id))),
                )
            }
            "remove_member" => (
                "remove_member",
                json!({ "group": group, "user": user }),
                Some((group, user)),
            ),
            "set_role" => (
                "set_member_role",
                json!({ "group": group, "user": user, "role": st.role }),
                Some((group, user)),
            ),
            "leave" => (
                "leave_group",
                json!({ "group": group }),
                Some((group, Some(self.devs[d].user))),
            ),
            "transfer_ownership" => (
                "transfer_ownership",
                json!({ "group": group, "user": user }),
                None,
            ),
            "accept_ownership" => ("accept_ownership", json!({ "group": group }), None),
            "delete_group" => (
                "delete_group",
                json!({ "group": group }),
                Some((group, None)),
            ),
            "revoke_device" => {
                let target = st
                    .device
                    .as_deref()
                    .and_then(|x| self.dev(x))
                    .and_then(|x| self.devs[x].device_id);
                (
                    "revoke_device",
                    json!({ "target": target }),
                    Some((None, target)),
                )
            }
            _ => return Err("account call".into()),
        };
        let r0 = self.server.borrow().st.rev_seq;
        let now = Timestamp(self.now());
        let res = self.devs[d]
            .agent
            .account_call(function, args, removal, now);
        let got = match res {
            Ok(Ok(_)) => "ok".to_owned(),
            Ok(Err(code)) => format!("error:{code}"),
            Err(e) => format!("{e}"),
        };
        if let Some(want) = &st.answer
            && *want != got
        {
            self.notes.push(format!(
                "step {}: {function} answered {got}, the scenario says {want}",
                i + 1
            ));
        }
        let name = st.id.clone().unwrap_or_else(|| function.to_owned());
        if let (Some(g), Some(u)) = (group, user)
            && matches!(
                function,
                "remove_member" | "leave_group" | "set_member_role"
            )
        {
            self.removals.insert((g, u), name.clone());
            // The scenario states as this step the call the device already made again by itself
            // after a restore (D7): the revision it wrote carries this step's name.
            if self.restored
                && let Some((key, rev)) =
                    self.server
                        .borrow()
                        .st
                        .rows
                        .iter()
                        .find_map(|(k, v)| match &v.row {
                            AnyRow::Membership(m) if m.profile_id == g && m.user_id == u => {
                                Some((*k, v.row.meta().rev))
                            }
                            _ => None,
                        })
            {
                self.names.insert((key, rev), name.clone());
                for dev in &mut self.devs {
                    if dev.version.contains_key(&key) {
                        dev.version.insert(key, name.clone());
                    }
                }
            }
        }
        self.name_new_revs(r0, &name);
        Ok(())
    }

    // ---- comparing ------------------------------------------------------------------------

    fn state_of(&self, row: Option<&AnyRow>) -> (&'static str, Option<String>) {
        match row {
            None => ("absent", None),
            Some(r) => {
                let name = self
                    .names
                    .get(&(r.key(), r.meta().rev))
                    .cloned()
                    .unwrap_or_else(|| format!("rev {}", r.meta().rev));
                (
                    if r.meta().deleted_at.is_some() {
                        "deleted"
                    } else {
                        "live"
                    },
                    Some(name),
                )
            }
        }
    }

    fn compare(&mut self) -> Vec<String> {
        let sc = self.sc;
        let mut diffs = Vec::new();
        let srv = self.server.borrow().clone();
        let mut expected_server: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
        for r in &sc.outcome.server.rows {
            expected_server.insert(r.row.clone(), (r.state.clone(), r.version.clone()));
            let Some(key) = self.key(&r.row) else {
                continue;
            };
            let (state, version) = self.state_of(srv.st.rows.get(&key).map(|s| &s.row));
            if state != r.state || (state != "absent" && version != r.version) {
                diffs.push(format!(
                    "server {}: {state} {version:?}, stated {} {:?}",
                    r.row, r.state, r.version
                ));
            }
        }
        // History.
        let mut want: BTreeSet<(String, String)> = sc
            .outcome
            .server
            .history
            .iter()
            .map(|v| (v.row.clone(), v.version.clone()))
            .collect();
        let mut got: BTreeSet<(String, String)> = BTreeSet::new();
        let names: Vec<(String, Id)> = self.ids.iter().map(|(n, i)| (n.clone(), *i)).collect();
        let by_id: BTreeMap<Id, String> = names.iter().map(|(n, i)| (*i, n.clone())).collect();
        for h in &srv.st.history {
            let Some(row) = by_id.get(&h.key.id) else {
                continue;
            };
            if !self.kinds.contains_key(row)
                || h.key.kind == RowKind::Membership && !expected_server.contains_key(row)
            {
                continue;
            }
            let v = self
                .names
                .get(&(h.key, h.rev))
                .cloned()
                .unwrap_or_else(|| format!("rev {}", h.rev));
            got.insert((row.clone(), v));
        }
        // Rows the scenario does not state (memberships of a call) are not compared.
        got.retain(|(r, _)| expected_server.contains_key(r));
        want.retain(|(r, _)| expected_server.contains_key(r));
        if got != want {
            diffs.push(format!("history {:?}, stated {:?}", got, want));
        }
        // Conflicts.
        let mut got_c: BTreeSet<String> = BTreeSet::new();
        for c in &srv.st.conflicts {
            let Some(row) = by_id.get(&c.key.id) else {
                continue;
            };
            let n = |rev: u64| {
                self.names
                    .get(&(c.key, rev))
                    .cloned()
                    .unwrap_or_else(|| format!("rev {rev}"))
            };
            let loser = c.versions.first().map(|v| n(v.rev)).unwrap_or_default();
            let mut carried: Vec<String> = c.versions.iter().skip(1).map(|v| n(v.rev)).collect();
            carried.sort();
            got_c.insert(format!(
                "{row} {} {}>{} {carried:?} {}",
                c.kind,
                n(c.winner_rev),
                loser,
                c.state
            ));
        }
        let want_c: BTreeSet<String> = sc
            .outcome
            .server
            .conflicts
            .iter()
            .map(|c| {
                let mut carried = c.carried.clone();
                carried.sort();
                format!(
                    "{} {} {}>{} {carried:?} {}",
                    c.row, c.kind, c.winner, c.loser, c.state
                )
            })
            .collect();
        if got_c != want_c {
            diffs.push(format!("conflicts {got_c:?}, stated {want_c:?}"));
        }
        // Devices.
        for (dname, out) in &sc.outcome.device {
            let Some(d) = self.dev(dname) else { continue };
            let mut expected = BTreeMap::new();
            if out.same_as.as_deref() == Some("server") {
                expected.clone_from(&expected_server);
            } else if let Some(other) = &out.same_as
                && let Some(o) = sc.outcome.device.get(other)
            {
                for r in &o.rows {
                    expected.insert(r.row.clone(), (r.state.clone(), r.version.clone()));
                }
            }
            for r in &out.rows {
                expected.insert(r.row.clone(), (r.state.clone(), r.version.clone()));
            }
            let rows: BTreeMap<RowRef, AnyRow> = self.devs[d]
                .agent
                .device
                .store
                .all_rows()
                .unwrap_or_default()
                .into_iter()
                .map(|r| (r.key(), r))
                .collect();
            let waiting: BTreeSet<RowRef> = self.devs[d]
                .agent
                .device
                .store
                .waiting_rows()
                .unwrap_or_default()
                .into_iter()
                .collect();
            let blocked: BTreeSet<RowRef> = self.devs[d]
                .agent
                .device
                .store
                .blocked_rows()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            for (row, (state, version)) in &expected {
                let Some(key) = self.key(row) else { continue };
                let held = rows.get(&key);
                let (s, mut v) = self.state_of(held);
                if held.is_some()
                    && (waiting.contains(&key)
                        || blocked.contains(&key)
                        || held.is_some_and(|h| h.meta().rev == 0))
                {
                    v = self.devs[d].version.get(&key).cloned();
                }
                // I1: for a row the server holds as a tombstone, a device holds the same
                // tombstone or nothing (a link's tombstone leaves with its tag, D16).
                let tombstone_or_nothing = s == "absent" && state == "deleted";
                if !tombstone_or_nothing && (s != state || (s != "absent" && v != *version)) {
                    diffs.push(format!(
                        "device {dname} {row}: {s} {v:?}, stated {state} {version:?}"
                    ));
                }
            }
            let kept = self.devs[d]
                .agent
                .device
                .store
                .kept_changes()
                .unwrap_or_default();
            let kept_rows: BTreeSet<String> = kept
                .iter()
                .filter_map(|k| by_id.get(&k.key.id).cloned())
                .collect();
            let want_kept: BTreeSet<String> = out
                .kept
                .iter()
                .filter_map(|step| {
                    sc.steps
                        .iter()
                        .find(|s| s.id.as_deref() == Some(step.as_str()))
                        .and_then(|s| s.row.clone())
                })
                .collect();
            if kept_rows != want_kept {
                diffs.push(format!(
                    "device {dname} kept {kept_rows:?}, stated {want_kept:?}"
                ));
            }
            let blocked_rows: BTreeSet<String> = blocked
                .iter()
                .filter_map(|k| by_id.get(&k.id).cloned())
                .collect();
            let want_blocked: BTreeSet<String> = out
                .blocked
                .iter()
                .filter_map(|step| {
                    sc.steps
                        .iter()
                        .find(|s| s.id.as_deref() == Some(step.as_str()))
                        .and_then(|s| s.row.clone())
                })
                .collect();
            if blocked_rows != want_blocked {
                diffs.push(format!(
                    "device {dname} blocked {blocked_rows:?}, stated {want_blocked:?}"
                ));
            }
            let waiting_n = waiting.union(&blocked).count() as u32;
            if waiting_n != out.outbox {
                diffs.push(format!(
                    "device {dname} outbox {waiting_n}, stated {}",
                    out.outbox
                ));
            }
        }
        for v in &srv.violations {
            diffs.push(format!("I7: {v}"));
        }
        diffs
    }
}

/// What the replay does not support yet, by scenario: the step or start it cannot run.
fn replay(sc: &Scenario) -> Result<Vec<String>, String> {
    let mut r = Replay::new(sc);
    r.setup()?;
    for i in 0..sc.steps.len() {
        r.step(i)
            .map_err(|e| format!("step {} ({}): {e}", i + 1, sc.steps[i].action))?;
    }
    for d in 0..r.devs.len() {
        let online = r.devs[d].link.borrow().online;
        if online {
            r.sync_dev(d);
        }
    }
    let mut diffs = r.compare();
    // An account call answered otherwise than the scenario says is a difference too.
    diffs.extend(
        r.notes
            .iter()
            .filter(|n| n.contains("answered"))
            .map(|n| format!("answer: {n}")),
    );
    Ok(diffs)
}

#[test]
fn every_scenario_replays_to_its_stated_outcome() {
    let file = load(&scenarios_path()).unwrap();
    let only = std::env::var("IAMB_SYNC_REPLAY").ok();
    let mut passed = Vec::new();
    let mut differ = Vec::new();
    let mut unsupported = Vec::new();
    for sc in &file.scenarios {
        if only.as_deref().is_some_and(|o| o != sc.id) {
            continue;
        }
        match replay(sc) {
            Ok(diffs) if diffs.is_empty() => passed.push(sc.id.clone()),
            Ok(diffs) => differ.push((sc.id.clone(), diffs)),
            Err(e) => unsupported.push((sc.id.clone(), e)),
        }
    }
    let report = json!({
        "scenarios": file.scenarios.len(),
        "reach_their_outcome": passed,
        "differ": differ.iter().map(|(i, d)| json!({ "id": i, "differences": d })).collect::<Vec<_>>(),
        "not_replayed": unsupported.iter().map(|(i, e)| json!({ "id": i, "why": e })).collect::<Vec<_>>(),
    });
    if let Ok(path) = std::env::var("IAMB_SYNC_REPLAY_REPORT") {
        let _ = std::fs::write(path, serde_json::to_string_pretty(&report).unwrap());
    }
    eprintln!(
        "replayed {}: {} reach their stated outcome, {} differ, {} not replayed",
        file.scenarios.len(),
        passed.len(),
        differ.len(),
        unsupported.len()
    );
    for (id, d) in &differ {
        eprintln!("{id}: {}", d.join(" | "));
    }
    for (id, e) in &unsupported {
        eprintln!("{id}: not replayed: {e}");
    }
    // The scenarios that reach their stated outcome must keep doing so: a list kept here, so
    // that a change that breaks one fails, and one that newly passes is added.
    let must_pass: BTreeSet<&str> = MUST_PASS.iter().copied().collect();
    if only.is_none() {
        let broken: Vec<&&str> = must_pass
            .iter()
            .filter(|id| !passed.iter().any(|p| p == **id))
            .collect();
        assert!(
            broken.is_empty(),
            "scenarios that no longer reach their outcome: {broken:?}"
        );
    }
}

/// The scenarios that reach their stated outcome, as far as the replay compares it. The others use
/// what the replay does not drive: another protocol version (S44), a first sign-in with a local
/// library (S48, D9), copying a held snippet (S56, a review of D10, not a sync rule), rows on some
/// devices only at the start (S83), and attachment rows a device does not hold (S73, S84).
const MUST_PASS: &[&str] = &[
    "S01", "S02", "S03", "S04", "S05", "S06", "S07", "S08", "S09", "S10", "S11", "S12", "S13",
    "S14", "S15", "S16", "S17", "S18", "S19", "S20", "S21", "S22", "S23", "S24", "S25", "S26",
    "S27", "S28", "S29", "S30", "S31", "S32", "S33", "S34", "S35", "S36", "S37", "S38", "S39",
    "S40", "S41", "S42", "S43", "S45", "S46", "S47", "S49", "S50", "S51", "S52", "S53", "S54",
    "S55", "S57", "S58", "S59", "S60", "S61", "S62", "S63", "S64", "S65", "S66", "S67", "S68",
    "S69", "S70", "S71", "S72", "S74", "S75", "S76", "S77", "S78", "S79", "S80", "S81", "S82",
    "S85", "S86", "S87",
];
