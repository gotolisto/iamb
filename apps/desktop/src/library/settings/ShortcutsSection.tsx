import { Row } from "./controls";
import { HotkeyRecorder } from "./HotkeyRecorder";
import { refusedText } from "./model";
import type { SectionProps } from "./section";
import { useMessages } from "./useMessages";

/** The palette's shortcut when none is set (specification §12, decision 2). */
export const DEFAULT_PALETTE_CHORD = "Ctrl+Shift+Space";

/** The global shortcuts of §5.1, by setting: the palette's, then the two that are unset by default. */
const SHORTCUTS = [
  {
    key: "palette_hotkey",
    label: "Search snippets",
    help: "Opens the search from any app. Backspace clears it, Esc leaves it as it is.",
    fallback: DEFAULT_PALETTE_CHORD,
  },
  {
    key: "reopen_form_hotkey",
    label: "Fill in the last form again",
    help: "Opens the last fill-in form again, with your answers, in the app you are in. Not set at first.",
    fallback: null,
  },
  {
    key: "pause_hotkey",
    label: "Pause or resume",
    help: "Turns expansion off or on everywhere, as the menu's Pause Iamb does. Not set at first.",
    fallback: null,
  },
] as const;

/** Shortcuts: the global shortcuts (§4.7, §5.1). */
export function ShortcutsSection(props: SectionProps) {
  return (
    <>
      {SHORTCUTS.map((s) => (
        <ShortcutRow key={s.key} {...props} setting={s.key} label={s.label} help={s.help} fallback={s.fallback} />
      ))}
    </>
  );
}

function ShortcutRow({
  data,
  mac,
  offline,
  setting,
  label,
  help,
  fallback,
}: SectionProps & { setting: string; label: string; help: string; fallback: string | null }) {
  const { messages, set, fail } = useMessages();
  const chord = data.values?.[setting];

  const write = (value: string | null) => {
    set("hotkey", null);
    void data.save(setting, value).then((failure) => {
      if (!failure) return;
      fail(
        "hotkey",
        failure,
        failure.kind === "conflict"
          ? "Another app already uses this shortcut"
          : failure.kind === "invalid"
            ? "Iamb cannot use that shortcut. Try another one."
            : failure.text,
      );
    });
  };

  return (
    <Row label={label} help={help} message={messages.hotkey ?? null}>
      {(ids) => (
        <div className="st-recorder-row">
          <HotkeyRecorder
            chord={chord}
            mac={mac}
            labelledBy={ids.label}
            describedBy={ids.describedBy}
            onChord={(c) => write(c)}
            onClear={() => write(null)}
            onRefused={(reason) => set("hotkey", { kind: "error", text: refusedText(reason, mac) })}
          />
          {fallback !== null && chord !== fallback && (
            <button type="button" className="button" disabled={offline} onClick={() => write(fallback)}>
              Use {mac ? "⌃⇧Space" : fallback}
            </button>
          )}
        </div>
      )}
    </Row>
  );
}
