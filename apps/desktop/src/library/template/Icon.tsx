import { ICONS } from "./icons";
import type { IconName } from "./macros";

/** An icon of the design system's set, hidden from assistive technology (it sits beside a
 * label that names it). */
export function TplIcon({ name }: { name: IconName }) {
  return (
    <svg className="tpl-icon" viewBox="0 0 16 16" aria-hidden="true">
      {ICONS[name].map((s, i) =>
        s[0] === "path" ? (
          <path key={i} d={s[1]} />
        ) : s[0] === "rect" ? (
          <rect key={i} x={s[1]} y={s[2]} width={s[3]} height={s[4]} rx={s[5]} />
        ) : (
          <circle key={i} cx={s[1]} cy={s[2]} r={s[3]} />
        ),
      )}
    </svg>
  );
}
