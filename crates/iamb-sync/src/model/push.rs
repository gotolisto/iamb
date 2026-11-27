//! `hello` and `push` of the model (§6.2, §6.3; C1 to C15, C17 to C19).

use std::collections::BTreeSet;

use iamb_core::model::{Id, Timestamp};
use iamb_core::sync::{Access, AnyRow, RowKind, RowRef};
use serde_json::{Value, json};

use super::{
    Caller, Conflict, DeviceRow, Hist, LogLine, Offered, Processed, Server, Stored, Tx, arg_id,
};
use crate::api::row_from_wire;

/// A row without its sync fields, for comparing what two versions hold.
fn strip(r: &AnyRow) -> AnyRow {
    let mut r = r.clone();
    let m = r.meta_mut();
    m.created_at = Timestamp(0);
    m.updated_at = Timestamp(0);
    m.updated_by = None;
    m.rev = 0;
    m.deleted_at = None;
    r
}

/// Equal in every synced field (C4).
pub(super) fn fields_eq(a: &AnyRow, b: &AnyRow) -> bool {
    strip(a) == strip(b)
}

/// Equal in the fields whose change records a conflict (C3).
pub(super) fn recorded_eq(a: &AnyRow, b: &AnyRow) -> bool {
    match (a, b) {
        (AnyRow::Snippet(x), AnyRow::Snippet(y)) => {
            x.folder_id == y.folder_id
                && x.abbreviation == y.abbreviation
                && x.label == y.label
                && x.kind == y.kind
                && x.body == y.body
                && x.trigger == y.trigger
                && x.case_mode == y.case_mode
                && x.word_start == y.word_start
                && x.enabled == y.enabled
        }
        (AnyRow::Folder(x), AnyRow::Folder(y)) => {
            x.parent_id == y.parent_id
                && x.name == y.name
                && x.member_permission == y.member_permission
                && x.defaults == y.defaults
        }
        (AnyRow::Tag(x), AnyRow::Tag(y)) => x.name == y.name && x.color == y.color,
        _ => true,
    }
}

/// Equal in what the snippet does (C17).
pub(super) fn content_eq(a: &AnyRow, b: &AnyRow) -> bool {
    match (a, b) {
        (AnyRow::Snippet(x), AnyRow::Snippet(y)) => {
            x.abbreviation == y.abbreviation
                && x.kind == y.kind
                && x.body == y.body
                && x.trigger == y.trigger
                && x.case_mode == y.case_mode
                && x.word_start == y.word_start
        }
        _ => true,
    }
}

fn deleted(r: &AnyRow) -> bool {
    r.meta().deleted_at.is_some()
}

/// What a push result says (§6.3).
#[derive(Debug, Clone, Default)]
pub(super) struct Outcome {
    pub status: &'static str,
    pub reason: Option<&'static str>,
    pub conflict: Option<Id>,
    pub adjusted: Vec<String>,
    pub cascade: Option<Value>,
}

impl Outcome {
    fn new(status: &'static str) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }

    fn reason(status: &'static str, reason: &'static str) -> Self {
        Self {
            status,
            reason: Some(reason),
            ..Self::default()
        }
    }
}

/// One op as parsed.
pub(super) struct Op {
    pub op_id: Id,
    pub follows: Vec<Id>,
    pub key: RowRef,
    pub put: bool,
    pub base: u64,
    pub row: Option<AnyRow>,
    pub cascade: Vec<(RowRef, u64)>,
    pub repush: Option<(String, u64, String)>,
    pub author_self: bool,
    pub created_at: i64,
}

fn parse_op(v: &Value) -> Result<Op, &'static str> {
    let op_id = arg_id(v, "op_id").ok_or("invalid")?;
    let entity = v.get("entity").and_then(Value::as_str).ok_or("invalid")?;
    let kind = RowKind::parse(entity).ok_or("invalid")?;
    let id = arg_id(v, "id").ok_or("invalid")?;
    let put = match v.get("kind").and_then(Value::as_str) {
        Some("put") => true,
        Some("delete") => false,
        _ => return Err("invalid"),
    };
    let row = if put {
        let r = v.get("row").ok_or("invalid")?;
        let mut row = row_from_wire(entity, r).map_err(|_| "invalid")?;
        row.meta_mut().id = id;
        Some(row)
    } else {
        None
    };
    let created_at = v
        .get("row")
        .and_then(|r| r.get("created_at"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cascade = v
        .get("cascade")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    Some((
                        RowRef {
                            kind: RowKind::parse(i.get("entity")?.as_str()?)?,
                            id: arg_id(i, "id")?,
                        },
                        i.get("base_rev").and_then(Value::as_u64).unwrap_or(0),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let repush = v.get("repush").and_then(Value::as_str).map(|k| {
        (
            k.to_owned(),
            v.get("repush_rev").and_then(Value::as_u64).unwrap_or(0),
            v.get("repush_epoch")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )
    });
    Ok(Op {
        op_id,
        follows: v
            .get("follows")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str()?.parse().ok()).collect())
            .unwrap_or_default(),
        key: RowRef { kind, id },
        put,
        base: v.get("base_rev").and_then(Value::as_u64).unwrap_or(0),
        row,
        cascade,
        repush,
        author_self: v.get("author").and_then(Value::as_str) == Some("self"),
        created_at,
    })
}

impl Server {
    // ---- hello (§6.2, C25) ------------------------------------------------------------------

    pub(super) fn hello(&mut self, c: Caller, args: &Value) -> Value {
        let Some(dev) = args.get("device").and_then(|d| arg_id(d, "id")) else {
            return self.err("invalid");
        };
        let protos: Vec<u32> = args
            .get("protocols")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| u32::try_from(x.as_u64()?).ok())
                    .collect()
            })
            .unwrap_or_default();
        let lo = protos.first().copied().unwrap_or(1);
        let hi = protos.last().copied().unwrap_or(lo);
        let proto = hi.min(self.st.protocol.1);
        if proto < lo.max(self.st.protocol.0) {
            return self.err(if hi < self.st.protocol.0 {
                "upgrade_required"
            } else {
                "server_too_old"
            });
        }
        let max_rev = args.get("max_rev").and_then(Value::as_u64).unwrap_or(0);
        if max_rev > self.st.rev_seq {
            return self.err("server_behind");
        }
        let by_session = self
            .st
            .devices
            .iter()
            .find(|(_, d)| d.session == c.session)
            .map(|(id, d)| (*id, d.clone()));
        let by_id = self.st.devices.get(&dev).cloned();
        let mut tx = Tx::default();
        if let Some((id, d)) = by_session {
            if d.revoked {
                return self.err("device_revoked");
            }
            if id != dev {
                return self.err("device_conflict");
            }
            if let Some(x) = self.st.devices.get_mut(&dev) {
                x.protocol = proto;
            }
        } else if let Some(d) = by_id {
            if d.revoked || d.user != c.user || self.st.sessions.contains(&d.session) {
                return self.err("device_conflict");
            }
            if let Some(x) = self.st.devices.get_mut(&dev) {
                x.session = c.session;
                x.protocol = proto;
            }
        } else {
            self.st.devices.insert(
                dev,
                DeviceRow {
                    user: c.user,
                    session: c.session,
                    revoked: false,
                    position: None,
                    position_before: None,
                    protocol: proto,
                },
            );
            tx.account.insert(c.user);
        }
        let personal = self.st.users.get(&c.user).map(|u| u.0);
        let head = self.st.feeds.get(&c.user).map_or(0, |f| f.head);
        self.commit(tx);
        json!({ "error": null, "protocol": proto, "user": c.user, "personal_profile": personal,
                "epoch": self.epoch().id, "head": head, "retention_days": 30,
                "server_time": self.now, "display_name": "",
                "limits": { "push_ops": 200, "push_bytes": 4_194_304, "pull_entries": 500,
                            "cascade_max": self.limits.cascade_max, "attachment_bytes_max": 26_214_400 } })
    }

    // ---- helpers ------------------------------------------------------------------------------

    fn edit_on(&self, user: Id, folder: Id) -> bool {
        self.permission(user, folder)
            .is_some_and(|g| matches!(g.access, Access::Edit | Access::Manage))
    }

    fn manage_on(&self, user: Id, folder: Id) -> bool {
        self.permission(user, folder)
            .is_some_and(|g| g.access == Access::Manage)
    }

    fn live_share_root(&self, folder: Id) -> bool {
        self.st.rows.values().any(|s| {
            matches!(&s.row, AnyRow::FolderShare(x) if x.folder_id == folder && x.meta.deleted_at.is_none())
        })
    }

    /// The profile a row of an op belongs to (C24: read before any lock).
    fn op_profile(&self, op: &Op, request: &[Op]) -> Option<Id> {
        self.st
            .rows
            .get(&op.key)
            .and_then(|s| self.row_profile(&s.row, request))
            .or_else(|| op.row.as_ref().and_then(|r| self.row_profile(r, request)))
    }

    /// The profile a row is in, from the rows it refers to (or the request's own rows).
    fn row_profile(&self, row: &AnyRow, request: &[Op]) -> Option<Id> {
        let folder_profile = |f: Id| -> Option<Id> {
            self.folder(f).map(|x| x.profile_id).or_else(|| {
                request.iter().find_map(|o| match &o.row {
                    Some(AnyRow::Folder(x)) if x.meta.id == f => Some(x.profile_id),
                    _ => None,
                })
            })
        };
        let snippet_profile = |s: Id| -> Option<Id> {
            let folder = self.snippet(s).map(|x| x.folder_id).or_else(|| {
                request.iter().find_map(|o| match &o.row {
                    Some(AnyRow::Snippet(x)) if x.meta.id == s => Some(x.folder_id),
                    _ => None,
                })
            })?;
            folder_profile(folder)
        };
        let of = |r: &AnyRow| -> Option<Id> {
            match r {
                AnyRow::Folder(f) => Some(f.profile_id),
                AnyRow::Tag(t) => Some(t.profile_id),
                AnyRow::Attachment(a) => Some(a.profile_id),
                AnyRow::Snippet(s) => folder_profile(s.folder_id),
                AnyRow::FolderShare(x) => folder_profile(x.folder_id),
                AnyRow::SnippetTag(l) => snippet_profile(l.snippet_id),
                AnyRow::SnippetAttachment(l) => snippet_profile(l.snippet_id),
                AnyRow::Profile(p) => Some(p.meta.id),
                AnyRow::Membership(m) => Some(m.profile_id),
            }
        };
        of(row)
    }

    /// The version of a row at a revision: the row itself or a history version.
    fn version_at(&self, key: RowRef, rev: u64) -> Option<Stored> {
        if let Some(s) = self.st.rows.get(&key)
            && s.row.meta().rev == rev
        {
            return Some(s.clone());
        }
        self.st
            .history
            .iter()
            .find(|h| h.key == key && h.rev == rev)
            .map(|h| h.stored.clone())
    }

    /// The version written by the latest op an op names in `follows`.
    fn version_of_follows(&self, key: RowRef, follows: &[Id]) -> Option<Stored> {
        if follows.is_empty() {
            return None;
        }
        if let Some(s) = self.st.rows.get(&key)
            && s.updated_op.is_some_and(|o| follows.contains(&o))
        {
            return Some(s.clone());
        }
        self.st
            .history
            .iter()
            .filter(|h| h.key == key && h.stored.updated_op.is_some_and(|o| follows.contains(&o)))
            .max_by_key(|h| h.rev)
            .map(|h| h.stored.clone())
    }

    /// Replaces the row with `new`, keeping the version it replaces in the history (C2).
    fn write(&mut self, tx: &mut Tx, key: RowRef, new: Stored) {
        self.touch(tx, key);
        let rev = new.row.meta().rev;
        if let Some(old) = self.st.rows.insert(key, new) {
            let old_rev = old.row.meta().rev;
            if !self
                .st
                .history
                .iter()
                .any(|h| h.key == key && h.rev == old_rev)
            {
                self.st.history.push(Hist {
                    key,
                    rev: old_rev,
                    stored: old,
                    replaced_by: rev,
                });
            }
        }
    }

    /// Records a conflict (C3): a new one takes over every version an open one offered.
    pub(super) fn record_conflict(
        &mut self,
        tx: &mut Tx,
        key: RowRef,
        kind: &str,
        winner_rev: u64,
        winner_user: Id,
        winner_device: Option<Id>,
        replaced: &Stored,
    ) -> Id {
        let id = self.new_id();
        let mut versions = vec![Offered {
            rev: replaced.row.meta().rev,
            user: replaced.row.meta().updated_by,
            device: replaced.updated_device,
            taken_over: false,
        }];
        let mut parties: BTreeSet<Id> = BTreeSet::new();
        parties.insert(winner_user);
        for c in &mut self.st.conflicts {
            if c.key == key && c.state == "open" {
                c.state = "replaced".into();
                for v in &c.versions {
                    versions.push(Offered {
                        taken_over: true,
                        ..v.clone()
                    });
                }
                parties.insert(c.winner_user);
                let old = c.id;
                tx.conflicts.push((old, parties.clone(), key));
            }
        }
        for v in &versions {
            if let Some(u) = v.user {
                parties.insert(u);
            }
        }
        self.st.conflicts.push(Conflict {
            id,
            key,
            kind: kind.to_owned(),
            winner_rev,
            winner_user,
            winner_device,
            state: "open".into(),
            versions,
        });
        tx.conflicts.push((id, parties, key));
        id
    }

    fn new_stored(&self, mut row: AnyRow, rev: u64, user: Id, device: Id, op: Id) -> Stored {
        let created = row.meta().created_at.0.min(self.now);
        let m = row.meta_mut();
        m.rev = rev;
        m.updated_at = Timestamp(self.now);
        m.updated_by = Some(user);
        m.deleted_at = None;
        m.created_at = Timestamp(created);
        Stored {
            row,
            created_rev: rev,
            updated_device: Some(device),
            updated_op: Some(op),
            lost_rev: None,
            content_by: None,
            content_at: None,
            backed: false,
        }
    }

    // ---- C7: folders on the way up ------------------------------------------------------------

    /// Restores every deleted folder from `folder` up, if the writer may; false when it may not.
    fn restore_up(&mut self, tx: &mut Tx, user: Id, device: Id, op: Id, folder: Id) -> bool {
        let chain = self.chain(folder);
        let mut dead: Vec<Id> = Vec::new();
        for f in &chain {
            let Some(x) = self.folder(*f) else {
                return false;
            };
            if x.meta.deleted_at.is_some() {
                let ok = match x.parent_id {
                    None => self.manages(user, x.profile_id),
                    Some(p) => self.edit_on(user, p),
                };
                if !ok {
                    return false;
                }
                dead.push(*f);
            }
        }
        for f in dead.into_iter().rev() {
            let key = RowRef {
                kind: RowKind::Folder,
                id: f,
            };
            let Some(cur) = self.st.rows.get(&key).cloned() else {
                continue;
            };
            let profile = cur.row.own_profile().unwrap_or(f);
            let rev = self.next_rev(profile);
            let mut row = cur.row.clone();
            let m = row.meta_mut();
            m.rev = rev;
            m.deleted_at = None;
            m.updated_at = Timestamp(self.now);
            m.updated_by = Some(user);
            let new = Stored {
                row,
                updated_device: Some(device),
                updated_op: Some(op),
                ..cur.clone()
            };
            self.write(tx, key, new);
            self.record_conflict(tx, key, "folder_restored", rev, user, Some(device), &cur);
        }
        true
    }

    // ---- C8: the tree kept valid --------------------------------------------------------------

    fn height(&self, folder: Id) -> usize {
        let mut best = 0usize;
        for s in self.st.rows.values() {
            if let AnyRow::Folder(f) = &s.row
                && f.meta.deleted_at.is_none()
                && f.meta.id != folder
            {
                let chain = self.chain(f.meta.id);
                if let Some(pos) = chain.iter().position(|x| *x == folder) {
                    best = best.max(pos);
                }
            }
        }
        best
    }

    // ---- push -----------------------------------------------------------------------------------

    pub(super) fn push(&mut self, c: Caller, args: &Value) -> Value {
        let Some(device) = arg_id(args, "device") else {
            return self.err("invalid");
        };
        let position = args
            .get("position")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let raw: Vec<Value> = args
            .get("ops")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if raw.len() > 200 {
            return self.err("too_large");
        }
        if let Err(e) = self.position(c, device, position.as_deref(), "push", None) {
            return e;
        }
        let mut ops: Vec<(usize, Result<Op, &'static str>)> =
            raw.iter().map(parse_op).enumerate().collect();
        let parsed: Vec<Op> = raw.iter().filter_map(|v| parse_op(v).ok()).collect();
        let depth_in_request = |op: &Op| -> usize {
            let mut d = 0usize;
            let mut cur = match &op.row {
                Some(AnyRow::Folder(f)) => f.parent_id,
                _ => None,
            };
            while let Some(p) = cur {
                d += 1;
                if d > 10 {
                    break;
                }
                cur = parsed
                    .iter()
                    .find_map(|o| match &o.row {
                        Some(AnyRow::Folder(f)) if f.meta.id == p => Some(f.parent_id),
                        _ => None,
                    })
                    .flatten();
            }
            d
        };
        let sortkey = |op: &Result<Op, &'static str>| -> usize {
            let Ok(op) = op else { return 50 };
            if op.put {
                match op.key.kind {
                    RowKind::Folder => 100 + depth_in_request(op),
                    RowKind::Tag => 200,
                    RowKind::Attachment => 210,
                    RowKind::Snippet => 300,
                    RowKind::SnippetTag => 400,
                    RowKind::SnippetAttachment => 410,
                    RowKind::FolderShare => 500,
                    _ => 50,
                }
            } else {
                match op.key.kind {
                    RowKind::SnippetTag => 600,
                    RowKind::SnippetAttachment => 610,
                    RowKind::Snippet => 700,
                    RowKind::FolderShare => 750,
                    RowKind::Folder => 800 + (20 - self.chain(op.key.id).len().min(20)),
                    RowKind::Tag => 900,
                    RowKind::Attachment => 910,
                    _ => 50,
                }
            }
        };
        ops.sort_by_key(|(i, o)| (sortkey(o), *i));
        let mut tx = self.begin();
        let mut results: Vec<Value> = vec![Value::Null; raw.len()];
        let mut blocked: BTreeSet<Id> = BTreeSet::new();
        for (idx, op) in ops {
            let op = match op {
                Ok(op) => op,
                Err(_) => {
                    results[idx] = json!({ "op_id": raw[idx].get("op_id"), "status": "blocked",
                        "reason": "invalid", "conflict": null, "adjusted": [], "replayed": false,
                        "blob": null, "row": null, "origin": null });
                    continue;
                }
            };
            if let Some((status, reason, conflict, adjusted)) =
                self.st.receipts.get(&(device, op.op_id)).cloned()
            {
                let mut r = self.result_json(
                    c,
                    &op,
                    &status,
                    reason.as_deref(),
                    conflict,
                    &adjusted,
                    true,
                );
                if op.key.kind == RowKind::Folder
                    && !op.put
                    && let Value::Object(m) = &mut r
                {
                    m.insert("cascade".into(), self.cascade_answer(c, &op.cascade));
                }
                results[idx] = r;
                continue;
            }
            let out = self.push_op(&mut tx, c, device, &op, &parsed, &blocked);
            if out.status == "blocked" {
                blocked.insert(op.key.id);
            }
            if matches!(
                out.status,
                "applied" | "unchanged" | "superseded" | "rejected"
            ) {
                self.st.receipts.insert(
                    (device, op.op_id),
                    (
                        out.status.to_owned(),
                        out.reason.map(str::to_owned),
                        out.conflict,
                        out.adjusted.clone(),
                    ),
                );
            }
            let mut r = self.result_json(
                c,
                &op,
                out.status,
                out.reason,
                out.conflict,
                &out.adjusted,
                false,
            );
            if let (Some(casc), Value::Object(m)) = (out.cascade, &mut r) {
                m.insert("cascade".into(), casc);
            }
            self.processed.push(Processed {
                device,
                op_id: op.op_id,
                key: op.key,
                row: op.row.clone(),
                status: out.status.to_owned(),
                rev: self.st.rows.get(&op.key).map(|s| s.row.meta().rev),
                at: self.now,
            });
            results[idx] = r;
        }
        self.commit(tx);
        let head = self.st.feeds.get(&c.user).map_or(0, |f| f.head);
        json!({ "error": null, "epoch": self.epoch().id, "head": head, "results": results })
    }

    #[allow(clippy::too_many_arguments)]
    fn result_json(
        &self,
        c: Caller,
        op: &Op,
        status: &str,
        reason: Option<&str>,
        conflict: Option<Id>,
        adjusted: &[String],
        replayed: bool,
    ) -> Value {
        let (row, origin) = self
            .wire_row(c.user, op.key)
            .map_or((Value::Null, Value::Null), |(r, o)| (r, o));
        json!({ "op_id": op.op_id, "status": status, "reason": reason, "conflict": conflict,
                "adjusted": adjusted, "replayed": replayed, "blob": null, "row": row, "origin": origin })
    }

    /// The answer for every row of a folder delete's cascade list (C6).
    fn cascade_answer(&self, c: Caller, list: &[(RowRef, u64)]) -> Value {
        let mut out = Vec::new();
        for (key, base) in list {
            let cur = self.st.rows.get(key);
            match cur {
                Some(s) if self.can_read(c.user, *key) => {
                    let rev = s.row.meta().rev;
                    if !deleted(&s.row) && rev != *base {
                        let (row, origin) = self
                            .wire_row(c.user, *key)
                            .map_or((Value::Null, Value::Null), |(r, o)| (r, o));
                        out.push(
                            json!({ "entity": key.kind.as_str(), "id": key.id, "rev": rev,
                                         "deleted": false, "row": row, "origin": origin }),
                        );
                    } else {
                        out.push(
                            json!({ "entity": key.kind.as_str(), "id": key.id, "rev": rev,
                                         "deleted": deleted(&s.row) }),
                        );
                    }
                }
                _ => out.push(json!({ "entity": key.kind.as_str(), "id": key.id, "gone": true })),
            }
        }
        Value::Array(out)
    }

    fn push_op(
        &mut self,
        tx: &mut Tx,
        c: Caller,
        device: Id,
        op: &Op,
        request: &[Op],
        blocked: &BTreeSet<Id>,
    ) -> Outcome {
        if op.key.kind.server_only() {
            return Outcome::reason("rejected", "not_pushable");
        }
        let Some(profile) = self.op_profile(op, request) else {
            return Outcome::reason("rejected", "parent_gone");
        };
        // A create inserts and nothing else (4.3, finding 3): one that names an id in use in
        // another profile is not this op's to write.
        if op.put
            && op.base == 0
            && op.repush.is_none()
            && self.st.rows.contains_key(&op.key)
            && op
                .row
                .as_ref()
                .and_then(|r| self.row_profile(r, request))
                .is_some_and(|p| p != profile)
        {
            return Outcome::reason("blocked", "invalid");
        }
        // An op on a profile or a group the caller has no grant in (4.3, finding 6): the answer of
        // an unknown row, whatever the server holds under that id.
        let group = match &op.row {
            Some(AnyRow::FolderShare(x)) => Some(x.profile_id),
            _ => None,
        };
        if !self.has_grant(c.user, profile) || group.is_some_and(|g| !self.has_grant(c.user, g)) {
            if !op.put {
                let mut o = Outcome::new("unchanged");
                if op.key.kind == RowKind::Folder {
                    o.cascade = Some(self.cascade_answer(c, &op.cascade));
                }
                return o;
            }
            return Outcome::reason("rejected", "parent_gone");
        }
        if let Some(row) = &op.row {
            // C14: sizes.
            let too_large = match row {
                AnyRow::Snippet(s) => {
                    s.body.len() > self.limits.body_max || s.label.len() > self.limits.name_max
                }
                AnyRow::Folder(f) => f.name.len() > self.limits.name_max,
                AnyRow::Tag(t) => t.name.len() > self.limits.name_max,
                _ => false,
            };
            if too_large {
                return Outcome::reason("blocked", "too_large");
            }
            if row.references().iter().any(|r| blocked.contains(&r.id)) {
                return Outcome::reason("blocked", "parent_blocked");
            }
            if let Some(limit) = self.st.limits.get("snippets_max").copied()
                && row.kind() == RowKind::Snippet
                && !self.st.rows.contains_key(&op.key)
            {
                let n = self
                    .st
                    .rows
                    .values()
                    .filter(|s| {
                        matches!(&s.row, AnyRow::Snippet(x) if x.meta.deleted_at.is_none()
                        && self.folder(x.folder_id).is_some_and(|f| f.profile_id == profile))
                    })
                    .count();
                if i64::try_from(n).unwrap_or(i64::MAX) >= limit {
                    return Outcome::reason("blocked", "plan_limit");
                }
            }
        }
        if op.key.kind == RowKind::FolderShare {
            return self.push_share(tx, c, device, op);
        }
        if op.key.kind == RowKind::Folder && !op.put {
            return self.push_folder_delete(tx, c, device, op, profile);
        }
        if let Some((kind, rp_rev, rp_epoch)) = &op.repush {
            return self.push_repush(tx, c, device, op, profile, kind, *rp_rev, rp_epoch);
        }
        self.push_row(tx, c, device, op, profile, op.base)
    }

    /// Whether the user has any grant in the profile: its owner, a live member of a live group, or
    /// someone who can read one of its folders (`has_grant` of the server).
    fn has_grant(&self, user: Id, profile: Id) -> bool {
        if self.st.users.get(&user).is_some_and(|(p, _)| *p == profile) {
            return true;
        }
        let live_group = self
            .st
            .rows
            .get(&RowRef {
                kind: RowKind::Profile,
                id: profile,
            })
            .is_some_and(|g| !deleted(&g.row));
        if live_group && self.role(user, profile).is_some() {
            return true;
        }
        self.st.rows.iter().any(|(k, s)| {
            matches!(&s.row, AnyRow::Folder(f) if f.profile_id == profile && f.meta.deleted_at.is_none())
                && self.can_read(user, *k)
        })
    }

    /// May the user create this row (§4.3)?
    fn may_create(&self, user: Id, row: &AnyRow) -> bool {
        match row {
            AnyRow::Folder(f) => match f.parent_id {
                None => self.manages(user, f.profile_id),
                Some(p) => self.edit_on(user, p),
            },
            AnyRow::Snippet(s) => self.edit_on(user, s.folder_id),
            AnyRow::Tag(t) => {
                self.manages(user, t.profile_id) || self.has_edit_in(user, t.profile_id)
            }
            AnyRow::Attachment(a) => {
                self.manages(user, a.profile_id) || self.has_edit_in(user, a.profile_id)
            }
            AnyRow::SnippetTag(l) => self
                .snippet(l.snippet_id)
                .is_some_and(|s| self.edit_on(user, s.folder_id)),
            AnyRow::SnippetAttachment(l) => self
                .snippet(l.snippet_id)
                .is_some_and(|s| self.edit_on(user, s.folder_id)),
            _ => false,
        }
    }

    /// May the user change `cur` into `new` (§4.3)?
    fn may_change(&self, user: Id, cur: &AnyRow, new: &AnyRow) -> bool {
        match (cur, new) {
            (AnyRow::Snippet(a), AnyRow::Snippet(b)) => {
                if a.folder_id == b.folder_id {
                    return self.edit_on(user, a.folder_id);
                }
                let (ga, gb) = (
                    self.permission(user, a.folder_id),
                    self.permission(user, b.folder_id),
                );
                match (ga, gb) {
                    (Some(x), Some(y)) => {
                        let edit =
                            |g: &super::Grant| matches!(g.access, Access::Edit | Access::Manage);
                        edit(&x)
                            && edit(&y)
                            && (x.root == y.root
                                || (x.access == Access::Manage && y.access == Access::Manage))
                    }
                    _ => false,
                }
            }
            (AnyRow::Folder(a), AnyRow::Folder(b)) => {
                if a.parent_id.is_none() || self.live_share_root(a.meta.id) {
                    return self.manage_on(user, a.meta.id);
                }
                if a.member_permission != b.member_permission {
                    return self.manage_on(user, a.meta.id);
                }
                match b.parent_id {
                    None => self.manages(user, a.profile_id),
                    Some(np) => {
                        let here = self.permission(user, a.meta.id);
                        let there = self.permission(user, np);
                        match (here, there) {
                            (Some(x), Some(y)) => {
                                let edit = |g: &super::Grant| {
                                    matches!(g.access, Access::Edit | Access::Manage)
                                };
                                edit(&x)
                                    && edit(&y)
                                    && (x.root == y.root || y.access == Access::Manage)
                            }
                            _ => false,
                        }
                    }
                }
            }
            (AnyRow::Tag(t), _) => self.manages(user, t.profile_id),
            (AnyRow::SnippetTag(l), _) => self
                .snippet(l.snippet_id)
                .is_some_and(|s| self.edit_on(user, s.folder_id)),
            (AnyRow::SnippetAttachment(l), _) => self
                .snippet(l.snippet_id)
                .is_some_and(|s| self.edit_on(user, s.folder_id)),
            (AnyRow::Attachment(a), _) => self.manages(user, a.profile_id),
            _ => false,
        }
    }

    /// May the user restore `version` over `cur` (C20)?
    pub(super) fn may_restore(&self, user: Id, cur: &AnyRow, version: &AnyRow) -> bool {
        if version.meta().deleted_at.is_some() {
            return self.may_delete(user, cur)
                || matches!(cur, AnyRow::Folder(f) if self.manage_on(user, f.meta.id));
        }
        if cur.meta().deleted_at.is_some() {
            return self.may_create(user, version) || self.may_change(user, cur, version);
        }
        self.may_change(user, cur, version)
    }

    fn may_delete(&self, user: Id, cur: &AnyRow) -> bool {
        match cur {
            AnyRow::Snippet(s) => self.edit_on(user, s.folder_id),
            AnyRow::Tag(t) => self.manages(user, t.profile_id),
            AnyRow::Attachment(a) => self.manages(user, a.profile_id),
            AnyRow::SnippetTag(l) => self
                .snippet(l.snippet_id)
                .is_some_and(|s| self.edit_on(user, s.folder_id)),
            AnyRow::SnippetAttachment(l) => self
                .snippet(l.snippet_id)
                .is_some_and(|s| self.edit_on(user, s.folder_id)),
            _ => false,
        }
    }

    /// C13 and C8: the rows a new or changed row refers to exist, are readable, and are in the
    /// row's profile. Answers the outcome that refuses it, if any.
    fn check_refs(&self, user: Id, row: &AnyRow, profile: Id) -> Option<Outcome> {
        let readable = |k: RowRef| self.st.rows.contains_key(&k) && self.can_read(user, k);
        match row {
            AnyRow::Snippet(s) => {
                let k = RowRef {
                    kind: RowKind::Folder,
                    id: s.folder_id,
                };
                if !readable(k) {
                    return Some(Outcome::reason("rejected", "parent_gone"));
                }
                if self
                    .folder(s.folder_id)
                    .is_some_and(|f| f.profile_id != profile)
                {
                    return Some(Outcome::reason("blocked", "invalid"));
                }
            }
            AnyRow::Folder(f) => {
                if f.profile_id != profile {
                    return Some(Outcome::reason("blocked", "invalid"));
                }
                if let Some(p) = f.parent_id {
                    let k = RowRef {
                        kind: RowKind::Folder,
                        id: p,
                    };
                    if !readable(k) {
                        return Some(Outcome::reason("rejected", "parent_gone"));
                    }
                    if self.folder(p).is_some_and(|x| x.profile_id != profile) {
                        return Some(Outcome::reason("blocked", "invalid"));
                    }
                }
            }
            AnyRow::SnippetTag(l) => {
                let (s, t) = (
                    RowRef {
                        kind: RowKind::Snippet,
                        id: l.snippet_id,
                    },
                    RowRef {
                        kind: RowKind::Tag,
                        id: l.tag_id,
                    },
                );
                if !self.st.rows.contains_key(&s) || !self.st.rows.contains_key(&t) {
                    return Some(Outcome::reason("rejected", "parent_gone"));
                }
                let tp = match &self.st.rows[&t].row {
                    AnyRow::Tag(x) => x.profile_id,
                    _ => profile,
                };
                if tp != profile {
                    return Some(Outcome::reason("blocked", "invalid"));
                }
            }
            AnyRow::SnippetAttachment(l) => {
                let (s, a) = (
                    RowRef {
                        kind: RowKind::Snippet,
                        id: l.snippet_id,
                    },
                    RowRef {
                        kind: RowKind::Attachment,
                        id: l.attachment_id,
                    },
                );
                if !self.st.rows.contains_key(&s) || !self.st.rows.contains_key(&a) {
                    return Some(Outcome::reason("rejected", "parent_gone"));
                }
                let ap = match &self.st.rows[&a].row {
                    AnyRow::Attachment(x) => x.profile_id,
                    _ => profile,
                };
                if ap != profile {
                    return Some(Outcome::reason("blocked", "invalid"));
                }
            }
            AnyRow::Tag(t) if t.profile_id != profile => {
                return Some(Outcome::reason("blocked", "invalid"));
            }
            AnyRow::Attachment(a) if a.profile_id != profile => {
                return Some(Outcome::reason("blocked", "invalid"));
            }
            _ => {}
        }
        None
    }

    /// The folder a live row needs live: a snippet's folder, a folder's parent.
    fn needs_live(row: &AnyRow) -> Option<Id> {
        match row {
            AnyRow::Snippet(s) => Some(s.folder_id),
            AnyRow::Folder(f) => f.parent_id,
            _ => None,
        }
    }

    /// C17: who wrote the content of the row a put writes.
    #[allow(clippy::too_many_arguments, reason = "the parts of C17's rule")]
    fn content_author(
        &self,
        user: Id,
        device: Id,
        key: RowRef,
        sent: &AnyRow,
        cur: Option<&Stored>,
        base: u64,
        follows: &[Id],
    ) -> (Option<Id>, Option<i64>) {
        let Some(cur) = cur else {
            return (Some(user), Some(self.now));
        };
        if content_eq(sent, &cur.row) && !deleted(&cur.row) {
            return (cur.content_by, cur.content_at);
        }
        // The base version (4.3, finding 11): a history version the writer could not read while
        // it was the row cannot be found, and `follows` names this device's own ops only.
        let bv = if base > 0 {
            if cur.row.meta().rev == base {
                Some(cur.clone())
            } else {
                self.st
                    .history
                    .iter()
                    .find(|h| h.key == key && h.rev == base)
                    .filter(|h| self.version_readable(user, &h.stored.row, h.replaced_by))
                    .map(|h| h.stored.clone())
            }
        } else {
            self.version_of_follows(key, follows)
                .filter(|v| v.updated_device == Some(device))
        };
        match bv {
            Some(v) if content_eq(sent, &v.row) => (v.content_by, v.content_at),
            Some(_) => (Some(user), Some(self.now)),
            None => (None, None),
        }
    }

    /// Whether a write over `cur` records a conflict (C3).
    fn conflicts_with(
        &self,
        device: Id,
        op: &Op,
        cur: &Stored,
        base: u64,
        sent_live: Option<&AnyRow>,
    ) -> bool {
        let stale = base < cur.row.meta().rev;
        let own = cur.updated_device == Some(device)
            && cur
                .updated_op
                .is_some_and(|o| o == op.op_id || op.follows.contains(&o));
        if !stale || own {
            return false;
        }
        match self.version_at(cur.row.key(), base) {
            Some(bv) => !recorded_eq(&bv.row, &cur.row) || deleted(&bv.row) != deleted(&cur.row),
            None => match sent_live {
                Some(s) => !recorded_eq(s, &cur.row) || deleted(&cur.row),
                None => !deleted(&cur.row),
            },
        }
    }

    /// An ordinary put or delete of a folder, snippet, tag, attachment or link.
    #[allow(clippy::too_many_lines)]
    fn push_row(
        &mut self,
        tx: &mut Tx,
        c: Caller,
        device: Id,
        op: &Op,
        profile: Id,
        base: u64,
    ) -> Outcome {
        let key = op.key;
        let cur = self.st.rows.get(&key).cloned();
        let Some(sent) = op.row.clone() else {
            // A delete (C5). One of a row the caller cannot read answers as one of an unknown id.
            let Some(cur) = cur else {
                return Outcome::new("unchanged");
            };
            if !self.can_read(c.user, key) {
                return Outcome::new("unchanged");
            }
            if deleted(&cur.row) {
                return Outcome::new("unchanged");
            }
            if !self.may_delete(c.user, &cur.row) {
                return Outcome::reason("rejected", "forbidden");
            }
            let conflict = self.conflicts_with(device, op, &cur, base, None);
            let rev = self.next_rev(profile);
            let mut row = cur.row.clone();
            let m = row.meta_mut();
            m.rev = rev;
            m.deleted_at = Some(Timestamp(self.now));
            m.updated_at = Timestamp(self.now);
            m.updated_by = Some(c.user);
            let new = Stored {
                row,
                updated_device: Some(device),
                updated_op: Some(op.op_id),
                ..cur.clone()
            };
            self.write(tx, key, new);
            let mut out = Outcome::new("applied");
            if conflict {
                out.conflict = Some(self.record_conflict(
                    tx,
                    key,
                    "delete_over_edit",
                    rev,
                    c.user,
                    Some(device),
                    &cur,
                ));
            }
            return out;
        };
        // A put.
        if let Some(o) = self.check_refs(c.user, &sent, profile) {
            return o;
        }
        let mut sent = sent;
        let mut adjusted = Vec::new();
        match &cur {
            None => {
                if base > 0 {
                    return Outcome::reason("rejected", "gone");
                }
                if !self.may_create(c.user, &sent) {
                    return Outcome::reason("rejected", "forbidden");
                }
                // C8: a new folder that would be a fifth level goes up.
                if let AnyRow::Folder(f) = &mut sent
                    && let Some(p) = f.parent_id
                {
                    let chain = self.chain(p);
                    if chain.len() >= 4 {
                        let keep = chain.len() - 3;
                        f.parent_id = chain.get(keep).copied();
                        adjusted.push("parent_id".to_owned());
                    }
                }
            }
            Some(cur) => {
                if cur.row.kind() == RowKind::Attachment {
                    return Outcome::new("unchanged");
                }
                if fields_eq(&sent, &cur.row) && !deleted(&cur.row) {
                    return Outcome::new("unchanged");
                }
                let allowed = if deleted(&cur.row) {
                    self.may_create(c.user, &sent) || self.may_change(c.user, &cur.row, &sent)
                } else {
                    self.may_change(c.user, &cur.row, &sent)
                };
                if !allowed {
                    return Outcome::reason("rejected", "forbidden");
                }
                // C8: a move that would make a cycle or a fifth level keeps the current parent.
                if let (AnyRow::Folder(f), AnyRow::Folder(old)) = (&mut sent, &cur.row)
                    && f.parent_id != old.parent_id
                    && let Some(np) = f.parent_id
                {
                    let chain = self.chain(np);
                    if chain.contains(&f.meta.id) || chain.len() + 1 + self.height(f.meta.id) > 4 {
                        f.parent_id = old.parent_id;
                        adjusted.push("parent_id".to_owned());
                    }
                }
            }
        }
        // C8 put the tree right and left nothing else changed: nothing new, nothing written.
        if !adjusted.is_empty()
            && let Some(cv) = &cur
            && fields_eq(&sent, &cv.row)
            && !deleted(&cv.row)
        {
            let mut out = Outcome::new("unchanged");
            out.adjusted = adjusted;
            return out;
        }
        // C7: a live row needs live folders.
        if let Some(f) = Self::needs_live(&sent)
            && self.folder(f).is_some_and(|x| x.meta.deleted_at.is_some())
            && !self.restore_up(tx, c.user, device, op.op_id, f)
        {
            return Outcome::reason("rejected", "parent_gone");
        }
        let conflict = cur
            .as_ref()
            .is_some_and(|cv| self.conflicts_with(device, op, cv, base, Some(&sent)));
        let (content_by, content_at) =
            self.content_author(c.user, device, key, &sent, cur.as_ref(), base, &op.follows);
        let rev = self.next_rev(profile);
        let mut new = self.new_stored(sent, rev, c.user, device, op.op_id);
        if let Some(cv) = &cur {
            new.created_rev = cv.created_rev;
            new.row.meta_mut().created_at = cv.row.meta().created_at;
            new.lost_rev = cv.lost_rev;
        } else {
            new.row.meta_mut().created_at = Timestamp(op.created_at.min(self.now));
        }
        if key.kind == RowKind::Snippet {
            new.content_by = content_by;
            new.content_at = content_at;
        }
        self.write(tx, key, new);
        let mut out = Outcome::new("applied");
        out.adjusted = adjusted;
        if conflict && let Some(cv) = &cur {
            let kind = if deleted(&cv.row) {
                "edit_over_delete"
            } else {
                "edit_edit"
            };
            if matches!(key.kind, RowKind::Snippet | RowKind::Folder | RowKind::Tag) {
                out.conflict =
                    Some(self.record_conflict(tx, key, kind, rev, c.user, Some(device), cv));
            }
        }
        out
    }

    /// C11: shares never win by being late.
    fn push_share(&mut self, tx: &mut Tx, c: Caller, device: Id, op: &Op) -> Outcome {
        let key = op.key;
        let cur = self.st.rows.get(&key).cloned();
        let Some(sent) = op.row.clone() else {
            let Some(cur) = cur else {
                return Outcome::new("unchanged");
            };
            if !self.can_read(c.user, key) || deleted(&cur.row) {
                return Outcome::new("unchanged");
            }
            let AnyRow::FolderShare(x) = &cur.row else {
                return Outcome::reason("blocked", "invalid");
            };
            if !self.manage_on(c.user, x.folder_id) {
                return Outcome::reason("rejected", "forbidden");
            }
            let profile = self
                .folder(x.folder_id)
                .map_or(x.profile_id, |f| f.profile_id);
            self.lock_group(x.profile_id);
            let rev = self.next_rev(profile);
            let mut row = cur.row.clone();
            let m = row.meta_mut();
            m.rev = rev;
            m.deleted_at = Some(Timestamp(self.now));
            m.updated_at = Timestamp(self.now);
            m.updated_by = Some(c.user);
            let (group, share) = (x.profile_id, x.meta.id);
            self.write(
                tx,
                key,
                Stored {
                    row,
                    updated_device: Some(device),
                    updated_op: Some(op.op_id),
                    ..cur
                },
            );
            self.log.push(LogLine {
                at: self.now,
                call: "share_delete".into(),
                user: c.user,
                group: Some(group),
                subject: Some(share),
                args: json!({}),
            });
            return Outcome::new("applied");
        };
        let AnyRow::FolderShare(x) = &sent else {
            return Outcome::reason("blocked", "invalid");
        };
        let folder = self.folder(x.folder_id).cloned();
        let personal = folder.as_ref().is_some_and(|f| {
            self.st
                .rows
                .get(&RowRef { kind: RowKind::Profile, id: f.profile_id })
                .is_some_and(|p| matches!(&p.row, AnyRow::Profile(pp) if pp.kind == iamb_core::model::ProfileKind::Personal))
        });
        let role = self.role(c.user, x.profile_id);
        let company = matches!(
            self.st.rows.get(&RowRef { kind: RowKind::Profile, id: x.profile_id }).map(|s| &s.row),
            Some(AnyRow::Profile(p)) if p.kind == iamb_core::model::ProfileKind::Company
        );
        let ok = folder.is_some()
            && personal
            && self.manage_on(c.user, x.folder_id)
            && role.is_some()
            && (!company || matches!(role.as_deref(), Some("owner" | "admin")));
        if !ok {
            return Outcome::reason("rejected", "forbidden");
        }
        // A share of a deleted folder would give access to tombstones only, and C6 ended the
        // folder's shares with it: refused (proposal to C11).
        if folder.as_ref().is_some_and(|f| f.meta.deleted_at.is_some()) {
            return Outcome::reason("rejected", "parent_gone");
        }
        let profile = folder.as_ref().map_or(x.profile_id, |f| f.profile_id);
        // A share a device gives back after a restore (C18) is judged against the row as the
        // restored server has it: the device could not have seen anything written since.
        let base = match (&op.repush, &cur) {
            (Some(_), Some(cv)) => op.base.max(cv.row.meta().rev),
            (Some(_), None) => 0,
            (None, _) => op.base,
        };
        if let Some(cv) = &cur {
            if base < cv.row.meta().rev {
                return Outcome::new("superseded");
            }
            if fields_eq(&sent, &cv.row) && !deleted(&cv.row) {
                return Outcome::new("unchanged");
            }
        } else if base > 0 {
            return Outcome::reason("rejected", "gone");
        }
        if let Some(limit) = self.st.limits.get("shared_folders_max").copied() {
            let n = self
                .st
                .rows
                .values()
                .filter_map(|s| match &s.row {
                    AnyRow::FolderShare(y)
                        if y.meta.deleted_at.is_none() && y.folder_id != x.folder_id =>
                    {
                        self.folder(y.folder_id)
                            .filter(|f| f.profile_id == profile)
                            .map(|f| f.meta.id)
                    }
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .len();
            if i64::try_from(n).unwrap_or(i64::MAX) >= limit {
                return Outcome::reason("rejected", "plan_limit");
            }
        }
        self.lock_group(x.profile_id);
        let (group, share) = (x.profile_id, x.meta.id);
        let rev = self.next_rev(profile);
        let mut new = self.new_stored(sent, rev, c.user, device, op.op_id);
        if let Some(cv) = &cur {
            new.created_rev = cv.created_rev;
            new.row.meta_mut().created_at = cv.row.meta().created_at;
        }
        self.write(tx, key, new);
        self.log.push(LogLine {
            at: self.now,
            call: "share_created".into(),
            user: c.user,
            group: Some(group),
            subject: Some(share),
            args: json!({}),
        });
        Outcome::new("applied")
    }

    /// Records a group's `head_rev` as touched by a share (the lock of C24).
    fn lock_group(&mut self, group: Id) {
        let _ = group;
    }

    /// C6: a folder's delete takes the subtree as the server has it.
    #[allow(clippy::too_many_lines)]
    fn push_folder_delete(
        &mut self,
        tx: &mut Tx,
        c: Caller,
        device: Id,
        op: &Op,
        profile: Id,
    ) -> Outcome {
        let key = op.key;
        let Some(cur) = self.st.rows.get(&key).cloned() else {
            let mut o = Outcome::new("unchanged");
            o.cascade = Some(self.cascade_answer(c, &op.cascade));
            return o;
        };
        if !self.can_read(c.user, key) {
            let mut o = Outcome::new("unchanged");
            o.cascade = Some(self.cascade_answer(c, &op.cascade));
            return o;
        }
        if deleted(&cur.row) {
            let mut o = Outcome::new("unchanged");
            o.cascade = Some(self.cascade_answer(c, &op.cascade));
            return o;
        }
        let AnyRow::Folder(f) = &cur.row else {
            return Outcome::reason("blocked", "invalid");
        };
        let fid = f.meta.id;
        let subtree: Vec<Id> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::Folder(x)
                    if x.meta.deleted_at.is_none()
                        && x.meta.id != fid
                        && self.chain(x.meta.id).contains(&fid) =>
                {
                    Some(x.meta.id)
                }
                _ => None,
            })
            .collect();
        let mut allowed = match f.parent_id {
            None => self.manages(c.user, f.profile_id),
            Some(p) => self.edit_on(c.user, p),
        };
        if self.live_share_root(fid) && !self.manage_on(c.user, fid) {
            allowed = false;
        }
        for s in &subtree {
            if self.live_share_root(*s) && !self.manage_on(c.user, *s) {
                allowed = false;
            }
        }
        if !allowed {
            let mut o = Outcome::reason("rejected", "forbidden");
            o.cascade = Some(self.cascade_answer(c, &op.cascade));
            return o;
        }
        let mut all_folders = subtree.clone();
        all_folders.push(fid);
        let list: std::collections::BTreeMap<RowRef, u64> = op.cascade.iter().copied().collect();
        // Snippets, then folders deepest first.
        let mut victims: Vec<RowRef> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::Snippet(x)
                    if x.meta.deleted_at.is_none() && all_folders.contains(&x.folder_id) =>
                {
                    Some(s.row.key())
                }
                _ => None,
            })
            .collect();
        let mut subs = subtree.clone();
        subs.sort_by_key(|x| std::cmp::Reverse(self.chain(*x).len()));
        victims.extend(subs.iter().map(|x| RowRef {
            kind: RowKind::Folder,
            id: *x,
        }));
        if victims.len() > self.limits.cascade_max {
            return Outcome::new("partial");
        }
        for vk in victims {
            let Some(v) = self.st.rows.get(&vk).cloned() else {
                continue;
            };
            let vrev = v.row.meta().rev;
            let other = v.updated_device != Some(device)
                || v.updated_op
                    .is_none_or(|o| o != op.op_id && !op.follows.contains(&o));
            let conflict = other
                && match list.get(&vk) {
                    None => true,
                    Some(b) if *b < vrev => self
                        .version_at(vk, *b)
                        .is_none_or(|bv| !recorded_eq(&bv.row, &v.row)),
                    Some(_) => false,
                };
            let rev = self.next_rev(profile);
            let mut row = v.row.clone();
            let m = row.meta_mut();
            m.rev = rev;
            m.deleted_at = Some(Timestamp(self.now));
            m.updated_at = Timestamp(self.now);
            m.updated_by = Some(c.user);
            self.write(
                tx,
                vk,
                Stored {
                    row,
                    updated_device: Some(device),
                    updated_op: Some(op.op_id),
                    ..v.clone()
                },
            );
            if conflict {
                self.record_conflict(tx, vk, "deleted_with_folder", rev, c.user, Some(device), &v);
            }
        }
        // The shares of the folder and of every folder below it end with it.
        let shares: Vec<(RowRef, Id, Id)> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::FolderShare(x)
                    if x.meta.deleted_at.is_none() && all_folders.contains(&x.folder_id) =>
                {
                    Some((s.row.key(), x.profile_id, x.meta.id))
                }
                _ => None,
            })
            .collect();
        for (sk, group, sid) in shares {
            let Some(v) = self.st.rows.get(&sk).cloned() else {
                continue;
            };
            let rev = self.next_rev(profile);
            let mut row = v.row.clone();
            let m = row.meta_mut();
            m.rev = rev;
            m.deleted_at = Some(Timestamp(self.now));
            m.updated_at = Timestamp(self.now);
            m.updated_by = Some(c.user);
            self.write(
                tx,
                sk,
                Stored {
                    row,
                    updated_device: Some(device),
                    updated_op: Some(op.op_id),
                    ..v
                },
            );
            self.log.push(LogLine {
                at: self.now,
                call: "share_delete".into(),
                user: c.user,
                group: Some(group),
                subject: Some(sid),
                args: json!({}),
            });
        }
        let conflict = self.conflicts_with(device, op, &cur, op.base, None);
        let rev = self.next_rev(profile);
        let mut row = cur.row.clone();
        let m = row.meta_mut();
        m.rev = rev;
        m.deleted_at = Some(Timestamp(self.now));
        m.updated_at = Timestamp(self.now);
        m.updated_by = Some(c.user);
        self.write(
            tx,
            key,
            Stored {
                row,
                updated_device: Some(device),
                updated_op: Some(op.op_id),
                ..cur.clone()
            },
        );
        let mut out = Outcome::new("applied");
        if conflict {
            out.conflict = Some(self.record_conflict(
                tx,
                key,
                "delete_over_edit",
                rev,
                c.user,
                Some(device),
                &cur,
            ));
        }
        out.cascade = Some(self.cascade_answer(c, &op.cascade));
        out
    }

    /// C18: a re-push after a restore.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn push_repush(
        &mut self,
        tx: &mut Tx,
        c: Caller,
        device: Id,
        op: &Op,
        profile: Id,
        kind: &str,
        rp_rev: u64,
        rp_epoch: &str,
    ) -> Outcome {
        let idx = self.st.epochs.iter().position(|e| e.id == rp_epoch);
        let ep = idx
            .and_then(|i| self.st.epochs.get(i + 1))
            .unwrap_or_else(|| self.epoch())
            .clone();
        let backup = ep.backup_rev.get(&profile).copied().unwrap_or(0);
        let in_span = rp_rev > backup && rp_rev < ep.first_rev;
        if !in_span {
            return self.push_row(tx, c, device, op, profile, rp_rev);
        }
        let key = op.key;
        let cur = self.st.rows.get(&key).cloned();
        if let Some(sent) = &op.row
            && let Some(o) = self.check_refs(c.user, sent, profile)
        {
            return o;
        }
        let permitted = match (&cur, &op.row) {
            (None, Some(s)) => self.may_create(c.user, s),
            (Some(cv), Some(s)) => {
                self.may_create(c.user, s) || self.may_change(c.user, &cv.row, s)
            }
            (Some(cv), None) => {
                self.may_delete(c.user, &cv.row)
                    || matches!(&cv.row, AnyRow::Folder(f) if self.manage_on(c.user, f.meta.id))
            }
            (None, None) => true,
        };
        if !permitted {
            return Outcome::reason("rejected", "forbidden");
        }
        let author = |s: &Self, cur: Option<&Stored>, sent: &AnyRow| -> (Option<Id>, Option<i64>) {
            match cur {
                Some(cv) if content_eq(sent, &cv.row) && !deleted(&cv.row) => {
                    (cv.content_by, cv.content_at)
                }
                _ => {
                    if op.author_self {
                        (Some(c.user), Some(s.now))
                    } else {
                        (None, None)
                    }
                }
            }
        };
        let build = |s: &mut Self, cur: Option<&Stored>| -> Stored {
            let rev = s.next_rev(profile);
            let mut st = match &op.row {
                Some(r) => s.new_stored(r.clone(), rev, c.user, device, op.op_id),
                None => {
                    let mut st = cur.cloned().unwrap_or_else(|| {
                        s.new_stored(
                            AnyRow::Tag(iamb_core::model::Tag {
                                meta: iamb_core::model::SyncMeta::new(key.id, Timestamp(s.now)),
                                profile_id: profile,
                                name: String::new(),
                                color: None,
                            }),
                            rev,
                            c.user,
                            device,
                            op.op_id,
                        )
                    });
                    let m = st.row.meta_mut();
                    m.rev = rev;
                    m.deleted_at = Some(Timestamp(s.now));
                    m.updated_at = Timestamp(s.now);
                    m.updated_by = Some(c.user);
                    st.updated_device = Some(device);
                    st.updated_op = Some(op.op_id);
                    st
                }
            };
            if let Some(cv) = cur {
                st.created_rev = cv.created_rev;
                st.row.meta_mut().created_at = cv.row.meta().created_at;
            }
            if let (Some(r), RowKind::Snippet) = (&op.row, key.kind) {
                let (b, a) = author(s, cur, r);
                st.content_by = b;
                st.content_at = a;
            }
            st.lost_rev = Some(rp_rev.max(cur.and_then(|cv| cv.lost_rev).unwrap_or(0)));
            st
        };
        match cur {
            None => {
                // Case 4: unknown to the server.
                if self.now - ep.started_at > self.limits.retention_ms {
                    return Outcome::reason("rejected", "gone");
                }
                if op.row.is_none() {
                    return Outcome::new("unchanged");
                }
                if let Some(f) = op.row.as_ref().and_then(Self::needs_live)
                    && self.folder(f).is_some_and(|x| x.meta.deleted_at.is_some())
                    && !self.restore_up(tx, c.user, device, op.op_id, f)
                {
                    return Outcome::reason("rejected", "parent_gone");
                }
                let new = build(self, None);
                self.write(tx, key, new);
                Outcome::new("applied")
            }
            Some(cv) => {
                let cur_rev = cv.row.meta().rev;
                let same = match &op.row {
                    Some(r) => fields_eq(r, &cv.row) && !deleted(&cv.row),
                    None => deleted(&cv.row),
                };
                if kind == "row" && cv.lost_rev.is_some_and(|l| l >= rp_rev) {
                    // Case 3: an earlier version of the same lost history.
                    if same {
                        return Outcome::new("unchanged");
                    }
                    if !self
                        .st
                        .history
                        .iter()
                        .any(|h| h.key == key && h.rev == rp_rev)
                        && let Some(r) = &op.row
                    {
                        let mut st = self.new_stored(r.clone(), rp_rev, c.user, device, op.op_id);
                        st.created_rev = cv.created_rev;
                        self.st.history.push(Hist {
                            key,
                            rev: rp_rev,
                            stored: st.clone(),
                            replaced_by: cur_rev,
                        });
                        let differs = !recorded_eq(r, &cv.row);
                        if differs && kind == "row" && cv.updated_device != Some(device) {
                            self.touch(tx, key);
                            self.record_conflict(
                                tx,
                                key,
                                "edit_edit",
                                cur_rev,
                                cv.row.meta().updated_by.unwrap_or(c.user),
                                cv.updated_device,
                                &st,
                            );
                        }
                    }
                    return Outcome::new("superseded");
                }
                if same && cur_rev <= backup {
                    let mut st = cv.clone();
                    st.lost_rev = Some(rp_rev);
                    self.st.rows.insert(key, st);
                    return Outcome::new("unchanged");
                }
                if let Some(f) = op.row.as_ref().and_then(Self::needs_live)
                    && self.folder(f).is_some_and(|x| x.meta.deleted_at.is_some())
                    && !self.restore_up(tx, c.user, device, op.op_id, f)
                {
                    return Outcome::reason("rejected", "parent_gone");
                }
                // A change made on top of the lost revision that the row now is (given back by
                // another device) is based on it knowingly: no conflict.
                let written_since =
                    cur_rev > backup && !(kind == "base" && cv.lost_rev == Some(rp_rev));
                let new = build(self, Some(&cv));
                let rev = new.row.meta().rev;
                self.write(tx, key, new);
                let mut out = Outcome::new("applied");
                // Case 2: written since the restore: the re-push wins, and a conflict offers the
                // version it replaces when they differ.
                if written_since {
                    let differs = match &op.row {
                        Some(r) => !recorded_eq(r, &cv.row) || deleted(&cv.row),
                        None => !deleted(&cv.row),
                    };
                    if differs
                        && matches!(key.kind, RowKind::Snippet | RowKind::Folder | RowKind::Tag)
                    {
                        let kind = match (&op.row, deleted(&cv.row)) {
                            (None, _) => "delete_over_edit",
                            (Some(_), true) => "edit_over_delete",
                            (Some(_), false) => "edit_edit",
                        };
                        out.conflict = Some(self.record_conflict(
                            tx,
                            key,
                            kind,
                            rev,
                            c.user,
                            Some(device),
                            &cv,
                        ));
                    }
                }
                out
            }
        }
    }
}
