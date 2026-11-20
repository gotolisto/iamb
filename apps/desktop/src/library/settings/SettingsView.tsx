import { useState } from "react";

import type { LibraryEnv } from "../env";
import { isMac } from "../model";
import { AboutSection } from "./AboutSection";
import { AppsSection } from "./AppsSection";
import { DatesSection } from "./DatesSection";
import { GeneralSection } from "./GeneralSection";
import { PrivacySection } from "./PrivacySection";
import { SECTIONS, type SectionId, type SectionProps } from "./section";
import { ShortcutsSection } from "./ShortcutsSection";
import { TypingSection } from "./TypingSection";
import { useSettings } from "./useSettings";
import "./settings.css";

const BODIES: Record<SectionId, (props: SectionProps) => React.ReactNode> = {
  general: (p) => <GeneralSection {...p} />,
  typing: (p) => <TypingSection {...p} />,
  dates: (p) => <DatesSection {...p} />,
  shortcuts: (p) => <ShortcutsSection {...p} />,
  apps: (p) => <AppsSection {...p} />,
  privacy: (p) => <PrivacySection {...p} />,
  about: () => <AboutSection />,
};

/**
 * Settings (specification §4.7): a list of sections on the left and the settings of one on the
 * right. Every setting takes effect at once and says so by its own state; there is no Save.
 * It sits under the library window's own heading ("Settings").
 */
export function SettingsView({ env, section }: { env: LibraryEnv; section?: SectionId | undefined }) {
  const api = env.settings;
  if (!api) return <p className="help">This view is not available yet.</p>;
  return <SettingsBody env={env} api={api} section={section} />;
}

function SettingsBody({
  env,
  api,
  section,
}: {
  env: LibraryEnv;
  api: NonNullable<LibraryEnv["settings"]>;
  section: SectionId | undefined;
}) {
  const data = useSettings(env, api);
  const [current, setCurrent] = useState<SectionId>(section ?? "general");
  const mac = env.mac ?? isMac();
  const online = env.connection === "connected";
  const props: SectionProps = { env, data, mac, offline: !online || data.values === null, go: setCurrent };
  const label = SECTIONS.find((s) => s.id === current)?.label ?? "";

  return (
    <div className="settings">
      <nav className="settings-nav" aria-label="Settings sections">
        <ul>
          {SECTIONS.map((s) => (
            <li key={s.id}>
              <button
                type="button"
                className={s.id === current ? "nav-item selected" : "nav-item"}
                aria-current={s.id === current ? "page" : undefined}
                onClick={() => setCurrent(s.id)}
              >
                {s.label}
              </button>
            </li>
          ))}
        </ul>
      </nav>
      <section className="settings-body" aria-labelledby="settings-section">
        <h2 id="settings-section" className="settings-heading">
          {label}
        </h2>
        {data.error && (
          <div className="banner danger" role="alert">
            <p>{data.error}</p>
            <button type="button" className="button" onClick={() => void data.reload()}>
              Try again
            </button>
          </div>
        )}
        {data.values === null && !data.error ? (
          <p className="help">Reading the settings…</p>
        ) : (
          BODIES[current](props)
        )}
      </section>
    </div>
  );
}
