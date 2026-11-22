import { useState } from "react";

import { Badges } from "./controls";
import { chordBadges, chordSpoken, recordChord, type RefusedReason } from "./model";

/**
 * The hotkey recorder (design system §4.2): a field that says "Press the keys" while it has the
 * focus and shows the chord as key badges. Esc leaves it unchanged, Backspace clears it, and
 * Tab and Shift+Tab move on, so the keyboard is never trapped (WCAG 2.1.2).
 */
export function HotkeyRecorder({
  chord,
  mac,
  labelledBy,
  describedBy,
  busy,
  onChord,
  onClear,
  onRefused,
}: {
  chord: string | undefined;
  mac: boolean;
  labelledBy: string;
  describedBy?: string;
  busy?: boolean;
  onChord: (chord: string) => void;
  onClear: () => void;
  onRefused: (reason: RefusedReason) => void;
}) {
  const [listening, setListening] = useState(false);
  const [held, setHeld] = useState<string[]>([]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
    if (e.key === "Tab" && plain) return; // moves on
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape" && plain && !e.shiftKey) {
      e.currentTarget.blur();
      return;
    }
    if (e.key === "Backspace" && plain && !e.shiftKey) {
      setHeld([]);
      onClear();
      return;
    }
    const result = recordChord(e.nativeEvent, mac);
    if (result.kind === "waiting") {
      setHeld(
        [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Meta"].filter(
          (p): p is string => typeof p === "string",
        ),
      );
    } else if (result.kind === "chord") {
      setHeld([]);
      onChord(result.chord);
    } else {
      setHeld([]);
      onRefused(result.reason);
    }
  };

  const shown = listening && held.length > 0 ? held.join("+") : chord;
  return (
    <button
      type="button"
      className={listening ? "input st-recorder listening" : "input st-recorder"}
      aria-labelledby={labelledBy}
      {...(describedBy ? { "aria-describedby": describedBy } : {})}
      aria-busy={busy ? true : undefined}
      onFocus={() => setListening(true)}
      onBlur={() => {
        setListening(false);
        setHeld([]);
      }}
      onKeyDown={onKeyDown}
    >
      {listening && held.length === 0 ? (
        <span className="help">Press the keys</span>
      ) : shown ? (
        <Badges parts={chordBadges(shown, mac)} spoken={chordSpoken(shown, mac)} />
      ) : (
        <span className="help">Not set</span>
      )}
    </button>
  );
}
