# ADR-0004: The engine is a state machine: events in, actions out

Status: proposed (awaiting owner approval)

**Context.** Keyboard hooks, injection and forms differ on every platform, but the decision "what should happen for this keystroke" should not.

**Decision.** `Engine::handle(event, env) -> Vec<Action>` is the only entry point. Events and actions are closed enums listed in the architecture document. The engine never performs an action itself and never learns how text reaches the target app.

**Consequences.** A typing session is a list of events with an expected list of actions, so the 50 scripted sessions in task 1.6 need no OS. Platform crates stay thin. Adding an action is a deliberate, reviewed change to a shared enum.

**Rejected.** Callbacks into a platform object from inside the engine (hard to test, re-entrancy risks). Separate matcher and renderer APIs glued together by each platform (the glue would be rewritten five times).
