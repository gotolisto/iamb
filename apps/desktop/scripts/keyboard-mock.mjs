// The command layer for the keyboard-only run (`keyboard-run.mjs`): what `src-tauri` answers,
// over a small in-page library that keeps what the front end writes. Invented text only.
/**
 * `addInitScript(mock, config)`: stands in for the Tauri runtime, so that the built front end
 * runs in a plain browser. `config.window` is the window's label (`library`, `palette`, `form`,
 * `onboarding`); `config.snippets` seeds the library; `config.open` is what the panel was opened
 * with. Self-contained: Playwright sends its source into the page. The page's state is read
 * back through `window.__mock` (what was saved, picked, submitted and set).
 */
export const mock = (config) => {
  const now = 1_700_000_000_000;
  const folders = [
    { id: "f1", parentId: null, name: "Replies", position: 0 },
    { id: "f2", parentId: null, name: "Signatures", position: 1 },
  ].map((f) => ({ ...f, defaults: { abbreviationPrefix: null, trigger: null, caseMode: null } }));
  const tags = ["urgent", "team"].map((name, i) => ({ id: `t${i}`, name, color: ["red", "blue"][i], snippets: 0 }));
  let seq = 0;
  const snippets = new Map();
  const seed = (s) => {
    const id = s.id ?? `00000000-0000-7000-8000-${String(++seq).padStart(12, "0")}`;
    snippets.set(id, {
      id, folderId: "f1", abbreviation: "", label: "", kind: "plain", body: "", trigger: "delimiter",
      caseMode: "insensitive", wordStart: true, enabled: true, tags: [], rev: 0, createdAt: now, updatedAt: now,
      ...s,
    });
    return id;
  };
  for (const s of config.snippets ?? []) seed(s);
  const settings = { ...(config.settings ?? {}) };
  const state = { saved: [], picked: [], submitted: [], cancelled: 0, puts: [], events: [] };
  window.__mock = { snippets, settings, state };

  const summary = (s) => ({
    id: s.id, folderId: s.folderId, abbreviation: s.abbreviation, label: s.label,
    firstLine: s.body.split("\n")[0].slice(0, 120), kind: s.kind, trigger: s.trigger, enabled: s.enabled,
    tags: s.tags, broken: null, uses: null, lastUsed: null, updatedAt: s.updatedAt,
  });

  // A tokenizer for `{name attr="value" ...}` macros: literal text and macros covering the source.
  const parse = (source) => {
    const tokens = [];
    let at = 0;
    const re = /\{([a-z]+)((?:\s+[a-z]+=(?:"[^"]*"|[^\s}]+))*)\s*\}/g;
    let m;
    while ((m = re.exec(source))) {
      if (m.index > at) tokens.push({ kind: "text", start: at, end: m.index, attributes: [] });
      const attributes = [...m[2].matchAll(/([a-z]+)=(?:"([^"]*)"|([^\s}]+))/g)].map((a) => ({ name: a[1], value: a[2] ?? a[3] }));
      tokens.push({ kind: m[1], start: m.index, end: m.index + m[0].length, attributes });
      at = m.index + m[0].length;
    }
    if (at < source.length) tokens.push({ kind: "text", start: at, end: source.length, attributes: [] });
    let depth = 0;
    for (const ch of source) depth += ch === "{" ? 1 : ch === "}" ? -1 : 0;
    const error = depth > 0 ? { code: "E_UNCLOSED_MACRO", start: source.lastIndexOf("{"), end: source.length, snippet: null } : null;
    return { tokens: error ? [] : tokens, error };
  };
  const fieldsOf = (source) =>
    parse(source).tokens.filter((t) => t.kind === "field").map((t) => {
      const a = Object.fromEntries(t.attributes.map((x) => [x.name, x.value]));
      return { name: a.name ?? "Field", kind: a.type === "area" ? "area" : a.type === "toggle" ? "toggle" : a.options ? "choice" : "text", options: a.options ? a.options.split(",") : [], default: a.default ?? "" };
    });

  const listeners = new Map();
  let callbackId = 0;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  /** Sends an event to the page, as the Rust side would. */
  window.__emit = (event, payload) => {
    for (const id of listeners.get(event) ?? []) window.__callbacks.get(id)?.({ event, id: 0, payload });
  };
  window.__callbacks = new Map();
  const rows = (hits) => ({
    total: snippets.size,
    hits: hits.map((s) => ({
      row: { id: s.id, abbreviation: s.abbreviation, label: s.label, folder: "Replies", firstLine: s.body.split("\n")[0], kind: s.kind === "rich" ? "rich" : fieldsOf(s.body).length ? "form" : "plain", enabled: s.enabled, broken: null, needsReview: false, tags: s.tags },
      field: "label", ranges: [],
    })),
  });

  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: config.window }, currentWebview: { windowLabel: config.window, label: config.window } },
    transformCallback(cb) {
      const id = ++callbackId;
      window.__callbacks.set(id, cb);
      return id;
    },
    unregisterCallback() {},
    convertFileSrc: (p) => p,
    async invoke(command, args) {
      switch (command) {
        case "status":
        case "refresh_status":
          return { connection: "connected", service: { version: "0", paused: false, suspended: false, snippets: snippets.size, revision: 1, listener: true, memoryLocked: true } };
        case "set_paused":
          return { connection: "connected", service: { version: "0", paused: args.paused, suspended: false, snippets: snippets.size, revision: 1, listener: true, memoryLocked: true } };
        case "report_first_paint":
        case "dismiss":
        case "palette_ready":
        case "palette_painted":
        case "form_ready":
        case "form_painted":
        case "form_laid_out":
        case "onboarding_open_library":
        case "onboarding_close":
        case "library_take_request":
          return null;
        case "library_list_folders":
          return { profile: "p", folders };
        case "library_list_tags":
          return tags;
        case "library_list_summaries": {
          const all = [...snippets.values()].map(summary);
          return { revision: 1, total: all.length, summaries: all.slice(args.offset, args.offset + args.limit), next: args.offset + args.limit < all.length ? args.offset + args.limit : null };
        }
        case "library_get_snippet":
          return snippets.get(args.id);
        case "library_check_abbreviation": {
          const typed = String(args.abbreviation ?? args.typed ?? "").toLowerCase();
          const other = [...snippets.values()].find((s) => s.abbreviation.toLowerCase() === typed && s.id !== (args.snippet ?? null));
          return { conflict: other ? other.id : null, hides: [], hiddenBy: [] };
        }
        case "library_save_snippet": {
          const d = args.draft;
          const id = d.id ?? `00000000-0000-7000-8000-${String(++seq + 100).padStart(12, "0")}`;
          const prev = snippets.get(id);
          const rev = (prev?.rev ?? -1) + 1;
          const row = { ...(prev ?? {}), ...d, id, rev, createdAt: prev?.createdAt ?? now, updatedAt: now + rev };
          snippets.set(id, row);
          state.saved.push(row);
          return { id, rev, updatedAt: row.updatedAt };
        }
        case "library_delete_snippet":
          snippets.delete(args.id);
          return null;
        case "library_set_enabled": {
          const s = snippets.get(args.id);
          s.enabled = args.enabled;
          s.rev += 1;
          return { id: s.id, rev: s.rev, updatedAt: s.updatedAt };
        }
        case "library_health":
          return { shadowed: [], excluded: [], unused: [] };
        case "template_parse":
          return parse(args.source);
        case "template_preview":
          return { parts: [{ part: "text", text: args.source, blanks: [], clipboard: [] }], cursor: null, fields: fieldsOf(args.source), error: null };
        case "settings_get":
          return settings;
        case "settings_put":
          state.puts.push([args.key, args.value]);
          if (args.value === null) delete settings[args.key];
          else settings[args.key] = args.value;
          return null;
        case "settings_list_exclusions":
          return [{ appId: "mail.exe", displayName: "Mail", createdAt: now }];
        case "settings_put_exclusion":
          return [];
        case "diagnostics_recent":
          return [{ code: "suspended_secure_field", snippet: null, app: null, at: now }];
        case "palette_current":
          return config.open ?? null;
        case "palette_search": {
          const q = String(args.query).toLowerCase();
          return rows([...snippets.values()].filter((s) => `${s.abbreviation} ${s.label} ${s.body}`.toLowerCase().includes(q)));
        }
        case "palette_recent":
          return rows([...snippets.values()].slice(0, 3));
        case "palette_preview":
          return null;
        case "palette_pick":
          state.picked.push(args.action);
          return null;
        case "form_current":
          return config.open ?? null;
        case "form_submit":
          state.submitted.push(args.values);
          return null;
        case "form_cancel":
          state.cancelled += 1;
          return null;
        case "onboarding_permissions":
          return { accessibility: "not_required", inputMonitoring: "not_required" };
        case "onboarding_login_item":
          return "enabled";
        case "onboarding_finish":
          return { paletteHotkey: "Ctrl+Shift+Space", hotkeyRefused: false };
        case "onboarding_restart_helper":
        case "onboarding_open_permission_settings":
          return null;
        case "plugin:event|listen": {
          const list = listeners.get(args.event) ?? [];
          list.push(args.handler);
          listeners.set(args.event, list);
          return ++callbackId;
        }
        default:
          return command.startsWith("plugin:") ? ++callbackId : null;
      }
    },
  };
};
