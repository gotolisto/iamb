import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { blockReload } from "./reload";
import "./styles/tokens.css";
import "./styles/base.css";

/** The window's title bar follows the page's title, which names the view ("Iamb – Settings"):
 * a screen reader reads it when the window gets the focus (spec §8, 2.4.2). */
function followTitle(label: string): void {
  if (label !== "library" && label !== "onboarding") return;
  const title = document.querySelector("title");
  if (!title) return;
  const window = getCurrentWindow();
  new MutationObserver(() => void window.setTitle(document.title).catch(() => {})).observe(title, {
    childList: true,
  });
}

// F5 and Ctrl+R would reload the page and drop what was typed (src/reload.ts).
blockReload(window);

const root = document.getElementById("root");
if (root) {
  followTitle(getCurrentWindow().label);
  createRoot(root).render(
    <StrictMode>
      <App label={getCurrentWindow().label} />
    </StrictMode>,
  );
}
