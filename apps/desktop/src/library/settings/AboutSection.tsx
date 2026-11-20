/** The address of the source code (AGPL-3.0-or-later: the source is part of the licence). */
export const SOURCE_URL = "https://github.com/gotolisto/iamb";

/** About: the licence and where the source is. */
export function AboutSection() {
  return (
    <dl className="st-about">
      <div>
        <dt>Licence</dt>
        <dd>AGPL-3.0-or-later</dd>
      </div>
      <div>
        <dt>Source code</dt>
        <dd className="mono">{SOURCE_URL}</dd>
      </div>
    </dl>
  );
}
