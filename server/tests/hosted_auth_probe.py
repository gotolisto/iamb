#!/usr/bin/env python3
"""Checks, on a hosted Supabase project, what docs/sync-design.md §11.6 asks of the authentication
service before Iamb builds on it (server/README.md, "Platform checks"). Uses throwaway accounts that
it creates with SQL through the Management API and deletes again; prints claims, status codes and
counts, never a token, a key or a password.

  SUPABASE_PROJECT_REF=<ref> python3 -I server/tests/hosted_auth_probe.py [--enable-hook]

Needs the Management API (a token from SUPABASE_ACCESS_TOKEN or a proxy that adds it). The
project's publishable key is read through the Management API and kept in memory.

Steps: password sign-in (the only sign-in that needs no mailbox); the claims (`session_id`, `amr`)
before and after a refresh; Iamb's own functions through the REST interface with that session
(expected: `session_refused`, C25); a function's own `statement_timeout` through the REST interface
(two temporary probe functions in `api`, dropped again); sign-out of each scope and deletion of the
account against `auth.sessions`; with `--enable-hook`, the custom access token hook switched on and
a password sign-in refused by it while a refresh still passes.
"""

import base64
import json
import os
import secrets
import sys
import time
import urllib.error
import urllib.request

REF = os.environ["SUPABASE_PROJECT_REF"]
MGMT = f"https://api.supabase.com/v1/projects/{REF}"
BASE = f"https://{REF}.supabase.co"
TOKEN = os.environ.get("SUPABASE_ACCESS_TOKEN")


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
        raise SystemExit(f"sql failed: HTTP {code} {str(out)[:300]}")
    return out


def claims(access_token):
    payload = access_token.split(".")[1]
    payload += "=" * (-len(payload) % 4)
    return json.loads(base64.urlsafe_b64decode(payload))


def short(c):
    return {"session_id": (c.get("session_id") or "")[:8] + "…", "amr": [a.get("method") for a in c.get("amr", [])],
            "aal": c.get("aal"), "role": c.get("role"), "lifetime_s": c.get("exp", 0) - c.get("iat", 0)}


def main():
    enable_hook = "--enable-hook" in sys.argv
    code, keys = mgmt("GET", "/api-keys?reveal=false")
    pk = next(k["api_key"] for k in keys if k.get("type") == "publishable")
    auth_h = {"apikey": pk}

    def new_user():
        email = f"iamb-probe-{secrets.token_hex(6)}@example.test"
        password = secrets.token_urlsafe(24)
        rows = sql(f"""
            insert into auth.users (instance_id, id, aud, role, email, encrypted_password, email_confirmed_at,
                raw_app_meta_data, raw_user_meta_data, created_at, updated_at, confirmation_token, recovery_token,
                email_change_token_new, email_change, email_change_token_current, phone_change, phone_change_token,
                reauthentication_token)
            values ('00000000-0000-0000-0000-000000000000', gen_random_uuid(), 'authenticated', 'authenticated',
                '{email}', extensions.crypt('{password}', extensions.gen_salt('bf')), now(),
                '{{"provider":"email","providers":["email"]}}', '{{}}', now(), now(), '', '', '', '', '', '', '', '')
            returning id;""")
        uid = rows[0]["id"]
        sql(f"""insert into auth.identities (id, user_id, provider_id, provider, identity_data, created_at, updated_at, last_sign_in_at)
                values (gen_random_uuid(), '{uid}', '{uid}', 'email',
                        jsonb_build_object('sub', '{uid}', 'email', '{email}', 'email_verified', true), now(), now(), now());""")
        return uid, email, password

    def sign_in(email, password):
        return http("POST", f"{BASE}/auth/v1/token?grant_type=password", {"email": email, "password": password}, auth_h)

    def refresh(rt):
        return http("POST", f"{BASE}/auth/v1/token?grant_type=refresh_token", {"refresh_token": rt}, auth_h)

    def sessions(uid):
        return sql(f"select count(*)::int as n from auth.sessions where user_id = '{uid}'")[0]["n"]

    report = {}
    uid, email, password = new_user()
    try:
        c1, a = sign_in(email, password)
        report["password_sign_in"] = c1
        ca = claims(a["access_token"])
        report["claims_after_sign_in"] = short(ca)
        c2, a2 = refresh(a["refresh_token"])
        ca2 = claims(a2["access_token"])
        report["claims_after_refresh"] = short(ca2)
        report["session_id_same_after_refresh"] = ca.get("session_id") == ca2.get("session_id")
        report["session_row_exists"] = sql(f"select count(*)::int as n from auth.sessions where id = '{ca2['session_id']}'")[0]["n"] == 1
        user_h = dict(auth_h, Authorization=f"Bearer {a2['access_token']}")

        # Iamb's functions refuse a session opened with a password (C25, S83).
        dev = "019a0000-0000-7000-8000-000000000001"
        c, out = http("POST", f"{BASE}/rest/v1/rpc/hello",
                      {"device": {"id": dev, "name": "probe", "platform": "linux", "app_version": "0"}, "protocols": [1, 1], "max_rev": 0},
                      dict(user_h, **{"Content-Profile": "api"}))
        report["rest_hello_with_password_session"] = {"http": c, "error": (out or {}).get("error")}
        c, out = http("POST", f"{BASE}/rest/v1/rpc/check", {"device": dev, "protocol": 1}, dict(user_h, **{"Content-Profile": "api"}))
        report["rest_check_with_password_session"] = {"http": c, "error": (out or {}).get("error")}
        c, out = http("POST", f"{BASE}/rest/v1/rpc/attachment_verified", {"id": dev, "sha256": "0" * 64, "size": 1},
                      dict(user_h, **{"Content-Profile": "api_service"}))
        report["rest_api_service_as_user"] = {"http": c, "code": (out or {}).get("code")}
        c, out = http("POST", f"{BASE}/rest/v1/rpc/hello", {"device": {"id": dev}, "protocols": [1, 1], "max_rev": 0},
                      dict(auth_h, **{"Content-Profile": "api"}))
        report["rest_hello_anon"] = {"http": c, "code": (out or {}).get("code")}

        # A function's own statement_timeout applies through the REST interface (§11.6).
        sql("""create function api.iamb_probe_own_timeout() returns text language sql set statement_timeout = '15s'
                 set search_path = '' as $$ select pg_sleep(9.5)::text $$;
               create function api.iamb_probe_role_timeout() returns text language sql set search_path = ''
                 as $$ select pg_sleep(9.5)::text $$;
               revoke execute on function api.iamb_probe_own_timeout(), api.iamb_probe_role_timeout() from public, anon;
               grant execute on function api.iamb_probe_own_timeout(), api.iamb_probe_role_timeout() to authenticated;
               notify pgrst, 'reload schema';""")
        try:
            time.sleep(4)
            for name in ("iamb_probe_own_timeout", "iamb_probe_role_timeout"):
                t0 = time.time()
                c, out = http("POST", f"{BASE}/rest/v1/rpc/{name}", {}, dict(user_h, **{"Content-Profile": "api"}), timeout=40)
                report[name] = {"http": c, "seconds": round(time.time() - t0, 1), "code": (out or {}).get("code") if isinstance(out, dict) else None}
        finally:
            sql("drop function if exists api.iamb_probe_own_timeout(); drop function if exists api.iamb_probe_role_timeout(); notify pgrst, 'reload schema';")

        # Sign-out of each scope against auth.sessions.
        _, b = sign_in(email, password)
        _, cc = sign_in(email, password)
        report["sessions_after_three_sign_ins"] = sessions(uid)
        c, _ = http("POST", f"{BASE}/auth/v1/logout?scope=local", {}, dict(auth_h, Authorization=f"Bearer {b['access_token']}"))
        report["after_logout_local"] = {"http": c, "sessions": sessions(uid)}
        c, _ = http("POST", f"{BASE}/auth/v1/logout?scope=others", {}, user_h)
        report["after_logout_others"] = {"http": c, "sessions": sessions(uid),
                                         "own_session_kept": sql(f"select count(*)::int as n from auth.sessions where id = '{ca2['session_id']}'")[0]["n"] == 1}
        c, out = http("GET", f"{BASE}/auth/v1/user", None, dict(auth_h, Authorization=f"Bearer {cc['access_token']}"))
        report["access_token_of_ended_session_still_accepted_by_auth"] = c == 200

        # A password change without the current password (the settings of §11.3).
        c, out = http("PUT", f"{BASE}/auth/v1/user", {"password": secrets.token_urlsafe(24)}, user_h)
        report["password_change_without_current"] = {"http": c, "error_code": (out or {}).get("error_code") or (out or {}).get("code")}

        if enable_hook:
            code, _ = mgmt("PATCH", "/config/auth", {"hook_custom_access_token_enabled": True,
                                                    "hook_custom_access_token_uri": "pg-functions://postgres/iamb_private/token_hook"})
            report["hook_enabled"] = code
            time.sleep(8)
            c, out = sign_in(email, password)
            report["password_sign_in_with_hook"] = {"http": c, "error_code": (out or {}).get("error_code"),
                                                    "message": (out or {}).get("msg") or (out or {}).get("message")}
            c, out = refresh(a2["refresh_token"])
            report["refresh_with_hook"] = {"http": c, "claims": short(claims(out["access_token"])) if c == 200 else None}
            if c == 200:
                user_h = dict(auth_h, Authorization=f"Bearer {out['access_token']}")
        c, _ = http("POST", f"{BASE}/auth/v1/logout?scope=global", {}, user_h)
        report["after_logout_global"] = {"http": c, "sessions": sessions(uid)}

        # Deleting the account removes its sessions.
        uid2, email2, password2 = new_user()
        if not enable_hook:
            sign_in(email2, password2)
        else:
            sql(f"insert into auth.sessions (id, user_id, created_at, updated_at, aal) values (gen_random_uuid(), '{uid2}', now(), now(), 'aal1')")
        before = sessions(uid2)
        sql(f"delete from auth.users where id = '{uid2}'")
        report["user_deletion"] = {"sessions_before": before, "sessions_after": sessions(uid2)}
    finally:
        sql(f"delete from auth.users where id = '{uid}' or email like 'iamb-probe-%@example.test'")
        report["probe_accounts_left"] = sql("select count(*)::int as n from auth.users where email like 'iamb-probe-%@example.test'")[0]["n"]
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
