# ADR-0005: OS code sits behind traits in per-OS crates

Status: proposed (awaiting owner approval)

**Context.** Windows and macOS keyboard code cannot be compiled or run on the Linux build machine, and it is the riskiest code in the project.

**Decision.** `iamb-platform` defines `KeyListener`, `Injector`, `FocusInspector`, `ClipboardAccess` and `Permissions`, plus a mock. `iamb-platform-windows` and `iamb-platform-macos` implement them and are the only crates allowed to use `unsafe`. Each is compiled only for its own target.

**Consequences.** Service logic is fully tested with the mock on all three CI systems. Reviews of keystroke handling can focus on two small crates. Mobile uses different entry points and does not implement these traits.

**Rejected.** A third-party cross-platform input crate as the foundation (none gives the needed control over layouts, dead keys, self-injection marking and tap recovery; they may still be used for narrow jobs after review). `cfg` blocks spread through the service.
