# ADR-0025: One settings format for per-app injection overrides

Status: accepted (recorded during Phase 2; the owner asked for Phase 2 to continue)

**Context.** Tasks 2.2 and 2.4 each gave their injector a per-app strategy table that user settings can override, but the two parsers took different JSON: Windows used `default`, `typing` and line breaks `enter`/`shift_enter`; macOS used `defaults`, `method` and `return`/`shift+return`. The service also had no way to hand the settings to the injector: the method that takes them was outside the `Injector` trait, on each OS crate's concrete type.

**Decision.**
1. The settings are one JSON string in the device-local `setting` table under the key `injection_overrides`. Settings never sync (architecture §7), so a device only ever holds overrides for its own operating system.
2. Shared shape on both systems: `{"defaults": {...}, "apps": {"<app id>": {...}}}`, where the app id is the executable name on Windows and the bundle identifier on macOS. Shared fields, same names and values everywhere:
   - `method`: `auto`, `keys` or `paste`;
   - `paste_threshold`: characters above which `auto` pastes;
   - `line_break`: `enter` or `shift+enter` on both systems; macOS also accepts `option+enter`, `control+option+enter` and `paste`;
   - `key_delay_ms` and `paste_wait_ms`.
   Each system may add fields that only make sense there (`paste_keys` and `batch_delay_ms` on Windows, `chunk_units` on macOS). Unknown fields and out-of-range values are errors, and on an error the previous settings stay in force.
3. The `Injector` trait gains `configure(&self, settings_json: &str) -> Result<()>`, with a default that accepts an empty string and refuses anything else. The service calls it at start and whenever `injection_overrides` changes, and reports a refusal as a settings error to the client that wrote it. The concrete `set_injection_settings` methods become the OS crates' implementations of it.

**Consequences.** One format to document for users and the compatibility run guide. The service needs no OS-specific code to apply overrides. Older settings strings written by development builds in the Windows shape are rejected with an error, which is acceptable before the first release.

**Rejected.** Keeping two formats (two sets of docs, and the run guide has to explain both). Putting the settings in the snapshot for the engine (the engine never chooses how text is sent, ADR-0004).
