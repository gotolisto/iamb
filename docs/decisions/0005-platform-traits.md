# ADR-0005: OS code sits behind traits in per-OS crates

Status: accepted (approved by the owner on 2026-11-18)

**Context.** Windows and macOS keyboard code cannot be compiled or run on the Linux build machine, and it is the riskiest code in the project.

**Decision.** `iamb-platform` defines `KeyListener`, `Injector`, `FocusInspector`, `ClipboardAccess`, `Hotkeys`, `Permissions`, `LoginItem` and `MemoryLock`, plus a mock. `iamb-platform-windows` and `iamb-platform-macos` implement them and are the only crates allowed to use `unsafe`. Each is compiled only for its own target. The listener must classify any key that yields text as a character whatever modifiers are held, must translate without disturbing the target application's keyboard state, and must report input-method composition.

**Consequences.** Service logic is fully tested with the mock on all three CI systems. Reviews of keystroke handling can focus on two small crates. Settings such as hotkeys and start-at-login have a defined way to reach the OS. Mobile uses different entry points and does not implement these traits.

**Rejected.** A third-party cross-platform input crate as the foundation (none gives the needed control over layouts, dead keys, self-injection marking and tap recovery; they may still be used for narrow jobs after review). `cfg` blocks spread through the service. Treating every chord with Ctrl or Alt as a boundary (breaks AltGr and Option characters on most non-US layouts).
