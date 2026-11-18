# ADR-0020: The sync agent serves its own endpoint; sign-in happens in the browser

Status: accepted (approved by the owner on 2026-11-18)

**Context.** The UI must sign in, manage profiles and shares, and show sync state, but credentials must never pass through the process that sees keystrokes, and ideally not through the web view either.

**Decision.** `iamb-sync` runs from login (idle, with no network connections, while signed out) and serves a second local endpoint with the transport and peer verification of ADR-0006, for the UI host only. Sign-in uses the system browser and the authorisation-code flow with PKCE, returning to a loopback address that `iamb-sync` listens on only during sign-in; email magic links and third-party identity providers both use it. Tokens go straight into the OS keychain. Invitations are accepted in the app from a list fetched after sign-in.

**Consequences.** No secret crosses the service or the UI host. Sign-in always has a process to talk to. The loopback listener is the only inbound socket in the product and exists for seconds. No deep-link handler is needed in version 1.

**Rejected.** Starting the sync agent only after sign-in (nothing to sign in with). A sign-in form in the web view (the UI would handle passwords or codes). Routing account messages through the service (credentials in the keystroke process).
