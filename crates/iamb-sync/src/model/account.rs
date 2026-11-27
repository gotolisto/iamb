//! The account calls of the model (§5.14, C16, C20), and backups, restores and the operator's
//! replay (C18).

use std::collections::BTreeMap;

use iamb_core::model::{
    Id, Membership, Permission, Profile, ProfileKind, Role, SyncMeta, Timestamp,
};
use iamb_core::sync::{AnyRow, RowKind, RowRef};
use serde_json::{Value, json};

use super::{Caller, Epoch, LogLine, Server, Stored, Tx, arg_id};

/// The calls that take access away, which the operator's replay makes again (C18).
pub const REMOVALS: [&str; 8] = [
    "remove_member",
    "leave_group",
    "withdraw_invitation",
    "set_member_role",
    "transfer_ownership",
    "delete_group",
    "revoke_device",
    "share_delete",
];

impl Server {
    fn ok(&self, mark: Option<u64>) -> Value {
        json!({ "error": null, "epoch": self.epoch().id, "mark": mark })
    }

    fn logged(
        &mut self,
        call: &str,
        user: Id,
        group: Option<Id>,
        subject: Option<Id>,
        args: &Value,
    ) {
        self.log.push(LogLine {
            at: self.now,
            call: call.to_owned(),
            user,
            group,
            subject,
            args: args.clone(),
        });
    }

    fn membership(&self, group: Id, user: Id) -> Option<Stored> {
        self.st
            .rows
            .values()
            .find(|s| matches!(&s.row, AnyRow::Membership(m) if m.profile_id == group && m.user_id == user))
            .cloned()
    }

    /// Writes a membership row (new, changed or ended) under the group's lock.
    fn put_membership(
        &mut self,
        tx: &mut Tx,
        by: Id,
        group: Id,
        user: Id,
        role: Option<Role>,
    ) -> u64 {
        let rev = self.next_rev(group);
        let existing = self.membership(group, user);
        let id = existing
            .as_ref()
            .map_or_else(|| self.new_id(), |s| s.row.id());
        let key = RowRef {
            kind: RowKind::Membership,
            id,
        };
        self.touch(tx, key);
        // A new member's feed gets the group's profile and every live membership (§5.5).
        let others: Vec<RowRef> = self
            .st
            .rows
            .values()
            .filter(|s| matches!(&s.row, AnyRow::Membership(m) if m.profile_id == group && m.meta.deleted_at.is_none()))
            .map(|s| s.row.key())
            .chain(std::iter::once(RowRef {
                kind: RowKind::Profile,
                id: group,
            }))
            .collect();
        for k in others {
            self.touch(tx, k);
        }
        let created = existing
            .as_ref()
            .map_or(Timestamp(self.now), |s| s.row.meta().created_at);
        let row = AnyRow::Membership(Membership {
            meta: SyncMeta {
                id,
                created_at: created,
                updated_at: Timestamp(self.now),
                updated_by: Some(by),
                rev,
                deleted_at: if role.is_none() {
                    Some(Timestamp(self.now))
                } else {
                    None
                },
            },
            profile_id: group,
            user_id: user,
            role: role.unwrap_or(Role::Member),
        });
        let st = Stored {
            row,
            created_rev: existing.as_ref().map_or(rev, |s| s.created_rev),
            updated_device: None,
            updated_op: None,
            lost_rev: None,
            content_by: None,
            content_at: None,
            backed: false,
        };
        if let Some(old) = self.st.rows.insert(key, st) {
            self.st.history.push(super::Hist {
                key,
                rev: old.row.meta().rev,
                stored: old,
                replaced_by: rev,
            });
        }
        rev
    }

    /// Ends a membership: the shares from that person's folders into the group end with it (C16).
    fn end_membership(&mut self, tx: &mut Tx, by: Id, group: Id, user: Id) -> u64 {
        let shares: Vec<RowRef> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::FolderShare(x) if x.profile_id == group && x.meta.deleted_at.is_none() => {
                    let owner = self.folder(x.folder_id).and_then(|f| {
                        self.st
                            .users
                            .iter()
                            .find(|(_, (p, _))| *p == f.profile_id)
                            .map(|(u, _)| *u)
                    });
                    (owner == Some(user)).then(|| s.row.key())
                }
                _ => None,
            })
            .collect();
        for k in shares {
            self.tombstone(tx, k, by);
        }
        self.put_membership(tx, by, group, user, None)
    }

    pub(super) fn tombstone(&mut self, tx: &mut Tx, key: RowRef, by: Id) {
        let Some(cur) = self.st.rows.get(&key).cloned() else {
            return;
        };
        if cur.row.meta().deleted_at.is_some() {
            return;
        }
        self.touch(tx, key);
        let profile = match &cur.row {
            AnyRow::FolderShare(x) => self
                .folder(x.folder_id)
                .map_or(x.profile_id, |f| f.profile_id),
            AnyRow::Snippet(s) => self
                .folder(s.folder_id)
                .map_or(s.folder_id, |f| f.profile_id),
            r => r.own_profile().unwrap_or(key.id),
        };
        let rev = self.next_rev(profile);
        let mut row = cur.row.clone();
        let m = row.meta_mut();
        m.rev = rev;
        m.deleted_at = Some(Timestamp(self.now));
        m.updated_at = Timestamp(self.now);
        m.updated_by = Some(by);
        self.st.rows.insert(
            key,
            Stored {
                row,
                updated_device: None,
                updated_op: None,
                ..cur.clone()
            },
        );
        self.st.history.push(super::Hist {
            key,
            rev: cur.row.meta().rev,
            stored: cur,
            replaced_by: rev,
        });
    }

    fn email_of(&self, user: Id) -> String {
        self.st
            .users
            .get(&user)
            .map(|u| u.1.clone())
            .unwrap_or_default()
    }

    /// Account calls (§5.14) as `c.user`.
    #[allow(clippy::too_many_lines)]
    pub(super) fn account_call(&mut self, c: Caller, function: &str, args: &Value) -> Value {
        let group = arg_id(args, "group");
        let target = arg_id(args, "user");
        let manager =
            |s: &Self, g: Id| matches!(s.role(c.user, g).as_deref(), Some("owner" | "admin"));
        match function {
            "create_group" => {
                let (Some(id), kind) =
                    (arg_id(args, "id"), args.get("kind").and_then(Value::as_str))
                else {
                    return self.err("invalid");
                };
                if let Some(p) = self.st.rows.get(&RowRef {
                    kind: RowKind::Profile,
                    id,
                }) {
                    // A repeat answers as the first call did, for a member only: after leaving,
                    // the id is in use (4.3).
                    if self.role(c.user, id).is_none() {
                        return self.err("invalid");
                    }
                    let mark = self
                        .st
                        .head_rev
                        .get(&id)
                        .copied()
                        .unwrap_or(p.row.meta().rev);
                    return self.ok(Some(mark));
                }
                let mut tx = self.begin();
                let rev = self.next_rev(id);
                let key = RowRef {
                    kind: RowKind::Profile,
                    id,
                };
                self.touch(&mut tx, key);
                let row = AnyRow::Profile(Profile {
                    meta: SyncMeta {
                        id,
                        created_at: Timestamp(self.now),
                        updated_at: Timestamp(self.now),
                        updated_by: Some(c.user),
                        rev,
                        deleted_at: None,
                    },
                    kind: if kind == Some("company") {
                        ProfileKind::Company
                    } else {
                        ProfileKind::Family
                    },
                    name: args
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    owner_user_id: c.user,
                });
                self.st.rows.insert(
                    key,
                    Stored {
                        row,
                        created_rev: rev,
                        updated_device: None,
                        updated_op: None,
                        lost_rev: None,
                        content_by: None,
                        content_at: None,
                        backed: false,
                    },
                );
                let mark = self.put_membership(&mut tx, c.user, id, c.user, Some(Role::Owner));
                self.commit(tx);
                self.logged(function, c.user, Some(id), None, args);
                self.ok(Some(mark))
            }
            "invite" => {
                let (Some(id), Some(g)) = (arg_id(args, "id"), group) else {
                    return self.err("invalid");
                };
                if self.role(c.user, g).is_none() {
                    // Not a member: one answer for a group that exists and one that does not.
                    return self.err("not_found");
                }
                if !manager(self, g) {
                    return self.err("forbidden");
                }
                let email = args
                    .get("email")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                if self.st.invitations.contains_key(&id) {
                    return self.ok(self.st.head_rev.get(&g).copied());
                }
                let member = self
                    .st
                    .users
                    .iter()
                    .any(|(u, (_, e))| *e == email && self.role(*u, g).is_some());
                if member {
                    return self.err("already_member");
                }
                if self
                    .st
                    .invitations
                    .values()
                    .any(|i| i.group == g && i.email == email && i.status == "pending")
                {
                    return self.err("already_invited");
                }
                self.st.invitations.insert(
                    id,
                    super::Invitation {
                        group: g,
                        email,
                        role: args
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("member")
                            .to_owned(),
                        status: "pending".into(),
                    },
                );
                let mark = self.next_rev(g);
                self.ok(Some(mark))
            }
            "accept_invitation" => {
                let Some(id) = arg_id(args, "id") else {
                    return self.err("invalid");
                };
                let my = self.email_of(c.user);
                let Some(inv) = self.st.invitations.get(&id).cloned() else {
                    return self.err("invitation_invalid");
                };
                if inv.email != my || inv.status == "withdrawn" || inv.status == "declined" {
                    return self.err("invitation_invalid");
                }
                if inv.status == "accepted" && self.role(c.user, inv.group).is_none() {
                    // Accepted before, and the membership has ended since (4.3).
                    return self.err("invitation_invalid");
                }
                if self.role(c.user, inv.group).is_some() {
                    if let Some(i) = self.st.invitations.get_mut(&id) {
                        i.status = "accepted".into();
                    }
                    return self.ok(self.st.head_rev.get(&inv.group).copied());
                }
                let mut tx = self.begin();
                let role = if inv.role == "admin" {
                    Role::Admin
                } else {
                    Role::Member
                };
                let mark = self.put_membership(&mut tx, c.user, inv.group, c.user, Some(role));
                if let Some(i) = self.st.invitations.get_mut(&id) {
                    i.status = "accepted".into();
                }
                self.commit(tx);
                self.logged(function, c.user, Some(inv.group), Some(c.user), args);
                self.ok(Some(mark))
            }
            "decline_invitation" => {
                let Some(id) = arg_id(args, "id") else {
                    return self.err("invalid");
                };
                match self.st.invitations.get_mut(&id) {
                    Some(i) if i.status == "pending" => {
                        i.status = "declined".into();
                        self.ok(None)
                    }
                    _ => self.err("invitation_invalid"),
                }
            }
            "withdraw_invitation" => {
                let Some(id) = arg_id(args, "id") else {
                    return self.err("invalid");
                };
                let Some(inv) = self.st.invitations.get(&id).cloned() else {
                    return self.err("not_found");
                };
                if self.role(c.user, inv.group).is_none() {
                    // Not a member: one answer for a group that exists and one that does not.
                    return self.err("not_found");
                }
                if !manager(self, inv.group) {
                    return self.err("forbidden");
                }
                match inv.status.as_str() {
                    "accepted" => self.err("already_accepted"),
                    "withdrawn" => self.ok(self.st.head_rev.get(&inv.group).copied()),
                    _ => {
                        if let Some(i) = self.st.invitations.get_mut(&id) {
                            i.status = "withdrawn".into();
                        }
                        let mark = self.next_rev(inv.group);
                        self.logged(function, c.user, Some(inv.group), Some(id), args);
                        self.ok(Some(mark))
                    }
                }
            }
            "remove_member" => {
                let (Some(g), Some(u)) = (group, target) else {
                    return self.err("invalid");
                };
                if self.role(c.user, g).is_none() {
                    // Not a member: one answer for a group that exists and one that does not.
                    return self.err("not_found");
                }
                if !manager(self, g) {
                    return self.err("forbidden");
                }
                match self.role(u, g).as_deref() {
                    None => {
                        if self.membership(g, u).is_some() {
                            return self.ok(self.st.head_rev.get(&g).copied());
                        }
                        self.err("not_a_member")
                    }
                    Some("owner") => self.err("owner_role"),
                    Some(_) => {
                        let mut tx = self.begin();
                        let mark = self.end_membership(&mut tx, c.user, g, u);
                        self.commit(tx);
                        self.logged(function, c.user, Some(g), Some(u), args);
                        self.ok(Some(mark))
                    }
                }
            }
            "leave_group" => {
                let Some(g) = group else {
                    return self.err("invalid");
                };
                match self.role(c.user, g).as_deref() {
                    None => self.err("not_a_member"),
                    Some("owner") => self.err("owner_cannot_leave"),
                    Some(_) => {
                        let mut tx = self.begin();
                        let mark = self.end_membership(&mut tx, c.user, g, c.user);
                        self.commit(tx);
                        self.logged(function, c.user, Some(g), Some(c.user), args);
                        self.ok(Some(mark))
                    }
                }
            }
            "set_member_role" => {
                let (Some(g), Some(u)) = (group, target) else {
                    return self.err("invalid");
                };
                if self.role(c.user, g).is_none() {
                    // Not a member: one answer for a group that exists and one that does not.
                    return self.err("not_found");
                }
                if !manager(self, g) {
                    return self.err("forbidden");
                }
                let role = args.get("role").and_then(Value::as_str).unwrap_or("member");
                match self.role(u, g).as_deref() {
                    None => self.err("not_a_member"),
                    Some("owner") => self.err("owner_role"),
                    Some(r) if r == role => self.ok(self.st.head_rev.get(&g).copied()),
                    Some(_) => {
                        let mut tx = self.begin();
                        let mark = self.put_membership(
                            &mut tx,
                            c.user,
                            g,
                            u,
                            Some(if role == "admin" {
                                Role::Admin
                            } else {
                                Role::Member
                            }),
                        );
                        self.commit(tx);
                        self.logged(function, c.user, Some(g), Some(u), args);
                        self.ok(Some(mark))
                    }
                }
            }
            "transfer_ownership" => {
                let Some(g) = group else {
                    return self.err("invalid");
                };
                if self.role(c.user, g).as_deref() != Some("owner") {
                    return self.err("not_owner");
                }
                match target {
                    None => {
                        self.st.offers.remove(&g);
                        let mark = self.next_rev(g);
                        self.logged(function, c.user, Some(g), None, args);
                        self.ok(Some(mark))
                    }
                    Some(u) if self.role(u, g).is_some() => {
                        self.st.offers.insert(g, u);
                        let mark = self.next_rev(g);
                        self.ok(Some(mark))
                    }
                    Some(_) => self.err("not_a_member"),
                }
            }
            "accept_ownership" => {
                let Some(g) = group else {
                    return self.err("invalid");
                };
                if self.st.offers.get(&g) != Some(&c.user) || self.role(c.user, g).is_none() {
                    return self.err("no_offer");
                }
                let old = self.st.rows.values().find_map(|s| match &s.row {
                    AnyRow::Profile(p) if p.meta.id == g => Some(p.owner_user_id),
                    _ => None,
                });
                let mut tx = self.begin();
                if let Some(o) = old {
                    self.put_membership(&mut tx, c.user, g, o, Some(Role::Admin));
                }
                let mark = self.put_membership(&mut tx, c.user, g, c.user, Some(Role::Owner));
                let key = RowRef {
                    kind: RowKind::Profile,
                    id: g,
                };
                self.touch(&mut tx, key);
                if let Some(Stored {
                    row: AnyRow::Profile(p),
                    ..
                }) = self.st.rows.get_mut(&key)
                {
                    p.owner_user_id = c.user;
                    p.meta.rev = mark;
                }
                self.st.offers.remove(&g);
                self.commit(tx);
                self.ok(Some(mark))
            }
            "delete_group" => {
                let Some(g) = group else {
                    return self.err("invalid");
                };
                if self.role(c.user, g).as_deref() != Some("owner") {
                    return self.err("not_owner");
                }
                let mut tx = self.begin();
                let members: Vec<Id> = self
                    .st
                    .rows
                    .values()
                    .filter_map(|s| match &s.row {
                        AnyRow::Membership(m)
                            if m.profile_id == g && m.meta.deleted_at.is_none() =>
                        {
                            Some(m.user_id)
                        }
                        _ => None,
                    })
                    .collect();
                let shares: Vec<RowRef> = self
                    .st
                    .rows
                    .values()
                    .filter(|s| matches!(&s.row, AnyRow::FolderShare(x) if x.profile_id == g && x.meta.deleted_at.is_none()))
                    .map(|s| s.row.key())
                    .collect();
                for k in shares {
                    self.tombstone(&mut tx, k, c.user);
                }
                // The group's own rows: tombstones without feed entries (nobody can read them).
                let own: Vec<RowRef> = self
                    .st
                    .rows
                    .values()
                    .filter(|s| match &s.row {
                        AnyRow::Folder(f) => f.profile_id == g && f.meta.deleted_at.is_none(),
                        AnyRow::Snippet(sn) => {
                            sn.meta.deleted_at.is_none()
                                && self.folder(sn.folder_id).is_some_and(|f| f.profile_id == g)
                        }
                        _ => false,
                    })
                    .map(|s| s.row.key())
                    .collect();
                let mut mark = 0;
                for u in members {
                    mark = self.put_membership(&mut tx, c.user, g, u, None);
                }
                let before = tx.before.clone();
                for k in own {
                    self.tombstone(&mut tx, k, c.user);
                }
                tx.before = before;
                let key = RowRef {
                    kind: RowKind::Profile,
                    id: g,
                };
                self.tombstone(&mut tx, key, c.user);
                self.commit(tx);
                self.logged(function, c.user, Some(g), None, args);
                self.ok(Some(mark))
            }
            "revoke_device" => {
                let Some(t) = arg_id(args, "target") else {
                    return self.err("invalid");
                };
                match self.st.devices.get_mut(&t) {
                    Some(d) if d.user == c.user => {
                        d.revoked = true;
                        self.logged(function, c.user, None, Some(t), args);
                        self.ok(None)
                    }
                    _ => self.err("not_found"),
                }
            }
            "resolve_conflict" => self.resolve_conflict(c, args),
            "get_conflict" => self.get_conflict(c, args),
            "conflicts" => {
                let list: Vec<Value> = self
                    .st
                    .conflicts
                    .iter()
                    .filter(|x| x.state == "open" && self.party_reads(c.user, x))
                    .map(|x| json!({ "id": x.id, "entity": x.key.kind.as_str(), "row": x.key.id, "kind": x.kind }))
                    .collect();
                json!({ "error": null, "epoch": self.epoch().id, "conflicts": list })
            }
            "devices" => {
                let list: Vec<Value> = self
                    .st
                    .devices
                    .iter()
                    .filter(|(_, d)| d.user == c.user)
                    .map(|(id, d)| json!({ "id": id, "revoked": d.revoked }))
                    .collect();
                json!({ "error": null, "epoch": self.epoch().id, "devices": list })
            }
            _ => self.err("not_found"),
        }
    }

    /// A party of the conflict who can read its row now (ADR-0031): anyone else is told
    /// `not_found`, as for a conflict that does not exist.
    fn party_reads(&self, user: Id, x: &super::Conflict) -> bool {
        let party = x.winner_user == user || x.versions.iter().any(|v| v.user == Some(user));
        party && self.can_read(user, x.key)
    }

    /// `version_readable` of the server (C3, C20, ADR-0031): the user wrote the version, or had
    /// access to the folder it was in (a folder: to itself) from before it was replaced; for a
    /// tag, owns its personal profile or has been a member of its group since before then.
    pub(super) fn version_readable(&self, user: Id, row: &AnyRow, replaced_by: u64) -> bool {
        if row.meta().updated_by == Some(user) {
            return true;
        }
        // A live version was the row until it was replaced. A deleted one (the tombstone a
        // restore replaced) was readable only for an access that began before that delete,
        // which is the version's own revision (§5.4).
        let replaced_by = if row.meta().deleted_at.is_some() {
            replaced_by.min(row.meta().rev)
        } else {
            replaced_by
        };
        let since = |f: Id| {
            self.st
                .granted_rev
                .get(&(user, f))
                .is_some_and(|r| *r < replaced_by)
        };
        match row {
            AnyRow::Snippet(s) => since(s.folder_id),
            AnyRow::Folder(f) => since(f.meta.id),
            AnyRow::Tag(t) => {
                self.st
                    .users
                    .get(&user)
                    .is_some_and(|(p, _)| *p == t.profile_id)
                    || self.st.rows.values().any(|s| {
                        matches!(&s.row, AnyRow::Membership(m)
                            if m.profile_id == t.profile_id && m.user_id == user
                                && m.meta.deleted_at.is_none())
                            && s.created_rev < replaced_by
                    })
            }
            _ => false,
        }
    }

    /// `get_conflict`: the conflict, the current row and the versions on offer to the caller.
    fn get_conflict(&self, c: Caller, args: &Value) -> Value {
        let Some(id) = arg_id(args, "id") else {
            return self.err("invalid");
        };
        let Some(x) = self.st.conflicts.iter().find(|x| x.id == id) else {
            return self.err("not_found");
        };
        if !self.party_reads(c.user, x) {
            return self.err("not_found");
        }
        let versions: Vec<Value> = x
            .versions
            .iter()
            .filter_map(|v| {
                let h = self
                    .st
                    .history
                    .iter()
                    .find(|h| h.key == x.key && h.rev == v.rev)?;
                self.version_readable(c.user, &h.stored.row, h.replaced_by)
                    .then(|| {
                        json!({ "rev": v.rev, "user": v.user, "display_name": null,
                                "device": if v.user == Some(c.user) { json!(v.device) } else { Value::Null },
                                "taken_over": v.taken_over, "deleted": h.stored.row.meta().deleted_at.is_some() })
                    })
            })
            .collect();
        json!({ "error": null, "epoch": self.epoch().id,
                "conflict": { "id": x.id, "entity": x.key.kind.as_str(), "row_id": x.key.id,
                              "kind": x.kind, "state": x.state, "winner_rev": x.winner_rev,
                              "winner_user": x.winner_user },
                "versions": versions })
    }

    /// C20: keep the current version, or restore an offered one as a new revision. A party who
    /// cannot read the row is told `not_found`, also for "keep"; a version not on offer to the
    /// caller is `invalid`; a restore the caller may not write is `forbidden` and leaves the
    /// conflict open (ADR-0031).
    fn resolve_conflict(&mut self, c: Caller, args: &Value) -> Value {
        let Some(id) = arg_id(args, "id") else {
            return self.err("invalid");
        };
        let Some(pos) = self.st.conflicts.iter().position(|x| x.id == id) else {
            return self.err("not_found");
        };
        let conflict = self.st.conflicts[pos].clone();
        if !self.party_reads(c.user, &conflict) {
            return self.err("not_found");
        }
        if conflict.state != "open" {
            return json!({ "error": null, "epoch": self.epoch().id, "state": conflict.state });
        }
        let action = args.get("action").and_then(Value::as_str);
        if action == Some("restore") {
            let rev = args.get("rev").and_then(Value::as_u64).unwrap_or(0);
            if !conflict.versions.iter().any(|v| v.rev == rev) {
                return self.err("invalid");
            }
            let Some(h) = self
                .st
                .history
                .iter()
                .find(|h| h.key == conflict.key && h.rev == rev)
                .cloned()
            else {
                return self.err("gone");
            };
            if !self.version_readable(c.user, &h.stored.row, h.replaced_by) {
                return self.err("invalid");
            }
            // C20: restoring needs the right to write the row now, judged as a push of that
            // version by this user would be (§4.3).
            let cur = self.st.rows.get(&conflict.key).cloned();
            let allowed = match &cur {
                Some(cv) => self.may_restore(c.user, &cv.row, &h.stored.row),
                None => return self.err("gone"),
            };
            if !allowed {
                return self.err("forbidden");
            }
            let mut tx = self.begin();
            let profile = match &h.stored.row {
                AnyRow::Snippet(s) => self
                    .folder(s.folder_id)
                    .map_or(s.folder_id, |f| f.profile_id),
                r => r.own_profile().unwrap_or(conflict.key.id),
            };
            let new_rev = self.next_rev(profile);
            let cur = self.st.rows.get(&conflict.key).cloned();
            let mut st = h.stored.clone();
            let m = st.row.meta_mut();
            m.rev = new_rev;
            m.updated_at = Timestamp(self.now);
            m.updated_by = Some(c.user);
            if let Some(cv) = &cur {
                st.created_rev = cv.created_rev;
            }
            self.touch(&mut tx, conflict.key);
            if let Some(old) = self.st.rows.insert(conflict.key, st) {
                self.st.history.push(super::Hist {
                    key: conflict.key,
                    rev: old.row.meta().rev,
                    stored: old,
                    replaced_by: new_rev,
                });
            }
            self.commit(tx);
        } else if action != Some("keep") {
            return self.err("invalid");
        }
        self.st.conflicts[pos].state = "resolved".into();
        self.ok(None)
    }

    // ---- C18 --------------------------------------------------------------------------------

    /// Takes a backup of the database.
    pub fn backup(&mut self, name: &str) {
        self.backups
            .insert(name.to_owned(), (self.st.clone(), self.now));
    }

    /// Restores a backup and begins a new epoch (the operator's script, steps 1 and 3; step 2 is
    /// [`replay_calls`](Self::replay_calls)). `not_before` is the time of the last line of the
    /// function log.
    pub fn restore(&mut self, name: &str, not_before: Option<i64>) -> bool {
        let Some((st, _)) = self.backups.get(name).cloned() else {
            return false;
        };
        self.st = st;
        let t = self.now.max(not_before.unwrap_or(0));
        let by_clock = u64::try_from(t.max(0)).unwrap_or(0).saturating_mul(1024);
        let highest = self
            .st
            .head_rev
            .values()
            .copied()
            .max()
            .unwrap_or(0)
            .max(self.st.rev_seq);
        let first_rev = by_clock.max(highest + 1);
        let backup_rev: BTreeMap<Id, u64> = self
            .st
            .rows
            .values()
            .filter_map(|s| match &s.row {
                AnyRow::Profile(p) => Some((
                    p.meta.id,
                    self.st.head_rev.get(&p.meta.id).copied().unwrap_or(0),
                )),
                _ => None,
            })
            .collect();
        let id = self.new_id().to_string();
        self.st.epochs.push(Epoch {
            id,
            started_at: t,
            first_rev,
            backup_rev,
        });
        self.st.rev_seq = first_rev - 1;
        for feed in self.st.feeds.values_mut() {
            feed.entries.clear();
            feed.floor = feed.head;
        }
        for d in self.st.devices.values_mut() {
            d.position = None;
            d.position_before = None;
        }
        true
    }

    /// The operator's replay (C18 step 2): every logged call after `since` that took access away
    /// is made again as its caller, unless the restored state no longer lets that caller make it
    /// or the log shows the same access given back afterwards. Answers what it made again.
    pub fn replay_calls(&mut self, since: i64) -> Vec<String> {
        let lines: Vec<LogLine> = self.log.iter().filter(|l| l.at > since).cloned().collect();
        let mut made = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            if !REMOVALS.contains(&l.call.as_str()) {
                continue;
            }
            let given_back = lines[i + 1..].iter().any(|later| match l.call.as_str() {
                "share_delete" => later.call == "share_created" && later.subject == l.subject,
                "remove_member" | "leave_group" => {
                    later.call == "accept_invitation"
                        && later.group == l.group
                        && later.subject == l.subject
                }
                "set_member_role" => {
                    later.call == "set_member_role"
                        && later.group == l.group
                        && later.subject == l.subject
                }
                _ => false,
            });
            if given_back {
                continue;
            }
            let session = self.st.sessions.iter().next().copied().unwrap_or(l.user);
            let caller = Caller {
                user: l.user,
                session,
            };
            let answer = if l.call == "share_delete" {
                let Some(sid) = l.subject else { continue };
                let key = RowRef {
                    kind: RowKind::FolderShare,
                    id: sid,
                };
                let live = self
                    .st
                    .rows
                    .get(&key)
                    .is_some_and(|s| s.row.meta().deleted_at.is_none());
                let manage = match self.st.rows.get(&key).map(|s| &s.row) {
                    Some(AnyRow::FolderShare(x)) => self
                        .permission(l.user, x.folder_id)
                        .is_some_and(|g| g.access == iamb_core::sync::Access::Manage),
                    _ => false,
                };
                if live && manage {
                    let mut tx = self.begin();
                    self.tombstone(&mut tx, key, l.user);
                    self.commit(tx);
                    json!({ "error": null })
                } else {
                    json!({ "error": "skipped" })
                }
            } else {
                self.account_call(caller, &l.call, &l.args)
            };
            if answer.get("error").is_some_and(Value::is_null) {
                made.push(l.call.clone());
            }
        }
        made
    }

    /// Sets the protocol range (C23).
    pub fn set_protocol(&mut self, min: u32, max: u32) {
        self.st.protocol = (min, max);
    }

    /// Ends every session of the backup that a restore does not know: removes `session`.
    pub fn end_session(&mut self, session: Id) {
        self.st.sessions.remove(&session);
    }

    /// The permission a share names, for building share rows in tests.
    #[must_use]
    pub const fn share_permission(edit: bool) -> Permission {
        if edit {
            Permission::Edit
        } else {
            Permission::View
        }
    }
}
