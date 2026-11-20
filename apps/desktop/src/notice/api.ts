// The notice window's command and event (src-tauri/src/app/notice_window.rs).

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import type { Invoke } from "../ipc";
import { isNotice, type Notice } from "./model";

/** The Tauri event that says a newer notice is there (`notice::NOTICE_EVENT`). */
export const NOTICE_EVENT = "iamb:notice";

export interface NoticeApi {
  /** The notice on screen, if any. */
  current: () => Promise<Notice | null>;
}

export function createNoticeApi(invoke: Invoke): NoticeApi {
  return {
    current: async () => {
      const notice = await invoke<unknown>("notice_current");
      return isNotice(notice) ? notice : null;
    },
  };
}

export type ListenNotice = (handler: () => void) => Promise<UnlistenFn>;

/** Calls `handler` each time the Rust side shows a notice. */
export const listenNotice: ListenNotice = (handler) =>
  getCurrentWebviewWindow().listen<unknown>(NOTICE_EVENT, () => handler());

export const noticeApi: NoticeApi = createNoticeApi((command, args) => tauriInvoke(command, args));
