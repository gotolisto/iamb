import type { ReactNode } from "react";

import { Icon } from "./Icon";

/** An empty state of the design system (§4.16): the mark, a line that names the state, a sentence, actions. */
export function EmptyState({
  title,
  children,
  actions,
}: {
  title: string;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="empty">
      <span className="empty-mark">
        <Icon name="iamb" />
      </span>
      <h2 className="empty-title">{title}</h2>
      {children && <p className="empty-text">{children}</p>}
      {actions && <div className="empty-actions">{actions}</div>}
    </div>
  );
}
