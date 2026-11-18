# ADR-0017: Tauri 2 UI host: always resident, pre-warmed panels, preview through the service

Status: accepted (approved by the owner on 2026-11-18)

**Context.** The stack is fixed as Tauri 2 with React and TypeScript. The fill-in form must appear within 150 ms and the palette within 100 ms, which a cold web view cannot do. The editor needs the template parser and the whole library for live preview.

**Decision.** `iamb-desktop` starts hidden at login and stays resident whether or not the tray icon is shown. It keeps the fill-in form and search palette created but hidden (non-activating panels on macOS) and creates the library window on demand. The front end calls Tauri commands; only the Rust side speaks IPC. Parsing and preview are done by the service (`ParseTemplate`, `RenderPreview`) against its snapshot. Search is done by the service in memory.

**Consequences.** Latency targets are reachable with or without a tray icon. The editor and expansion can never disagree, and no second build target of the core is needed. The UI host uses memory while idle; the 20 MB footprint KPI applies to the service, and the UI host has its own budget set in task 3.2.

**Rejected.** Launching the UI host only when needed (misses the form and palette targets). Native form windows drawn by the service (a second UI toolkit inside the keystroke process). Compiling the core to WebAssembly for the editor (a second build target, and the front end would also need a copy of the library and settings).
