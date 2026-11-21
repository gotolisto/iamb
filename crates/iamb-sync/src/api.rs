//! The call layer: every function of the server's `api` schema the agent calls, with its exact
//! arguments and answer (`server/supabase/migrations/*.sql`; `docs/sync-design.md` §5.14, §6).
//!
//! This is the only module that knows the JSON shapes. A [`Transport`] carries one call (the
//! function's name and its arguments as a JSON object) and returns the JSON object the function
//! answered; [`crate::http`] does that over HTTPS, the model of the server
//! (`crate::model`, feature `model`) in process. When the server's functions change (the security
//! review of task 4.3 may change some), this module changes and nothing else.
//!
//! Every answer is one object with `error` (null or a code) and `epoch`. A refusal is data, not a
//! transport error ([`Refusal`]); a call that got no answer is a [`CallError`].

use std::collections::BTreeMap;
use std::fmt;

use iamb_core::model::{Id, Timestamp};
use iamb_core::sync::{
    Access, AccessRoot, AnyRow, CascadeResult, Change, LostLine, OpResult, OpStatus, Origin,
    RemoteRow, RepushKind, RowKind, RowRef,
};
use serde_json::{Value, json};

/// A call that got no usable answer (§6.1): retried with backoff, never acted on as an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The system reports no network.
    Offline,
    /// The connection failed or broke before an answer arrived.
    Network(String),
    /// No answer within the time-out (40 s, above every function's own).
    Timeout,
    /// HTTP 401: the access token is no longer good.
    Unauthorized,
    /// HTTP 5xx, or another status that is not an answer.
    Status(u16),
    /// The answer was not the JSON object the function returns.
    Malformed(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offline => f.write_str("offline"),
            Self::Network(e) => write!(f, "network error: {e}"),
            Self::Timeout => f.write_str("time-out"),
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::Status(s) => write!(f, "HTTP status {s}"),
            Self::Malformed(e) => write!(f, "malformed answer: {e}"),
        }
    }
}

impl std::error::Error for CallError {}

/// Carries one call of a function of `api` and returns its answer object.
pub trait Transport {
    /// Calls `function` with `args` (a JSON object of named arguments).
    ///
    /// # Errors
    ///
    /// [`CallError`] when no answer object arrived.
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError>;
}

impl<T: Transport + ?Sized> Transport for &mut T {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError> {
        (**self).call(function, args)
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError> {
        (**self).call(function, args)
    }
}

/// A function's refusal: `error` was not null (§6.1, §5.14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The code (`reset_required`, `device_conflict`, `forbidden`, …).
    pub code: String,
    /// `reason` (`expired` or `epoch` with `reset_required`).
    pub reason: Option<String>,
    /// `retry_after_ms` with `rate_limited`.
    pub retry_after_ms: Option<u64>,
    /// The server's current epoch, which every answer names.
    pub epoch: Option<String>,
}

/// Why a call did not give what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No answer.
    Call(CallError),
    /// An answer that refuses.
    Refused(Refusal),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Call(e) => e.fmt(f),
            Self::Refused(r) => write!(f, "refused: {}", r.code),
        }
    }
}

impl From<CallError> for Failure {
    fn from(e: CallError) -> Self {
        Self::Call(e)
    }
}

/// The result of one call.
pub type Reply<T> = Result<T, Failure>;

fn malformed(what: &str) -> Failure {
    Failure::Call(CallError::Malformed(what.to_owned()))
}

/// Splits an answer into its refusal or its body.
fn answer(v: Value) -> Reply<Value> {
    let Value::Object(_) = &v else {
        return Err(malformed("the answer is not an object"));
    };
    match v.get("error") {
        None | Some(Value::Null) => Ok(v),
        Some(e) => Err(Failure::Refused(Refusal {
            code: e.as_str().unwrap_or("unknown").to_owned(),
            reason: v.get("reason").and_then(Value::as_str).map(str::to_owned),
            retry_after_ms: v.get("retry_after_ms").and_then(Value::as_u64),
            epoch: v.get("epoch").and_then(Value::as_str).map(str::to_owned),
        })),
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn id_of(v: &Value, key: &str) -> Option<Id> {
    v.get(key).and_then(Value::as_str).and_then(|s| s.parse().ok())
}

fn u64_of(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// A row in the server's wire form (§6.3, `iamb_private.wire_of`) as a model row.
///
/// # Errors
///
/// The row does not decode as `entity`.
pub fn row_from_wire(entity: &str, row: &Value) -> Result<AnyRow, String> {
    let kind = RowKind::parse(entity).ok_or_else(|| format!("unknown entity {entity}"))?;
    let tagged = json!({ "entity": kind.as_str(), "row": row });
    serde_json::from_value(tagged).map_err(|e| format!("{entity}: {e}"))
}

/// A model row in the wire form a `push` takes (the model's serde form, which is the server's).
#[must_use]
pub fn row_to_wire(row: &AnyRow) -> Value {
    serde_json::to_value(row)
        .ok()
        .and_then(|mut v| v.get_mut("row").map(Value::take))
        .unwrap_or(Value::Null)
}

fn origin_from(v: Option<&Value>) -> Option<Origin> {
    let v = v?;
    if !v.is_object() || v.get("created_rev").is_none() {
        return None;
    }
    Some(Origin {
        content_by: id_of(v, "content_by"),
        content_at: v.get("content_at").and_then(Value::as_i64).map(Timestamp),
        created_rev: u64_of(v, "created_rev"),
    })
}

/// One `{entity, id, row, origin}` item of a page as a [`RemoteRow`].
fn remote_row(item: &Value) -> Reply<RemoteRow> {
    let entity = item
        .get("entity")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("row without entity"))?;
    let kind = RowKind::parse(entity).ok_or_else(|| malformed("unknown entity"))?;
    let id = id_of(item, "id").ok_or_else(|| malformed("row without id"))?;
    let row = match item.get("row") {
        None | Some(Value::Null) => None,
        Some(r) => Some(row_from_wire(entity, r).map_err(|e| malformed(&e))?),
    };
    Ok(RemoteRow {
        key: RowRef { kind, id },
        row,
        origin: origin_from(item.get("origin")),
    })
}

fn remote_rows(v: Option<&Value>) -> Reply<Vec<RemoteRow>> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(remote_row).collect())
        .unwrap_or_else(|| Ok(Vec::new()))
}

/// The device as `hello` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The device id.
    pub id: Id,
    /// A name the user can change ("Mac"); the host name is never sent.
    pub name: String,
    /// `macos`, `windows` or `linux`.
    pub platform: String,
    /// The app's version.
    pub app_version: String,
}

/// `hello`'s answer (§6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The protocol version agreed.
    pub protocol: u32,
    /// The account.
    pub user: Id,
    /// The account's personal profile.
    pub personal_profile: Id,
    /// The server's epoch.
    pub epoch: String,
    /// The head of the user's feed.
    pub head: u64,
    /// `push_ops`, `pull_entries`, … as the server states them.
    pub limits: BTreeMap<String, u64>,
}

/// One entry of a pulled page (§6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A row changed, or became readable or unreadable.
    Row(RemoteRow),
    /// A tree became readable.
    FolderVisible(Id),
    /// A tree stopped being readable, except the listed parts.
    FolderRevoked(Id, Vec<Id>),
    /// A conflict the user is party to changed.
    Conflict(Option<Id>),
    /// The plan changed.
    Entitlements,
    /// An invitation changed.
    Invitations,
    /// The account changed.
    Account,
    /// Attachment bytes were verified.
    Blob,
    /// A kind this build does not know (ignored, §6.10).
    Other(String),
}

/// A pulled page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The position to store with the page's effects.
    pub next: String,
    /// The feed's head.
    pub head: u64,
    /// Ask again at once.
    pub more: bool,
    /// The entries, in feed order.
    pub entries: Vec<Entry>,
    /// The rows the entries refer to that are not in the page (closed under references).
    pub refs: Vec<RemoteRow>,
    /// The epoch.
    pub epoch: Option<String>,
}

/// A page of a whole read (`snapshot` or `fetch_folder`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WholePage {
    /// The rows.
    pub rows: Vec<RemoteRow>,
    /// The cursor of the next page, if any.
    pub cursor: Option<String>,
    /// `snapshot`'s first page: the position at the feed's head.
    pub start: Option<String>,
    /// The feed's head when the first page was read.
    pub head: Option<u64>,
    /// `snapshot` after a restore: what the restore lost.
    pub lost: Option<LostLine>,
    /// The epoch.
    pub epoch: Option<String>,
}

/// The answer of `push` (§6.3), one result per op in the order sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    /// The feed's head after the request.
    pub head: u64,
    /// The results, matched to the changes sent.
    pub results: Vec<OpResult>,
}

/// The answer of `access` (§6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessAnswer {
    /// The access roots outside the personal profile.
    pub roots: Vec<AccessRoot>,
    /// The user's role in each group.
    pub groups: BTreeMap<Id, String>,
}

/// The calls, for one device speaking one protocol version.
pub struct Api<'a, T: Transport + ?Sized> {
    transport: &'a mut T,
    device: Id,
    protocol: u32,
}

impl<'a, T: Transport + ?Sized> Api<'a, T> {
    /// The calls of `device` speaking `protocol`.
    pub const fn new(transport: &'a mut T, device: Id, protocol: u32) -> Self {
        Self {
            transport,
            device,
            protocol,
        }
    }

    fn call(&mut self, function: &str, mut args: Value) -> Reply<Value> {
        if let Value::Object(m) = &mut args {
            m.insert("device".into(), json!(self.device));
            m.insert("protocol".into(), json!(self.protocol));
        }
        answer(self.transport.call(function, args)?)
    }

    /// `hello(device, protocols, max_rev)` (§6.2).
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn hello(
        transport: &mut T,
        device: &DeviceInfo,
        protocols: (u32, u32),
        max_rev: u64,
    ) -> Reply<Hello> {
        let args = json!({
            "device": { "id": device.id, "name": device.name, "platform": device.platform,
                        "app_version": device.app_version },
            "protocols": [protocols.0, protocols.1],
            "max_rev": max_rev,
        });
        let v = answer(transport.call("hello", args)?)?;
        Ok(Hello {
            protocol: u32::try_from(u64_of(&v, "protocol")).unwrap_or(1),
            user: id_of(&v, "user").ok_or_else(|| malformed("hello without user"))?,
            personal_profile: id_of(&v, "personal_profile")
                .ok_or_else(|| malformed("hello without personal_profile"))?,
            epoch: str_of(&v, "epoch").unwrap_or_default(),
            head: u64_of(&v, "head"),
            limits: v
                .get("limits")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n)))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// `push(device, protocol, position, ops)` (§6.3). The results are matched to `changes` by
    /// order, and carry each change's outbox entries.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn push(&mut self, position: &str, changes: &[Change]) -> Reply<Pushed> {
        let ops: Vec<Value> = changes.iter().map(op_to_wire).collect();
        let v = self.call("push", json!({ "position": position, "ops": ops }))?;
        let results = v
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("push without results"))?;
        if results.len() != changes.len() {
            return Err(malformed("push answered another number of results"));
        }
        let mut out = Vec::with_capacity(changes.len());
        for (r, c) in results.iter().zip(changes) {
            out.push(result_from_wire(r, c)?);
        }
        Ok(Pushed {
            head: u64_of(&v, "head"),
            results: out,
        })
    }

    /// `pull(device, protocol, after, limit)` (§6.4).
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn pull(&mut self, after: &str, limit: u32) -> Reply<Page> {
        let v = self.call("pull", json!({ "after": after, "limit": limit }))?;
        let mut entries = Vec::new();
        for e in v.get("entries").and_then(Value::as_array).into_iter().flatten() {
            let kind = e.get("kind").and_then(Value::as_str).unwrap_or("");
            entries.push(match kind {
                "row" => Entry::Row(remote_row(e)?),
                "folder_visible" => Entry::FolderVisible(
                    id_of(e, "folder").ok_or_else(|| malformed("folder_visible"))?,
                ),
                "folder_revoked" => Entry::FolderRevoked(
                    id_of(e, "folder").ok_or_else(|| malformed("folder_revoked"))?,
                    e.get("except")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|x| x.as_str()?.parse().ok()).collect())
                        .unwrap_or_default(),
                ),
                "conflict" => Entry::Conflict(id_of(e, "conflict")),
                "entitlements" => Entry::Entitlements,
                "invitations" => Entry::Invitations,
                "account" => Entry::Account,
                "blob" => Entry::Blob,
                other => Entry::Other(other.to_owned()),
            });
        }
        Ok(Page {
            next: str_of(&v, "next").ok_or_else(|| malformed("pull without next"))?,
            head: u64_of(&v, "head"),
            more: v.get("more").and_then(Value::as_bool).unwrap_or(false),
            entries,
            refs: remote_rows(v.get("refs"))?,
            epoch: str_of(&v, "epoch"),
        })
    }

    /// `rows(device, protocol, refs)` (§6.5, D15): up to 500 rows, each as the caller may read it
    /// now, or null.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn rows(&mut self, refs: &[RowRef]) -> Reply<Vec<RemoteRow>> {
        let refs: Vec<Value> = refs
            .iter()
            .map(|r| json!({ "entity": r.kind.as_str(), "id": r.id }))
            .collect();
        let v = self.call("rows", json!({ "refs": refs }))?;
        remote_rows(v.get("rows"))
    }

    /// `fetch_folder(device, protocol, folder, cursor)` (§6.5).
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn fetch_folder(&mut self, folder: Id, cursor: Option<&str>) -> Reply<WholePage> {
        let v = self.call("fetch_folder", json!({ "folder": folder, "cursor": cursor }))?;
        whole_page(&v)
    }

    /// `snapshot(device, protocol, position, cursor)` (§6.5).
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn snapshot(&mut self, position: Option<&str>, cursor: Option<&str>) -> Reply<WholePage> {
        let v = self.call("snapshot", json!({ "position": position, "cursor": cursor }))?;
        whole_page(&v)
    }

    /// `visible_folders(device, protocol)` (D7, D12).
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn visible_folders(&mut self) -> Reply<Vec<Id>> {
        let v = self.call("visible_folders", json!({}))?;
        Ok(v.get("folders")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str()?.parse().ok()).collect())
            .unwrap_or_default())
    }

    /// `access(device, protocol)` (§6.5).
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn access(&mut self) -> Reply<AccessAnswer> {
        let v = self.call("access", json!({}))?;
        let roots = v
            .get("roots")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        Some(AccessRoot {
                            folder_id: id_of(r, "root")?,
                            profile_id: id_of(r, "profile")?,
                            permission: Access::parse(r.get("permission")?.as_str()?)?,
                            via: str_of(r, "via").unwrap_or_default(),
                            granted_at: Timestamp(
                                r.get("granted_at").and_then(Value::as_i64).unwrap_or(0),
                            ),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let groups = v
            .get("groups")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|g| Some((id_of(g, "group")?, str_of(g, "role")?)))
                    .collect()
            })
            .unwrap_or_default();
        Ok(AccessAnswer { roots, groups })
    }

    /// `entitlements(device, protocol)` (§6.12): the flat map.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn entitlements(&mut self) -> Reply<BTreeMap<String, String>> {
        let v = self.call("entitlements", json!({}))?;
        let map = v
            .get("entitlements")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_str().map_or_else(|| v.to_string(), str::to_owned),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(map)
    }

    /// An account call (§5.14) by name with its own arguments; answers `mark` when it has one.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn account_call(&mut self, function: &str, args: Value) -> Reply<Option<u64>> {
        let v = self.call(function, args)?;
        Ok(v.get("mark").and_then(Value::as_u64))
    }

    /// Any other call by name; the answer object as it came.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn raw(&mut self, function: &str, args: Value) -> Reply<Value> {
        self.call(function, args)
    }
}

fn whole_page(v: &Value) -> Reply<WholePage> {
    let lost = v.get("lost").filter(|l| l.is_object()).map(|l| LostLine {
        epoch: str_of(l, "epoch").unwrap_or_default(),
        started_at: Timestamp(l.get("started_at").and_then(Value::as_i64).unwrap_or(0)),
        device_epoch: String::new(),
        profiles: l
            .get("profiles")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|p| Some((id_of(p, "profile")?, u64_of(p, "backup_rev"))))
                    .collect()
            })
            .unwrap_or_default(),
    });
    Ok(WholePage {
        rows: remote_rows(v.get("rows"))?,
        cursor: str_of(v, "cursor"),
        start: str_of(v, "start"),
        head: v.get("head").and_then(Value::as_u64),
        lost,
        epoch: str_of(v, "epoch"),
    })
}

/// A change as an op of `push` (§6.3).
#[must_use]
pub fn op_to_wire(c: &Change) -> Value {
    let mut op = json!({
        "op_id": c.op_id,
        "follows": c.follows,
        "entity": c.kind.as_str(),
        "id": c.id,
        "kind": if c.is_delete() { "delete" } else { "put" },
        "base_rev": c.base_rev,
    });
    if let (Some(row), Value::Object(m)) = (&c.row, &mut op) {
        m.insert("row".into(), row_to_wire(row));
    }
    if !c.cascade.is_empty()
        && let Value::Object(m) = &mut op
    {
        m.insert(
            "cascade".into(),
            Value::Array(
                c.cascade
                    .iter()
                    .map(|i| {
                        json!({ "entity": i.key.kind.as_str(), "id": i.key.id, "base_rev": i.base_rev })
                    })
                    .collect(),
            ),
        );
    }
    if let (Some(r), Value::Object(m)) = (&c.repush, &mut op) {
        m.insert(
            "repush".into(),
            json!(match r.kind {
                RepushKind::Row => "row",
                RepushKind::Base => "base",
            }),
        );
        m.insert("repush_rev".into(), json!(r.rev));
        m.insert("repush_epoch".into(), json!(r.epoch));
        if c.author_self {
            m.insert("author".into(), json!("self"));
        }
    }
    op
}

/// One result of `push` for the change it answers.
///
/// # Errors
///
/// The result is not the shape §6.3 gives.
pub fn result_from_wire(r: &Value, c: &Change) -> Reply<OpResult> {
    let status = r
        .get("status")
        .and_then(Value::as_str)
        .and_then(OpStatus::parse)
        .ok_or_else(|| malformed("result without status"))?;
    let row = match r.get("row") {
        None | Some(Value::Null) => None,
        Some(v) => Some(row_from_wire(c.kind.as_str(), v).map_err(|e| malformed(&e))?),
    };
    let mut cascade = Vec::new();
    for item in r.get("cascade").and_then(Value::as_array).into_iter().flatten() {
        let entity = item.get("entity").and_then(Value::as_str).unwrap_or("");
        let kind = RowKind::parse(entity).ok_or_else(|| malformed("cascade entity"))?;
        let id = id_of(item, "id").ok_or_else(|| malformed("cascade id"))?;
        let row = match item.get("row") {
            None | Some(Value::Null) => None,
            Some(v) => Some(row_from_wire(entity, v).map_err(|e| malformed(&e))?),
        };
        cascade.push(CascadeResult {
            key: RowRef { kind, id },
            rev: item.get("rev").and_then(Value::as_u64),
            deleted: item.get("deleted").and_then(Value::as_bool).unwrap_or(false),
            gone: item.get("gone").and_then(Value::as_bool).unwrap_or(false),
            fetch: item.get("fetch").and_then(Value::as_bool).unwrap_or(false),
            row,
            origin: origin_from(item.get("origin")),
        });
    }
    Ok(OpResult {
        op_id: c.op_id,
        key: c.row_ref(),
        status,
        reason: str_of(r, "reason"),
        conflict: id_of(r, "conflict"),
        adjusted: r
            .get("adjusted")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        replayed: r.get("replayed").and_then(Value::as_bool).unwrap_or(false),
        row,
        origin: origin_from(r.get("origin")),
        cascade,
        seqs: c.seqs.clone(),
        repush: c.repush.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iamb_core::model::{Folder, FolderDefaults, SyncMeta};

    fn id(n: u8) -> Id {
        let mut b = [0u8; 16];
        b[15] = n;
        b[6] = 0x70;
        b[8] = 0x80;
        Id::from_bytes(b)
    }

    #[test]
    fn a_folder_round_trips_through_the_wire_form() {
        let f = AnyRow::Folder(Folder {
            meta: SyncMeta::new(id(1), Timestamp(5)),
            profile_id: id(2),
            parent_id: None,
            name: "Replies".into(),
            position: 0,
            member_permission: None,
            defaults: FolderDefaults::default(),
        });
        let w = row_to_wire(&f);
        assert_eq!(w["defaults"]["trigger"], Value::Null);
        assert_eq!(row_from_wire("folder", &w).ok(), Some(f));
    }

    #[test]
    fn a_refusal_is_data() {
        let r = answer(json!({"error": "reset_required", "reason": "epoch", "epoch": "e"}));
        match r {
            Err(Failure::Refused(r)) => {
                assert_eq!(r.code, "reset_required");
                assert_eq!(r.reason.as_deref(), Some("epoch"));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
