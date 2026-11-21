// The command layer: what src-tauri answers, from a generated library. Invented text only.
/**
 * `addInitScript(mock, count)`: stands in for the Tauri runtime of the library window, so that
 * the built front end runs in a plain browser (the scroll measurement, screenshots). The
 * `library_*` commands are answered from a generated library of `count` snippets; everything
 * is invented text. Self-contained: Playwright sends its source into the page.
 */
export const mock = (count) => {
  const words = ["order", "invoice", "thanks", "delivery", "refund", "hours", "address", "signature", "welcome", "reminder"];
  const folders = [
    { id: "f1", parentId: null, name: "Replies", position: 0 },
    { id: "f2", parentId: "f1", name: "Shop", position: 0 },
    { id: "f3", parentId: null, name: "Signatures", position: 1 },
    { id: "f4", parentId: null, name: "Dates and times", position: 2 },
  ].map((f) => ({ ...f, defaults: { abbreviationPrefix: null, trigger: null, caseMode: null } }));
  const tags = ["urgent", "español", "team", "draft", "legal", "sales", "support", "later"].map((name, i) => ({
    id: `t${i}`,
    name,
    color: ["red", "orange", "yellow", "green", "teal", "blue", "purple", "grey"][i],
    snippets: 0,
  }));
  const summaries = Array.from({ length: count }, (_, i) => ({
    id: `00000000-0000-7000-8000-${String(i).padStart(12, "0")}`,
    folderId: folders[i % folders.length].id,
    abbreviation: `;${words[i % words.length].slice(0, 3)}${i}`,
    label: `${words[(i * 7) % words.length]} ${words[(i * 3) % words.length]} ${i}`,
    firstLine: i % 5 === 0 ? `Hello {field name="Name"}, thank you for your ${words[i % words.length]}.` : `Thank you for your ${words[i % words.length]} number ${i}.`,
    hasFields: i % 5 === 0,
    kind: "plain",
    trigger: "delimiter",
    enabled: i % 17 !== 0,
    tags: i % 3 === 0 ? [`t${i % 8}`] : [],
    broken: i % 251 === 0 ? "E_UNCLOSED_MACRO" : null,
    updatedAt: 1_700_000_000_000 + i,
  }));
  for (const s of summaries) for (const t of s.tags) tags[Number(t.slice(1))].snippets += 1;
  const listeners = new Map();
  let callbackId = 0;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "library" }, currentWebview: { windowLabel: "library", label: "library" } },
    transformCallback(cb) {
      const id = ++callbackId;
      listeners.set(id, cb);
      return id;
    },
    unregisterCallback() {},
    convertFileSrc: (p) => p,
    async invoke(command, args) {
      switch (command) {
        case "status":
        case "refresh_status":
          return { connection: "connected", service: { version: "0", paused: false, suspended: false, snippets: count, revision: 1, listener: true, memoryLocked: true } };
        case "library_list_folders":
          return { profile: "p", folders };
        case "library_list_tags":
          return tags;
        case "library_list_summaries":
          return {
            revision: 1,
            total: count,
            summaries: summaries.slice(args.offset, args.offset + args.limit),
            next: args.offset + args.limit < count ? args.offset + args.limit : null,
          };
        case "library_get_snippet": {
          const s = summaries.find((x) => x.id === args.id);
          return { ...s, body: s.firstLine, caseMode: "insensitive", wordStart: true, rev: 0, createdAt: 1, updatedAt: s.updatedAt };
        }
        case "library_check_abbreviation":
          return { conflict: null, hides: [], hiddenBy: [] };
        case "report_first_paint":
          return null;
        default:
          return command.startsWith("plugin:event") ? ++callbackId : null;
      }
    },
  };
};

