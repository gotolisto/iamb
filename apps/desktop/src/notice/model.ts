// The words of a notice (docs/ux/spec.md §4.11 and the "In a notice" column of §6.2; flow 4's
// and flow 5's error tables). The Rust side (src-tauri/src/notice.rs) sends the reason's code,
// the snippet's label, whether the request was a form's, and the palette's shortcut; nothing
// typed and no form value.

/** A notice, as `notice_current` answers it (`Notice` in src-tauri/src/notice.rs). */
export interface Notice {
  id: number;
  code: string;
  /** The snippet's label (its abbreviation when it has none); empty when it could not be read. */
  label: string;
  /** A form's: the answers are kept. */
  form: boolean;
  /** The palette's shortcut, as Settings writes it. */
  shortcut: string | null;
}

export function isNotice(v: unknown): v is Notice {
  if (typeof v !== "object" || v === null) return false;
  const n = v as Record<string, unknown>;
  return (
    typeof n.id === "number" &&
    typeof n.code === "string" &&
    typeof n.label === "string" &&
    typeof n.form === "boolean" &&
    (n.shortcut === null || typeof n.shortcut === "string")
  );
}

/** The reason's sentence (§6.2, "In a notice"). */
const REASONS: Record<string, string> = {
  refused_secure_field: "The cursor is in a password field. Iamb does not type there.",
  refused_excluded_app: "You excluded this app.",
  refused_unknown_focus: "Iamb cannot see into this window, so it stays out.",
  refused_composing: "Your keyboard is composing text. Finish the word and try again.",
  request_cancelled: "You moved to another app before the text could be typed.",
  // Not in §6.2: the window did not come back to the front after the form or the palette.
  request_focus_timeout: "Its window did not come back to the front in time.",
  injection_busy: "Iamb was still typing the previous snippet. Try again.",
  unknown_snippet: "That snippet was deleted.",
  // Not in §6.2: the window a form was for was closed, or its app quit (src-tauri/src/form.rs).
  form_target_gone: "Its window was closed.",
};

/** What a notice says: its title and its one sentence. */
export function noticeText(n: Notice): { title: string; sentence: string } {
  if (n.code === "no_last_form") {
    return {
      title: "There is no earlier form to fill in again",
      sentence: "Answers are kept for 30 minutes.",
    };
  }
  const title = n.label ? `“${n.label}” was not inserted` : "Nothing was inserted";
  const reason = n.code.startsWith("E_")
    ? "This snippet has an error."
    : (REASONS[n.code] ?? "Iamb could not type it here.");
  if (!n.form || n.code === "unknown_snippet") return { title, sentence: reason };
  const where = n.shortcut
    ? `press ${n.shortcut} where the text should go`
    : "open the search palette where the text should go";
  return { title, sentence: `${reason} Your answers are kept for 30 minutes: ${where}.` };
}
