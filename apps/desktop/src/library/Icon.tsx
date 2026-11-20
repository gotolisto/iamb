import { ICONS, type IconName } from "./icons";

/** An icon of the set. Decorative unless it has a `title` (then it is an image with a name). */
export function Icon({ name, title }: { name: IconName; title?: string }) {
  return (
    <svg
      className="icon"
      viewBox="0 0 16 16"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
      aria-label={title}
      focusable="false"
      // The markup is the fixed table of icons.ts, never user content.
      dangerouslySetInnerHTML={{ __html: ICONS[name] }}
    />
  );
}
