import type { StatusView } from "./ipc";

/** One line about the service, as the tray menu shows it (src-tauri/src/app/tray.rs). */
export function statusText(view: StatusView): string {
  switch (view.connection) {
    case "connected":
      if (view.service?.paused) return "Service: paused";
      if (view.service && !view.service.listener) return "Service: not listening";
      return "Service: running";
    case "connecting":
      return "Service: connecting…";
    case "disconnected":
      return "Service: not running, retrying";
  }
}
