#!/usr/bin/env python3
"""Real writing calls through the REST interface of a hosted Supabase project, as a device makes
them (security review 4.3, finding 14: on the platform the role that interface connects with loads
`safeupdate`, which refuses an UPDATE or DELETE without a WHERE clause; `hosted.sh` runs through the
Management API, whose role does not load it, so it could not see that every writing call failed).

  SUPABASE_PROJECT_REF=<ref> python3 -I server/tests/hosted_rest_writes.py

What it does, with throwaway accounts (`iamb-rest-…@example.test`) that it removes again:

  - opens real sessions without a mailbox: the account is a row of `auth.users` made by SQL through
    the Management API, a one-time code's hash is planted in it, and the authentication service
    itself opens the session (`POST /auth/v1/verify`), so the token carries `amr: otp`, which Iamb
    accepts (a password session is refused, which is why the probe of task 4.2 never got this far);
  - through `POST /rest/v1/rpc/<function>` with those tokens: `hello`, `snapshot`,
    `set_display_name`, `create_group`, `invite`, `accept_invitation`, `push` (folders, snippets, a
    share, a stale write that records a conflict, a folder delete with a cascade list, a put that is
    refused, and one request with an edit and the move of its folder into the shared tree, for the
    re-verification's N1; a snippet deleted before its folder is shared, which the member then
    reads by `pull`, `snapshot` and `rows` and tries to put back, for N5; a share of a deleted
    folder, and a folder made and deleted in one request, for the final attack pass's O3), `pull`,
    `rows`, `get_conflict`,
    `resolve_conflict`, `members`, `attachment_upload_begin`,
    `attachment_upload_cancel`, `remove_member`, `revoke_device`, `sign_out_everywhere`;
  - checks every answer (HTTP 200 and the expected `error` or status), and that a revoked device is
    refused at once;
  - removes every row it made, the pokes it caused in `realtime.messages` and its rate counters,
    and compares the row counts of every table of `iamb`, of the authentication service's tables and
    of `storage.objects` with the counts taken before. What it cannot take back: values drawn from
    `iamb.rev_seq` and the function log lines (throwaway ids).

Needs the Management API (a token from SUPABASE_ACCESS_TOKEN, or a proxy that adds it). The
project's publishable key is read through the Management API and kept in memory. Prints status
codes, answer codes and counts; never a token, a key or an address.
"""

import hashlib
import json
import os
import secrets
import sys
import urllib.error
import urllib.request
import uuid

REF = os.environ["SUPABASE_PROJECT_REF"]
MGMT = f"https://api.supabase.com/v1/projects/{REF}"
BASE = f"https://{REF}.supabase.co"
TOKEN = os.environ.get("SUPABASE_ACCESS_TOKEN")
MARK = "iamb-rest-"            # every throwaway account's address starts with this

results = []                   # (name, ok, detail)


def http(method, url, body=None, headers=None, timeout=60):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, {"raw": raw[:300].decode(errors="replace")}


def mgmt(method, path, body=None):
    headers = {"Authorization": f"Bearer {TOKEN}"} if TOKEN else {}
    return http(method, MGMT + path, body, headers)


def sql(query):
    code, out = mgmt("POST", "/database/query", {"query": query})
    if code >= 300:
        raise RuntimeError(f"sql failed: HTTP {code} {str(out)[:600]}")
    return out


def q(s):
    return "'" + str(s).replace("'", "''") + "'"


def check(name, cond, detail=""):
    results.append((name, bool(cond), detail))
    print(("ok    " if cond else "FAIL  ") + name + (f"  [{detail}]" if detail and not cond else ""))


COUNTS = """
select jsonb_object_agg(t, n order by t) as counts from (
  select 'iamb.' || c.relname as t,
         (xpath('/row/c/text()', query_to_xml(format('select count(*) as c from iamb.%I', c.relname), false, true, '')))[1]::text::bigint as n
  from pg_class c join pg_namespace s on s.oid = c.relnamespace where s.nspname = 'iamb' and c.relkind = 'r'
  union all select 'iamb_private.' || c.relname,
         (xpath('/row/c/text()', query_to_xml(format('select count(*) as c from iamb_private.%I', c.relname), false, true, '')))[1]::text::bigint
  from pg_class c join pg_namespace s on s.oid = c.relnamespace where s.nspname = 'iamb_private' and c.relkind = 'r'
  union all select 'auth.users', count(*) from auth.users
  union all select 'auth.sessions', count(*) from auth.sessions
  union all select 'auth.identities', count(*) from auth.identities
  union all select 'auth.refresh_tokens', count(*) from auth.refresh_tokens
  union all select 'auth.one_time_tokens', count(*) from auth.one_time_tokens
  union all select 'auth.mfa_amr_claims', count(*) from auth.mfa_amr_claims
  union all select 'storage.objects', count(*) from storage.objects
) x"""


def counts():
    return sql(COUNTS)[0]["counts"]


class Account:
    """A throwaway account: a row of auth.users and its email identity, made by SQL."""

    def __init__(self, label, pk):
        self.label = label
        self.pk = pk
        self.email = f"{MARK}{label}-{secrets.token_hex(5)}@example.test"
        rows = sql(f"""
            insert into auth.users (instance_id, id, aud, role, email, encrypted_password, email_confirmed_at,
                raw_app_meta_data, raw_user_meta_data, created_at, updated_at, confirmation_token, recovery_token,
                email_change_token_new, email_change, email_change_token_current, phone_change, phone_change_token,
                reauthentication_token)
            values ('00000000-0000-0000-0000-000000000000', gen_random_uuid(), 'authenticated', 'authenticated',
                {q(self.email)}, '', now(),
                '{{"provider":"email","providers":["email"]}}', '{{}}', now(), now(), '', '', '', '', '', '', '', '')
            returning id;""")
        self.uid = rows[0]["id"]
        sql(f"""insert into auth.identities (id, user_id, provider_id, provider, identity_data, created_at, updated_at, last_sign_in_at)
                values (gen_random_uuid(), {q(self.uid)}, {q(self.uid)}, 'email',
                        jsonb_build_object('sub', {q(self.uid)}, 'email', {q(self.email)}, 'email_verified', true), now(), now(), now());""")

    def open_session(self):
        """A new session, opened by the authentication service for a one-time code."""
        code = "".join(secrets.choice("0123456789") for _ in range(8))
        digest = hashlib.sha224((self.email + code).encode()).hexdigest()
        sql(f"update auth.users set recovery_token = {q(digest)}, recovery_sent_at = now() where id = {q(self.uid)};")
        status, out = http("POST", f"{BASE}/auth/v1/verify", {"type": "magiclink", "email": self.email, "token": code},
                           {"apikey": self.pk})
        if status != 200 or not isinstance(out, dict) or "access_token" not in out:
            safe = {k: v for k, v in (out or {}).items() if k in ("code", "error_code", "msg", "message", "error")}
            raise RuntimeError(f"verify failed for {self.label}: HTTP {status} {safe}")
        return Session(self, out["access_token"])


class Session:
    def __init__(self, account, access):
        self.account = account
        self.access = access
        self.device = str(uuid.uuid4())
        self.position = None

    def rpc(self, fn, args):
        return http("POST", f"{BASE}/rest/v1/rpc/{fn}", args,
                    {"apikey": self.account.pk, "Authorization": f"Bearer {self.access}",
                     "Content-Profile": "api", "Accept-Profile": "api"})

    def hello(self):
        return self.rpc("hello", {"device": {"id": self.device, "name": "rest test", "platform": "linux", "app_version": "0"},
                                  "protocols": [1, 1], "max_rev": 0})

    def call(self, fn, **args):
        body = {"device": self.device, "protocol": 1}
        body.update(args)
        return self.rpc(fn, body)

    def snapshot(self):
        status, out = self.call("snapshot", position=self.position, cursor=None)
        if status == 200 and isinstance(out, dict) and out.get("start"):
            self.position = out["start"]
        return status, out

    def pull(self):
        status, out = self.call("pull", after=self.position, limit=500)
        if status == 200 and isinstance(out, dict) and out.get("next"):
            self.position = out["next"]
        return status, out

    def push(self, ops):
        return self.call("push", position=self.position, ops=ops)


def op(entity, rid, row, base=0, **extra):
    o = {"op_id": str(uuid.uuid4()), "entity": entity, "id": rid, "kind": "put", "base_rev": base, "row": row}
    o.update(extra)
    return o


def folder(rid, profile, parent, name, base=0):
    return op("folder", rid, {"profile_id": profile, "parent_id": parent, "name": name, "position": 0, "member_permission": None,
                              "defaults": {"trigger": None, "case_mode": None, "abbreviation_prefix": None}, "created_at": 1795190000000}, base)


def snippet(rid, fid, abbr, body, base=0):
    return op("snippet", rid, {"folder_id": fid, "abbreviation": abbr, "label": "Label", "kind": "plain", "body": body,
                               "trigger": "delimiter", "case_mode": "insensitive", "word_start": True, "enabled": True,
                               "created_at": 1795190000000}, base)


def delete(entity, rid, base=0, **extra):
    o = {"op_id": str(uuid.uuid4()), "entity": entity, "id": rid, "kind": "delete", "base_rev": base}
    o.update(extra)
    return o


def ok200(name, answer, want_error=None):
    """HTTP 200 and the expected `error` (None for success)."""
    status, out = answer
    err = out.get("error") if isinstance(out, dict) else "?"
    detail = f"HTTP {status}, error {err!r}" + (f", {out.get('message')!r}" if isinstance(out, dict) and status != 200 else "")
    check(name, status == 200 and err == want_error, detail)
    return out if isinstance(out, dict) else {}


def statuses(out):
    return [(r.get("status") or "") + (":" + r["reason"] if r.get("reason") else "") for r in (out.get("results") or [])]


CLEANUP = f"""
begin;
create temp table rest_u on commit drop as select id from auth.users where email like '{MARK}%@example.test';
create temp table rest_s on commit drop as select id from auth.sessions where user_id in (select id from rest_u);
create temp table rest_p on commit drop as select id from iamb.profile where owner_user_id in (select id from rest_u);
create temp table rest_d on commit drop as select id from iamb.device where user_id in (select id from rest_u);
delete from iamb.feed where user_id in (select id from rest_u) or profile_id in (select id from rest_p);
delete from iamb.feed_head where user_id in (select id from rest_u);
delete from iamb.conflict where profile_id in (select id from rest_p);
delete from iamb.history h where h.row_id in (
    select id from iamb.folder where profile_id in (select id from rest_p)
    union all select id from iamb.snippet where profile_id in (select id from rest_p)
    union all select id from iamb.tag where profile_id in (select id from rest_p)
    union all select id from iamb.attachment where profile_id in (select id from rest_p)
    union all select l.id from iamb.snippet_tag l join iamb.snippet s on s.id = l.snippet_id where s.profile_id in (select id from rest_p)
    union all select l.id from iamb.snippet_attachment l join iamb.snippet s on s.id = l.snippet_id where s.profile_id in (select id from rest_p)
    union all select s.id from iamb.folder_share s where s.profile_id in (select id from rest_p)
    union all select s.id from iamb.folder_share s join iamb.folder f on f.id = s.folder_id where f.profile_id in (select id from rest_p)
    union all select m.id from iamb.membership m where m.profile_id in (select id from rest_p) or m.user_id in (select id from rest_u)
    union all select id from rest_p);
delete from iamb.folder_access where profile_id in (select id from rest_p) or user_id in (select id from rest_u);
delete from iamb.snippet_tag l using iamb.snippet s where s.id = l.snippet_id and s.profile_id in (select id from rest_p);
delete from iamb.snippet_attachment l using iamb.snippet s where s.id = l.snippet_id and s.profile_id in (select id from rest_p);
delete from iamb.snippet where profile_id in (select id from rest_p);
delete from iamb.folder_share where profile_id in (select id from rest_p) or folder_id in (select id from iamb.folder where profile_id in (select id from rest_p));
update iamb.folder set parent_id = null where profile_id in (select id from rest_p);
delete from iamb.folder where profile_id in (select id from rest_p);
delete from iamb.tag where profile_id in (select id from rest_p);
delete from iamb.attachment where profile_id in (select id from rest_p);
delete from iamb.attachment_upload where profile_id in (select id from rest_p) or uploaded_by in (select id from rest_u);
delete from iamb.attachment_blob where profile_id in (select id from rest_p);
delete from iamb.invitation where profile_id in (select id from rest_p) or invited_by in (select id from rest_u);
delete from iamb.subscription where profile_id in (select id from rest_p) or user_id in (select id from rest_u);
delete from iamb.epoch_profile where profile_id in (select id from rest_p);
delete from iamb.membership where profile_id in (select id from rest_p) or user_id in (select id from rest_u);
delete from iamb.profile where id in (select id from rest_p);
delete from iamb.push_receipt where device_id in (select id from rest_d);
delete from iamb.device where id in (select id from rest_d);
delete from iamb.user_profile where user_id in (select id from rest_u);
delete from iamb.feature_override where user_id in (select id from rest_u);
delete from iamb.rate_bucket where subject in (select id from rest_u union all select id from rest_s union all select id from rest_p union all select id from rest_d);
delete from realtime.messages where topic in (select 'feed:' || id::text from rest_u);
delete from auth.users where id in (select id from rest_u);
commit;
"""


def cleanup():
    sql(CLEANUP)
    return sql(f"select count(*) as n from auth.users where email like '{MARK}%@example.test'")[0]["n"]


def run(pk):
    owner = Account("owner", pk)
    member = Account("member", pk)
    a = owner.open_session()           # the owner's first computer
    b = owner.open_session()           # her second
    t = member.open_session()          # the member's

    # hello: the first writing call of every device (it failed with "DELETE requires a WHERE clause").
    h = ok200("hello (owner, first device)", a.hello())
    ok200("hello (owner, second device)", b.hello())
    ok200("hello (member)", t.hello())
    p_owner = h.get("personal_profile")
    for s, who in ((a, "owner"), (b, "owner, second device"), (t, "member")):
        ok200(f"snapshot ({who})", s.snapshot())
    ok200("set_display_name (owner)", a.call("set_display_name", name="Rest Owner"))
    ok200("set_display_name (member)", t.call("set_display_name", name="Rest Member"))

    # Account calls.
    g, inv = str(uuid.uuid4()), str(uuid.uuid4())
    ok200("create_group", a.call("create_group", id=g, kind="family", name="Rest family"))
    ok200("invite", a.call("invite", id=inv, group=g, email=member.email, role="member"))
    ok200("accept_invitation", t.call("accept_invitation", id=inv))
    out = ok200("members (owner)", a.call("members", group=g))
    check("members lists both, with addresses for the owner",
          len(out.get("members") or []) == 2 and all(m.get("email") for m in out.get("members") or []))
    out = ok200("members (member)", t.call("members", group=g))
    check("members gives a member no address", all(m.get("email") is None for m in out.get("members") or [{"email": 1}]))

    # push: folders, snippets and a share, in one request.
    f1, f2, s1, s2 = (str(uuid.uuid4()) for _ in range(4))
    share = sql(f"select iamb_private.share_id({q(f1)}, {q(g)}) as id")[0]["id"]
    out = ok200("push (two folders, two snippets, a share)", a.push([
        folder(f1, p_owner, None, "Shared"), folder(f2, p_owner, None, "Private"),
        snippet(s1, f1, ";hrs", "Open 9 to 5."), snippet(s2, f2, ";priv", "Private text."),
        op("folder_share", share, {"folder_id": f1, "profile_id": g, "permission": "edit"})]))
    check("all five ops applied", statuses(out) == ["applied"] * 5, str(statuses(out)))
    rev1 = ((out.get("results") or [{}] * 3)[2].get("row") or {}).get("rev")

    # The member reads the shared folder through the feed, and nothing of the private one.
    out = ok200("pull (member)", t.pull())
    check("the member's feed says that the shared folder became visible, and nothing of the private one",
          any(e.get("kind") == "folder_visible" and e.get("folder") == f1 for e in out.get("entries") or [])
          and f2 not in json.dumps(out) and s2 not in json.dumps(out))
    out = ok200("fetch_folder (member, the shared folder)", t.call("fetch_folder", folder=f1, cursor=None))
    seen = json.dumps(out)
    check("it brings the shared snippet and not the private one", s1 in seen and s2 not in seen)
    ok200("fetch_folder (member, the private folder)", t.call("fetch_folder", folder=f2, cursor=None), "not_found")

    # A refused write is data, and so is input of the wrong type (finding 7).
    out = ok200("push (member writes into the private folder; an op with a base that is not a number)", t.push([
        snippet(str(uuid.uuid4()), f2, ";x", "x"),
        {"op_id": str(uuid.uuid4()), "entity": "tag", "id": str(uuid.uuid4()), "kind": "delete", "base_rev": "x"}]))
    check("answered rejected:parent_gone and blocked:invalid", statuses(out) == ["rejected:parent_gone", "blocked:invalid"], str(statuses(out)))

    # A conflict: the member and the owner's second device both write the snippet from the same base.
    ok200("pull (owner, second device)", b.pull())
    out = ok200("push (member edits the shared snippet)", t.push([snippet(s1, f1, ";hrs", "Open 9 to 6.", rev1)]))
    check("applied", statuses(out) == ["applied"], str(statuses(out)))
    out = ok200("push (owner's second device, stale)", b.push([snippet(s1, f1, ";hrs", "Open 10 to 4.", rev1)]))
    conflict = ((out.get("results") or [{}])[0]).get("conflict")
    check("applied, with a conflict", statuses(out) == ["applied"] and bool(conflict), str(statuses(out)))
    out = ok200("conflicts (member)", t.call("conflicts"))
    check("the member lists the conflict", any(c.get("id") == conflict for c in out.get("conflicts") or []))
    out = ok200("get_conflict (member)", t.call("get_conflict", id=conflict))
    bodies = [(v.get("row") or {}).get("body") for v in out.get("versions") or []]
    check("it offers him the version he wrote", bodies == ["Open 9 to 6."], str(len(bodies)))
    rev_member = (out.get("versions") or [{}])[0].get("rev")
    out = ok200("resolve_conflict restore (member)", t.call("resolve_conflict", id=conflict, action="restore", rev=rev_member))
    check("resolved", out.get("state") == "resolved")

    # Uploads under way.
    out = ok200("attachment_upload_begin (member, in the owner's profile)",
                t.call("attachment_upload_begin", profile=p_owner, sha256="ab" * 32, size=1000, mime="image/png"))
    ok200("attachment_upload_cancel (owner ends the member's upload)", a.call("attachment_upload_cancel", upload=out.get("upload")))

    # Re-verification N1, end to end: the owner's second device replaces private content and
    # moves its folder into the shared tree in ONE push. The member is not offered the version
    # that was replaced, and cannot restore it.
    fp, sp = str(uuid.uuid4()), str(uuid.uuid4())
    out = ok200("push (owner: another private folder with a snippet)", a.push([
        folder(fp, p_owner, None, "Private two"), snippet(sp, fp, ";pin", "Nothing yet.")]))
    fp_rev = ((out.get("results") or [{}])[0].get("row") or {}).get("rev")
    sp_rev = ((out.get("results") or [{}] * 2)[1].get("row") or {}).get("rev")
    ok200("pull (owner, second device)", b.pull())
    out = ok200("push (owner, first device: private content)", a.push([snippet(sp, fp, ";pin", "PIN of the safe: 4711", sp_rev)]))
    rev_private = ((out.get("results") or [{}])[0].get("row") or {}).get("rev")
    out = ok200("push (owner, second device: the edit and the folder's move in one request)", b.push([
        snippet(sp, fp, ";pin", "Ask me.", sp_rev), folder(fp, p_owner, f1, "Private two", fp_rev)]))
    check("both applied, the edit with a conflict of her own",
          statuses(out) == ["applied", "applied"] and bool((out.get("results") or [{}])[0].get("conflict")), str(statuses(out)))
    ok200("pull (member)", t.pull())
    out = ok200("push (member: a stale edit of the snippet that came into the shared tree)", t.push([snippet(sp, fp, ";pin", "Member's edit", 0)]))
    conflict2 = ((out.get("results") or [{}])[0]).get("conflict")
    check("applied, with a conflict", statuses(out) == ["applied"] and bool(conflict2), str(statuses(out)))
    out = ok200("get_conflict (member)", t.call("get_conflict", id=conflict2))
    bodies = [(v.get("row") or {}).get("body") for v in out.get("versions") or []]
    check("the version replaced in the push that shared the folder is not offered to the member",
          bodies == ["Ask me."] and "4711" not in json.dumps(out), str(len(bodies)))
    ok200("resolve_conflict restore of that version (member)", t.call("resolve_conflict", id=conflict2, action="restore", rev=rev_private), "invalid")
    out = ok200("get_conflict (owner)", a.call("get_conflict", id=conflict2))
    check("the owner is offered it", "4711" in json.dumps(out))
    # N6: the member's "keep" closes the conflict for him and leaves it open for the owner.
    out = ok200("resolve_conflict keep (member)", t.call("resolve_conflict", id=conflict2, action="keep"))
    check("resolved for him", out.get("state") == "resolved")
    out = ok200("conflicts (owner)", a.call("conflicts"))
    check("still in the owner's list", any(c.get("id") == conflict2 for c in out.get("conflicts") or []))
    # N2: a cascade list that names a row twice is refused as data.
    out = ok200("push (a cascade list that names a row twice)", a.push([
        delete("folder", str(uuid.uuid4()), 0, cascade=[{"entity": "snippet", "id": sp, "base_rev": 0}] * 2)]))
    check("blocked:invalid", statuses(out) == ["blocked:invalid"], str(statuses(out)))

    # Re-verification N5, end to end: a snippet deleted before its folder is shared reaches the
    # member by no read, and a put of his does not bring it back.
    fq, sq, sk = (str(uuid.uuid4()) for _ in range(3))
    out = ok200("push (owner: a third private folder with two snippets)", a.push([
        folder(fq, p_owner, None, "Private three"), snippet(sq, fq, ";debt", "Lena still owes 40."), snippet(sk, fq, ";ok", "Stays.")]))
    sq_rev = ((out.get("results") or [{}] * 2)[1].get("row") or {}).get("rev")
    out = ok200("push (owner deletes one of them)", a.push([delete("snippet", sq, sq_rev)]))
    check("applied", statuses(out) == ["applied"], str(statuses(out)))
    share_q = sql(f"select iamb_private.share_id({q(fq)}, {q(g)}) as id")[0]["id"]
    out = ok200("push (owner shares the folder, to change)", a.push([
        op("folder_share", share_q, {"folder_id": fq, "profile_id": g, "permission": "edit"})]))
    check("applied", statuses(out) == ["applied"], str(statuses(out)))
    out = ok200("pull (member)", t.pull())
    check("the folder is visible to the member, and no entry carries the deleted snippet",
          any(e.get("kind") == "folder_visible" and e.get("folder") == fq for e in out.get("entries") or [])
          and "owes 40" not in json.dumps(out))
    out = ok200("snapshot (member)", t.snapshot())
    ids_seen = {r.get("id") for r in out.get("rows") or []}
    check("the member's snapshot has the folder and its live snippet, and not the one deleted before the share",
          fq in ids_seen and sk in ids_seen and sq not in ids_seen and "owes 40" not in json.dumps(out), str(len(ids_seen)))
    out = ok200("rows (member, the deleted snippet by its id)", t.call("rows", refs=[{"entity": "snippet", "id": sq}]))
    check("null", [r.get("row") for r in out.get("rows") or []] == [None])
    out = ok200("push (member: a put with the deleted snippet's id)", t.push([snippet(sq, fq, ";mine", "Member's text", 0)]))
    check("rejected:gone, with no row", statuses(out) == ["rejected:gone"] and (out.get("results") or [{}])[0].get("row") is None,
          str(statuses(out)))
    out = ok200("snapshot (owner)", a.snapshot())
    check("the owner still holds the tombstone, as she left it",
          any(r.get("id") == sq and (r.get("row") or {}).get("deleted_at") and (r.get("row") or {}).get("body") == "Lena still owes 40."
              for r in out.get("rows") or []))

    # The final attack pass, O3, as far as a device can reach it. A share of a deleted folder is
    # refused; and a folder made and deleted in one push is named to nobody who could not read it.
    fd, top, fx, sxx = (str(uuid.uuid4()) for _ in range(4))
    out = ok200("push (owner: one more private folder)", a.push([folder(fd, p_owner, None, "To be deleted")]))
    fd_rev = ((out.get("results") or [{}])[0].get("row") or {}).get("rev")
    out = ok200("push (owner deletes it)", a.push([delete("folder", fd, fd_rev, cascade=[])]))
    check("applied", statuses(out) == ["applied"], str(statuses(out)))
    share_d = sql(f"select iamb_private.share_id({q(fd)}, {q(g)}) as id")[0]["id"]
    out = ok200("push (owner shares the deleted folder)", a.push([
        op("folder_share", share_d, {"folder_id": fd, "profile_id": g, "permission": "view"})]))
    check("rejected:parent_gone", statuses(out) == ["rejected:parent_gone"], str(statuses(out)))
    out = ok200("snapshot (member)", t.snapshot())
    check("the member's snapshot does not name the deleted folder", fd not in json.dumps(out))
    out = ok200("push (owner: a folder in the shared one)", a.push([folder(top, p_owner, f1, "Returns")]))
    top_rev = ((out.get("results") or [{}])[0].get("row") or {}).get("rev")
    ok200("pull (member)", t.pull())
    out = ok200("push (owner: a folder made below it and the folder above deleted, in one request)", a.push([
        folder(fx, p_owner, top, "Made offline"), snippet(sxx, fx, ";off", "Made offline too."),
        delete("folder", top, top_rev, cascade=[{"entity": "folder", "id": fx, "base_rev": 0},
                                                {"entity": "snippet", "id": sxx, "base_rev": 0}])]))
    check("all three applied, and the answer says that what was made is deleted",
          statuses(out) == ["applied"] * 3
          and all(e.get("deleted") for e in ((out.get("results") or [{}] * 3)[2].get("cascade") or [{}])), str(statuses(out)))
    out = ok200("pull (member)", t.pull())
    check("the member's feed has the tombstone of the folder he read, and names nothing made and deleted in that request",
          any(e.get("id") == top and (e.get("row") or {}).get("deleted_at") for e in out.get("entries") or [])
          and fx not in json.dumps(out) and sxx not in json.dumps(out))

    # A folder's delete with a cascade list (the largest scratch use of task 4.2).
    out = ok200("push (owner deletes the private folder with its snippet)", a.push([
        delete("folder", f2, 0, cascade=[{"entity": "snippet", "id": s2, "base_rev": 0}])]))
    res = (out.get("results") or [{}])[0]
    check("applied, with an answer for the row of the list", res.get("status") == "applied" and len(res.get("cascade") or []) == 1,
          str(statuses(out)))

    # Removal, then revocation: what must work on the day it is needed.
    ok200("remove_member", a.call("remove_member", group=g, user=member.uid))
    out = ok200("pull (member, after the removal)", t.pull())
    check("the member's feed holds folder_revoked", any(e.get("kind") == "folder_revoked" for e in out.get("entries") or []))
    out = ok200("get_conflict (member, after the removal)", t.call("get_conflict", id=conflict), "not_found")
    out = ok200("revoke_device (the owner revokes her second device)", a.call("revoke_device", target=b.device))
    check("it names the revoked device", b.device in (out.get("devices") or []))
    ok200("a call by the revoked device", b.call("visible_folders"), "device_revoked")
    ok200("hello by the revoked session", b.hello(), "device_revoked")
    ok200("sign_out_everywhere", a.call("sign_out_everywhere"))
    ok200("a call after sign_out_everywhere", a.call("visible_folders"), "device_revoked")
    ok200("delete_group is refused to a revoked device", a.call("delete_group", group=g), "device_revoked")


def main():
    code, keys = mgmt("GET", "/api-keys?reveal=false")
    if code != 200:
        raise SystemExit(f"api-keys: HTTP {code}")
    pk = next((k["api_key"] for k in keys if k.get("type") == "publishable"), None)
    if not pk:
        raise SystemExit("the project has no publishable key")
    left = cleanup()               # whatever an interrupted run left
    before = counts()
    try:
        run(pk)
    except Exception as e:  # noqa: BLE001 - always clean up, then report
        check("the run completed", False, f"{type(e).__name__}: {str(e)[:300]}")
    finally:
        left = cleanup()
    after = counts()
    check("throwaway accounts left: 0", left == 0, str(left))
    diff = {k: (before.get(k), after.get(k)) for k in sorted(set(before) | set(after)) if before.get(k) != after.get(k)}
    check("every table has the row count it had before", not diff, json.dumps(diff))
    print("row counts after (tables that hold rows):", json.dumps({k: v for k, v in after.items() if v}))
    failed = [name for name, ok, _ in results if not ok]
    print(f"\nhosted REST writing test: {len(results) - len(failed)} passed, {len(failed)} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
