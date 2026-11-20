// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { stylesheet, undefinedTokens } from "../../scripts/stylesheet.mjs";
import { COMMANDS, type CommandName } from "../ipc";
import { createNoticeApi } from "./api";
import { isNotice, noticeText, type Notice } from "./model";
import { NoticeView } from "./NoticeView";

afterEach(cleanup);

const notice = (over: Partial<Notice> = {}): Notice => ({
  id: 1,
  code: "refused_secure_field",
  label: "Late delivery",
  form: false,
  shortcut: null,
  ...over,
});

describe("a notice's words (spec §4.11, §6.2; flows 4 and 5)", () => {
  it("names the snippet and the reason of a refused pick", () => {
    expect(noticeText(notice())).toEqual({
      title: "“Late delivery” was not inserted",
      sentence: "The cursor is in a password field. Iamb does not type there.",
    });
    expect(noticeText(notice({ code: "injection_busy" })).sentence).toBe(
      "Iamb was still typing the previous snippet. Try again.",
    );
    expect(noticeText(notice({ code: "E_SNIPPET_NOT_FOUND" })).sentence).toBe("This snippet has an error.");
  });

  it("says where a refused form's answers went", () => {
    expect(noticeText(notice({ form: true, shortcut: "Ctrl+Shift+Space" })).sentence).toBe(
      "The cursor is in a password field. Iamb does not type there. Your answers are kept for 30 minutes: press Ctrl+Shift+Space where the text should go.",
    );
    expect(noticeText(notice({ code: "form_target_gone", form: true })).sentence).toBe(
      "Its window was closed. Your answers are kept for 30 minutes: open the search palette where the text should go.",
    );
    expect(noticeText(notice({ code: "request_cancelled", form: true, shortcut: "Ctrl+Shift+Space" })).sentence).toBe(
      "You moved to another app before the text could be typed. Your answers are kept for 30 minutes: press Ctrl+Shift+Space where the text should go.",
    );
  });

  it("has the specification's sentences for a deleted snippet and no earlier form", () => {
    expect(noticeText(notice({ code: "unknown_snippet", label: "", form: true }))).toEqual({
      title: "Nothing was inserted",
      sentence: "That snippet was deleted.",
    });
    expect(noticeText(notice({ code: "no_last_form", label: "", form: true }))).toEqual({
      title: "There is no earlier form to fill in again",
      sentence: "Answers are kept for 30 minutes.",
    });
  });

  it("fetches the notice with its one command", async () => {
    const calls: string[] = [];
    const api = createNoticeApi(<T,>(command: CommandName) => {
      calls.push(command);
      return Promise.resolve(notice() as T);
    });
    expect(await api.current()).toEqual(notice());
    expect(calls).toEqual(["notice_current"]);
    expect(COMMANDS.filter((c) => c.startsWith("notice_"))).toEqual(["notice_current"]);
  });

  it("checks what the Rust side sends", () => {
    expect(isNotice(notice())).toBe(true);
    expect(isNotice({ ...notice(), shortcut: 3 })).toBe(false);
    expect(isNotice(null)).toBe(false);
  });
});

describe("the notice window", () => {
  it("shows the notice as an alert, and the newer one when told", async () => {
    let current: Notice | null = notice();
    let told: (() => void) | undefined;
    const api = createNoticeApi(<T,>() => Promise.resolve(current as T));
    render(
      <NoticeView
        api={api}
        listen={(h) => {
          told = h;
          return Promise.resolve(() => undefined);
        }}
      />,
    );
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      "“Late delivery” was not inserted" + "The cursor is in a password field. Iamb does not type there.",
    );
    current = notice({ id: 2, code: "refused_excluded_app", label: "Opening hours" });
    await act(async () => {
      told?.();
      await Promise.resolve();
    });
    expect((await screen.findByRole("alert")).textContent).toContain("You excluded this app.");
  });

  it("uses only design tokens that exist, and never animates", () => {
    const { used, missing } = undefinedTokens("notice/notice.css");
    expect(used).toBeGreaterThan(5);
    expect(missing).toEqual([]);
    expect(stylesheet("notice/notice.css")).not.toMatch(/transition|animation/);
  });
});
