import { Row } from "./controls";
import { HotkeyRecorder } from "./HotkeyRecorder";
import type { SectionProps } from "./section";
import { useMessages } from "./useMessages";

/** The palette's shortcut when none is set (specification §12, decision 2). */
export const DEFAULT_PALETTE_CHORD = "Ctrl+Shift+Space";

/** Shortcuts: the global shortcut of the search palette (§4.7). */
export function ShortcutsSection({ data, mac, offline }: SectionProps) {
  const { messages, set, fail } = useMessages();
  const chord = data.values?.palette_hotkey;

  const write = (value: string | null) => {
    set("hotkey", null);
    void data.save("palette_hotkey", value).then((failure) => {
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
    <Row
      label="Search snippets"
      help="Opens the search from any app. Backspace clears it, Esc leaves it as it is."
      message={messages.hotkey ?? null}
    >
      {(ids) => (
        <div className="st-recorder-row">
          <HotkeyRecorder
            chord={chord}
            mac={mac}
            labelledBy={ids.label}
            describedBy={ids.describedBy}
            onChord={(c) => write(c)}
            onClear={() => write(null)}
            onRefused={(reason) =>
              set("hotkey", {
                kind: "error",
                text:
                  reason === "modifier"
                    ? mac
                      ? "Add Control, Option or Command"
                      : "Add Ctrl, Alt or the Windows key"
                    : "That key cannot be part of a shortcut",
              })
            }
          />
          {chord !== DEFAULT_PALETTE_CHORD && (
            <button
              type="button"
              className="button"
              disabled={offline}
              onClick={() => write(DEFAULT_PALETTE_CHORD)}
            >
              Use {mac ? "⌃⇧Space" : DEFAULT_PALETTE_CHORD}
            </button>
          )}
        </div>
      )}
    </Row>
  );
}
