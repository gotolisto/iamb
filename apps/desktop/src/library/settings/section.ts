import type { LibraryEnv } from "../env";
import type { SettingsData } from "./useSettings";

/** What every section of Settings receives. */
export interface SectionProps {
  env: LibraryEnv;
  data: SettingsData;
  mac: boolean;
  /** The service does not answer: nothing can be changed. */
  offline: boolean;
  /** Moves to another section of Settings. */
  go: (section: SectionId) => void;
}

export type SectionId = "general" | "typing" | "dates" | "shortcuts" | "apps" | "privacy" | "about";

export const SECTIONS: { id: SectionId; label: string }[] = [
  { id: "general", label: "General" },
  { id: "typing", label: "Typing" },
  { id: "dates", label: "Dates" },
  { id: "shortcuts", label: "Shortcuts" },
  { id: "apps", label: "Apps" },
  { id: "privacy", label: "Privacy" },
  { id: "about", label: "About" },
];

export function isSectionId(v: unknown): v is SectionId {
  return SECTIONS.some((s) => s.id === v);
}
