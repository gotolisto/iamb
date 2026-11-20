// A page reload must not drop what the user typed (the Phase 3 UI review's finding 5).
//
// WebView2 keeps its browser keys: F5, Ctrl+R and Ctrl+Shift+R reload the page, which would
// discard the editor's unsaved draft without the "Not saved" guard of spec §4.3, and the answers
// typed into the fill-in form so far. Tauri cannot turn those keys off without native code, so
// every window stops them before the web view acts on them, and the library window also asks
// before an unload while the editor has unsaved edits (the browser's own question, as a last
// resort: the keys are stopped first).

/** Whether `e` is a key a web view reloads the page with. */
export function isReloadKey(e: KeyboardEvent): boolean {
  if (e.key === "F5") return true;
  return (e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "r";
}

/** Stops the reload keys in `target` (the window); returns the function that removes it. */
export function blockReload(target: Window): () => void {
  const onKeyDown = (e: KeyboardEvent) => {
    if (isReloadKey(e)) e.preventDefault();
  };
  target.addEventListener("keydown", onKeyDown, { capture: true });
  return () => target.removeEventListener("keydown", onKeyDown, { capture: true });
}

/** Asks before `target` unloads while `blocked()` says there is something to lose; returns the
 * function that removes it. */
export function guardUnload(target: Window, blocked: () => boolean): () => void {
  const onBeforeUnload = (e: BeforeUnloadEvent) => {
    if (!blocked()) return;
    e.preventDefault();
    // Older engines ask only when a return value is set.
    e.returnValue = "";
  };
  target.addEventListener("beforeunload", onBeforeUnload);
  return () => target.removeEventListener("beforeunload", onBeforeUnload);
}
