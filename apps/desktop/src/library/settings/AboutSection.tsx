import { useEffect, useState } from "react";

import type { About } from "./api";
import type { SectionProps } from "./section";

/** The address of the source code (AGPL-3.0-or-later: the source is part of the licence). */
export const SOURCE_URL = "https://github.com/gotolisto/iamb";

/** About: the version, the licence, where the source is, and the log folder (specification §4.7). */
export function AboutSection({ env }: Partial<SectionProps> = {}) {
  const api = env?.settings;
  const [about, setAbout] = useState<About | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    if (!api?.about) return;
    let live = true;
    api.about().then(
      (a) => live && setAbout(a),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [api]);

  const openFolder = () => {
    setProblem(null);
    api?.openLogFolder().catch(() => setProblem(`Iamb could not open the folder. It is ${about?.logFolder ?? "in Iamb’s data folder"}.`));
  };

  return (
    <>
      <dl className="st-about">
        {about && (
          <div>
            <dt>Version</dt>
            <dd>Iamb {about.version}</dd>
          </div>
        )}
        <div>
          <dt>Licence</dt>
          <dd>AGPL-3.0-or-later</dd>
        </div>
        <div>
          <dt>Source code</dt>
          <dd className="mono">{SOURCE_URL}</dd>
        </div>
        {about && (
          <div>
            <dt>Log folder</dt>
            <dd className="mono">{about.logFolder}</dd>
          </div>
        )}
      </dl>
      {about && api && (
        <div className="st-about-actions">
          <button type="button" className="button" onClick={openFolder}>
            Open the log folder
          </button>
          {problem && (
            <p className="field-message error" role="alert">
              {problem}
            </p>
          )}
        </div>
      )}
    </>
  );
}
