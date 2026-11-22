//! Two devices of one account against the model of the server: the basic paths of the loop.

#![cfg(feature = "model")]
#![allow(
    clippy::many_single_char_names,
    reason = "short names for the rows of a test"
)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use iamb_core::model::{Abbreviation, Folder, FolderDefaults, Id, Snippet, SyncMeta, Timestamp};
use iamb_core::sync::{RowKind, RowRef};
use iamb_store::Store;
use iamb_sync::model::{Caller, Server, SharedTransport, StoreDevice};
use iamb_sync::{Agent, AgentConfig};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const T0: i64 = 1_795_000_000_000;

fn ids(seed: u64) -> Box<dyn FnMut() -> Id + Send> {
    let mut r = iamb_sync::schedule::SplitMix(seed);
    Box::new(move || {
        let mut b = [0u8; 10];
        for x in &mut b {
            *x = (r.next_u64() & 0xff) as u8;
        }
        Id::v7(T0 as u64, b)
    })
}

struct Dev {
    agent: Agent<SharedTransport, StoreDevice>,
    user: Id,
}

fn device(
    server: &Rc<RefCell<Server>>,
    user: Id,
    session: Id,
    seed: u64,
) -> Result<Dev, Box<dyn std::error::Error>> {
    let clock = Rc::new(Cell::new(T0));
    let store = Store::open_in_memory()?;
    let t = SharedTransport {
        server: server.clone(),
        caller: Caller { user, session },
    };
    let agent = Agent::new(
        t,
        StoreDevice::new(store, clock.clone()),
        AgentConfig::default(),
        ids(seed),
    );
    Ok(Dev { agent, user })
}

fn store(d: &mut Dev) -> &mut Store {
    &mut d.agent.device.store
}

#[test]
fn a_change_on_one_device_reaches_the_other() -> TestResult {
    let server = Rc::new(RefCell::new(Server::new(T0, 1)));
    let (user, personal, s1) = server.borrow_mut().add_user("ines@example.com");
    let s2 = server.borrow_mut().add_session();
    let mut a = device(&server, user, s1, 11)?;
    let mut b = device(&server, user, s2, 22)?;
    a.agent.sync()?;
    b.agent.sync()?;
    let mut next = ids(99);
    let f = next();
    let s = next();
    store(&mut a).put(
        &Folder {
            meta: SyncMeta {
                updated_by: Some(user),
                ..SyncMeta::new(f, Timestamp(T0))
            },
            profile_id: personal,
            parent_id: None,
            name: "Replies".into(),
            position: 0,
            member_permission: None,
            defaults: FolderDefaults::default(),
        },
        Timestamp(T0),
    )?;
    let mut sn = Snippet::new_defaults(
        SyncMeta {
            updated_by: Some(user),
            ..SyncMeta::new(s, Timestamp(T0))
        },
        f,
        Abbreviation::new(";rty")?,
        "Thanks",
        "Thanks for your order.",
        None,
    );
    store(&mut a).put(&sn, Timestamp(T0))?;
    a.agent.sync()?;
    b.agent.sync()?;
    let got = store(&mut b).get_row(RowRef {
        kind: RowKind::Snippet,
        id: s,
    })?;
    assert!(got.is_some(), "B holds the snippet");
    assert_eq!(server.borrow().violations, Vec::<String>::new());

    // Both edit offline; B's write arrives last and wins with a conflict.
    sn.body = "Thank you for your order.".into();
    store(&mut a).put(&sn, Timestamp(T0 + 1))?;
    sn.body = "Thanks a lot.".into();
    store(&mut b).put(&sn, Timestamp(T0 + 2))?;
    a.agent.sync()?;
    b.agent.sync()?;
    a.agent.sync()?;
    let srv = server.borrow();
    let row = &srv.st.rows[&RowRef {
        kind: RowKind::Snippet,
        id: s,
    }];
    match &row.row {
        iamb_core::sync::AnyRow::Snippet(x) => assert_eq!(x.body, "Thanks a lot."),
        _ => panic!("not a snippet"),
    }
    assert_eq!(srv.st.conflicts.len(), 1);
    assert_eq!(srv.st.conflicts[0].kind, "edit_edit");
    drop(srv);
    let ra = store(&mut a).get_row(RowRef {
        kind: RowKind::Snippet,
        id: s,
    })?;
    let rb = store(&mut b).get_row(RowRef {
        kind: RowKind::Snippet,
        id: s,
    })?;
    assert_eq!(ra, rb, "both devices agree");
    let _ = (a.user, b.user);
    Ok(())
}

#[test]
fn undeleting_a_snippet_keeps_its_links() -> TestResult {
    use iamb_core::model::{SnippetTag, Tag};
    let server = Rc::new(RefCell::new(Server::new(T0, 2)));
    let (user, personal, s1) = server.borrow_mut().add_user("ines@example.com");
    let mut a = device(&server, user, s1, 11)?;
    a.agent.sync()?;
    let mut next = ids(77);
    let (f, s, t, l) = (next(), next(), next(), next());
    let meta = |id| SyncMeta {
        updated_by: Some(user),
        ..SyncMeta::new(id, Timestamp(T0))
    };
    store(&mut a).put(
        &Folder {
            meta: meta(f),
            profile_id: personal,
            parent_id: None,
            name: "F".into(),
            position: 0,
            member_permission: None,
            defaults: FolderDefaults::default(),
        },
        Timestamp(T0),
    )?;
    let sn = Snippet::new_defaults(meta(s), f, Abbreviation::new(";x")?, "x", "x", None);
    store(&mut a).put(&sn, Timestamp(T0))?;
    store(&mut a).put(
        &Tag {
            meta: meta(t),
            profile_id: personal,
            name: "t".into(),
            color: None,
        },
        Timestamp(T0),
    )?;
    store(&mut a).put(
        &SnippetTag {
            meta: meta(l),
            snippet_id: s,
            tag_id: t,
        },
        Timestamp(T0),
    )?;
    a.agent.sync()?;
    let link = RowRef {
        kind: RowKind::SnippetTag,
        id: l,
    };
    assert!(store(&mut a).get_row(link)?.is_some());
    store(&mut a).delete::<Snippet>(s, Timestamp(T0 + 5))?;
    a.agent.sync()?;
    assert!(store(&mut a).get_row(link)?.is_some(), "after the delete");
    let dead = store(&mut a).get_row(RowRef {
        kind: RowKind::Snippet,
        id: s,
    })?;
    let Some(iamb_core::sync::AnyRow::Snippet(mut back)) = dead else {
        panic!("no tombstone")
    };
    back.meta.updated_by = Some(user);
    store(&mut a).put(&back, Timestamp(T0 + 9))?;
    assert!(
        store(&mut a).get_row(link)?.is_some(),
        "after the local undelete"
    );
    a.agent.sync()?;
    assert!(store(&mut a).get_row(link)?.is_some(), "after the push");
    Ok(())
}

/// The refusal code of a call, or "ok".
fn code(r: &Result<serde_json::Value, iamb_sync::RoundError>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(iamb_sync::RoundError::Protocol(e)) => {
            e.trim_start_matches("unexpected refusal ").into()
        }
        Err(e) => e.to_string(),
    }
}

/// The answers the server gives since the security review of task 4.3 (ADR-0031; the server's
/// migrations 12 and 13), as the model gives them.
#[test]
fn answers_after_the_security_review() -> TestResult {
    use serde_json::json;
    let server = Rc::new(RefCell::new(Server::new(T0, 2)));
    let (ines, personal, s1) = server.borrow_mut().add_user("ines@example.com");
    let s2 = server.borrow_mut().add_session();
    let (tom, _, s3) = server.borrow_mut().add_user("tom@example.com");
    let mut a = device(&server, ines, s1, 11)?;
    let mut b = device(&server, ines, s2, 22)?;
    let mut t = device(&server, tom, s3, 33)?;
    a.agent.sync()?;
    b.agent.sync()?;
    t.agent.sync()?;
    let mut next = ids(77);
    let (f, s, g, op1, op2) = (next(), next(), next(), next(), next());
    store(&mut a).put(
        &Folder {
            meta: SyncMeta {
                updated_by: Some(ines),
                ..SyncMeta::new(f, Timestamp(T0))
            },
            profile_id: personal,
            parent_id: None,
            name: "Private".into(),
            position: 0,
            member_permission: None,
            defaults: FolderDefaults::default(),
        },
        Timestamp(T0),
    )?;
    let mut sn = Snippet::new_defaults(
        SyncMeta {
            updated_by: Some(ines),
            ..SyncMeta::new(s, Timestamp(T0))
        },
        f,
        Abbreviation::new(";iban")?,
        "Account",
        "My account number.",
        None,
    );
    store(&mut a).put(&sn, Timestamp(T0))?;
    a.agent.sync()?;
    b.agent.sync()?;

    // A stranger's delete of a row he cannot read answers as one of an unknown id, and a write
    // into a profile he has no grant in is `parent_gone`.
    let position = store(&mut t).sync_info()?.position.unwrap_or_default();
    let del = json!({ "op_id": op1, "follows": [], "entity": "snippet", "id": s, "kind": "delete",
                      "base_rev": 1 });
    let mut mine = sn.clone();
    mine.meta.id = next();
    mine.meta.updated_by = Some(tom);
    let put = json!({ "op_id": op2, "follows": [], "entity": "snippet", "id": mine.meta.id,
                      "kind": "put", "base_rev": 0,
                      "row": iamb_sync::api::row_to_wire(&iamb_core::sync::AnyRow::Snippet(mine)) });
    let r = t
        .agent
        .raw_call("push", json!({ "position": position, "ops": [del, put] }))?;
    assert_eq!(r["results"][0]["status"], "unchanged", "{r}");
    assert_eq!(r["results"][1]["status"], "rejected", "{r}");
    assert_eq!(r["results"][1]["reason"], "parent_gone", "{r}");

    // A conflict: its party sees the version she wrote; a stranger is told `not_found`, also for
    // "keep", and the conflict stays open.
    sn.body = "My account number, new.".into();
    store(&mut a).put(&sn, Timestamp(T0 + 1))?;
    sn.body = "My other account.".into();
    store(&mut b).put(&sn, Timestamp(T0 + 2))?;
    a.agent.sync()?;
    b.agent.sync()?;
    let cid = server.borrow().st.conflicts[0].id;
    let seen = a.agent.raw_call("get_conflict", json!({ "id": cid }))?;
    assert_eq!(seen["versions"].as_array().map(Vec::len), Some(1), "{seen}");
    let stranger = t.agent.raw_call("get_conflict", json!({ "id": cid }));
    assert_eq!(code(&stranger), "not_found");
    let keep = t
        .agent
        .raw_call("resolve_conflict", json!({ "id": cid, "action": "keep" }));
    assert_eq!(code(&keep), "not_found");
    let other = a.agent.raw_call(
        "resolve_conflict",
        json!({ "id": cid, "action": "restore", "rev": 999_999 }),
    );
    assert_eq!(code(&other), "invalid", "a version that is not on offer");
    assert_eq!(server.borrow().st.conflicts[0].state, "open");

    // Account calls: a non-member is told `not_found` first; an id in use is `invalid`.
    let made = a.agent.raw_call(
        "create_group",
        json!({ "id": g, "kind": "family", "name": "Family" }),
    );
    assert_eq!(code(&made), "ok");
    let again = t.agent.raw_call(
        "create_group",
        json!({ "id": g, "kind": "family", "name": "Mine" }),
    );
    assert_eq!(code(&again), "invalid");
    let invite = t.agent.raw_call(
        "invite",
        json!({ "id": next(), "group": g, "email": "x@example.com", "role": "member" }),
    );
    assert_eq!(code(&invite), "not_found");
    assert_eq!(server.borrow().violations, Vec::<String>::new());
    Ok(())
}

/// A transport that answers a push naming one row with the request-level `invalid` of the server
/// (an op that failed in the database undoes the whole request, 4.3).
struct Poisoned {
    inner: SharedTransport,
    bad: Id,
}

impl iamb_sync::api::Transport for Poisoned {
    fn call(
        &mut self,
        function: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, iamb_sync::api::CallError> {
        let names_bad = args["ops"]
            .as_array()
            .is_some_and(|ops| ops.iter().any(|o| o["id"] == serde_json::json!(self.bad)));
        if function == "push" && names_bad {
            let epoch = self.inner.server.borrow().epoch().id.clone();
            return Ok(serde_json::json!({ "error": "invalid", "epoch": epoch }));
        }
        self.inner.call(function, args)
    }
}

#[test]
fn a_request_undone_by_one_op_sends_the_others_and_blocks_that_one() -> TestResult {
    let server = Rc::new(RefCell::new(Server::new(T0, 3)));
    let (user, personal, s1) = server.borrow_mut().add_user("ines@example.com");
    let mut next = ids(55);
    let folders: Vec<Id> = (0..5).map(|_| next()).collect();
    let bad = folders[2];
    let clock = Rc::new(Cell::new(T0));
    let mut agent = Agent::new(
        Poisoned {
            inner: SharedTransport {
                server: server.clone(),
                caller: Caller { user, session: s1 },
            },
            bad,
        },
        StoreDevice::new(Store::open_in_memory()?, clock),
        AgentConfig::default(),
        ids(66),
    );
    agent.sync()?;
    for (i, f) in folders.iter().enumerate() {
        agent.device.store.put(
            &Folder {
                meta: SyncMeta {
                    updated_by: Some(user),
                    ..SyncMeta::new(*f, Timestamp(T0))
                },
                profile_id: personal,
                parent_id: None,
                name: format!("Folder {i}"),
                position: i64::try_from(i)?,
                member_permission: None,
                defaults: FolderDefaults::default(),
            },
            Timestamp(T0),
        )?;
    }
    agent.sync()?;
    let srv = server.borrow();
    for f in &folders {
        let key = RowRef {
            kind: RowKind::Folder,
            id: *f,
        };
        assert_eq!(srv.st.rows.contains_key(&key), *f != bad, "{f}");
    }
    drop(srv);
    let blocked = agent.device.store.blocked_rows()?;
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].0.id, bad);
    assert_eq!(blocked[0].1, "invalid");
    Ok(())
}
