# ADR-0017: Tauri 2 UI host: resident tray, pre-warmed form and palette

Status: proposed (awaiting owner approval)

**Context.** The stack is fixed as Tauri 2 with React and TypeScript. The fill-in form must appear within 150 ms and the palette within 100 ms, which a cold web view cannot do.

**Decision.** `iamb-desktop` starts hidden at login and owns the tray icon. It keeps the fill-in form and search palette windows created but hidden, and creates the library window on demand. The front end calls Tauri commands; only the Rust side speaks IPC. The template parser from `iamb-core` is compiled to WebAssembly for live preview in the editor.

**Consequences.** Latency targets are reachable. The UI host uses memory while idle; the 20 MB footprint KPI applies to the service, and the UI host has its own budget set in task 3.2. Users can turn the tray off, in which case forms open cold.

**Rejected.** Launching the UI host only when needed (misses the form and palette targets). Native form windows drawn by the service (a second UI toolkit inside the keystroke process).
