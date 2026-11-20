// Appearance (Settings → General): the tokens follow the system unless `data-theme` says
// otherwise (`styles/tokens.css`).

export type Theme = "system" | "light" | "dark";

export function themeOf(stored: string | undefined): Theme {
  return stored === "light" || stored === "dark" ? stored : "system";
}

/** Shows the window in `theme`. */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
  if (theme === "system") delete root.dataset.theme;
  else root.dataset.theme = theme;
}
