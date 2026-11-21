//! Two devices of one account against the model of the server: the basic paths of the loop.

#![cfg(feature = "model")]

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
    clock: Rc<Cell<i64>>,
    user: Id,
}

fn device(server: &Rc<RefCell<Server>>, user: Id, session: Id, seed: u64) -> Result<Dev, Box<dyn std::error::Error>> {
    let clock = Rc::new(Cell::new(T0));
    let store = Store::open_in_memory()?;
    let t = SharedTransport { server: server.clone(), caller: Caller { user, session } };
    let agent = Agent::new(t, StoreDevice::new(store, clock.clone()), AgentConfig::default(), ids(seed));
    Ok(Dev { agent, clock, user })
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
            meta: SyncMeta { updated_by: Some(user), ..SyncMeta::new(f, Timestamp(T0)) },
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
        SyncMeta { updated_by: Some(user), ..SyncMeta::new(s, Timestamp(T0)) },
        f,
        Abbreviation::new(";rty")?,
        "Thanks",
        "Thanks for your order.",
        None,
    );
    store(&mut a).put(&sn, Timestamp(T0))?;
    a.agent.sync()?;
    b.agent.sync()?;
    let got = store(&mut b).get_row(RowRef { kind: RowKind::Snippet, id: s })?;
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
    let row = &srv.st.rows[&RowRef { kind: RowKind::Snippet, id: s }];
    match &row.row {
        iamb_core::sync::AnyRow::Snippet(x) => assert_eq!(x.body, "Thanks a lot."),
        _ => panic!("not a snippet"),
    }
    assert_eq!(srv.st.conflicts.len(), 1);
    assert_eq!(srv.st.conflicts[0].kind, "edit_edit");
    drop(srv);
    let ra = store(&mut a).get_row(RowRef { kind: RowKind::Snippet, id: s })?;
    let rb = store(&mut b).get_row(RowRef { kind: RowKind::Snippet, id: s })?;
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
    let meta = |id| SyncMeta { updated_by: Some(user), ..SyncMeta::new(id, Timestamp(T0)) };
    store(&mut a).put(&Folder { meta: meta(f), profile_id: personal, parent_id: None, name: "F".into(), position: 0, member_permission: None, defaults: FolderDefaults::default() }, Timestamp(T0))?;
    let sn = Snippet::new_defaults(meta(s), f, Abbreviation::new(";x")?, "x", "x", None);
    store(&mut a).put(&sn, Timestamp(T0))?;
    store(&mut a).put(&Tag { meta: meta(t), profile_id: personal, name: "t".into(), color: None }, Timestamp(T0))?;
    store(&mut a).put(&SnippetTag { meta: meta(l), snippet_id: s, tag_id: t }, Timestamp(T0))?;
    a.agent.sync()?;
    let link = RowRef { kind: RowKind::SnippetTag, id: l };
    assert!(store(&mut a).get_row(link)?.is_some());
    store(&mut a).delete::<Snippet>(s, Timestamp(T0 + 5))?;
    a.agent.sync()?;
    assert!(store(&mut a).get_row(link)?.is_some(), "after the delete");
    let dead = store(&mut a).get_row(RowRef { kind: RowKind::Snippet, id: s })?;
    let Some(iamb_core::sync::AnyRow::Snippet(mut back)) = dead else { panic!("no tombstone") };
    back.meta.updated_by = Some(user);
    store(&mut a).put(&back, Timestamp(T0 + 9))?;
    assert!(store(&mut a).get_row(link)?.is_some(), "after the local undelete");
    a.agent.sync()?;
    assert!(store(&mut a).get_row(link)?.is_some(), "after the push");
    Ok(())
}
