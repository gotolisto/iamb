import { useEffect, useState } from "react";

import { describeFailure } from "../errors";
import type { LoginState } from "./api";
import { Choice, Row, Switch } from "./controls";
import type { SectionProps } from "./section";
import { type Theme, applyTheme, themeOf } from "./theme";
import { useMessages } from "./useMessages";

const THEMES: { value: Theme; label: string }[] = [
  { value: "system", label: "Follow the system" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

/** General: start at login, pause, the tray icon and the appearance (specification §4.7). */
export function GeneralSection({ env, data, mac, offline }: SectionProps) {
  const { messages, set, fail } = useMessages();
  // Start at login is the system's, not the service's: the UI host asks the system.
  const loginItem = env.settings?.loginItem;
  const [login, setLogin] = useState<LoginState | null>(null);
  useEffect(() => {
    if (!loginItem) return;
    let live = true;
    loginItem().then(
      (state) => live && setLogin(state),
      () => live && setLogin("unavailable"),
    );
    return () => {
      live = false;
    };
  }, [loginItem]);
  const startsAtLogin = login === "enabled" || login === "requires_approval";
  const where = mac ? "Applications folder" : "Start menu";
  const values = data.values ?? {};
  const theme = themeOf(values["ui.theme"]);
  const trayShown = values["ui.tray_icon"] !== "off";

  useEffect(() => applyTheme(theme), [theme]);

  return (
    <>
      {loginItem && (
        <Row
          label="Start Iamb when I log in"
          help={
            login === "requires_approval"
              ? "macOS asks you to approve this once, in System Settings, under Login Items."
              : login === "unavailable"
                ? `This copy of Iamb cannot start at login. Open it from the ${where}.`
                : undefined
          }
          message={messages.login ?? null}
        >
          {(ids) => (
            <Switch
              on={startsAtLogin}
              labelledBy={ids.label}
              describedBy={ids.describedBy}
              disabled={login === null || login === "unavailable"}
              onChange={(on) => {
                set("login", null);
                loginItem(on).then(setLogin, () =>
                  set("login", {
                    kind: "error",
                    text: `Iamb could not change this. It is not a problem: you can start it from the ${where}.`,
                  }),
                );
              }}
            />
          )}
        </Row>
      )}
      <Row
        label="Expand snippets as I type"
        help="Turn this off to pause Iamb everywhere. The menu has the same switch."
        message={messages.pause ?? null}
      >
        {(ids) => (
          <Switch
            on={!env.paused}
            labelledBy={ids.label}
            describedBy={ids.describedBy}
            disabled={offline}
            onChange={(on) => {
              fail("pause", null);
              env.api.setPaused(!on).catch((e: unknown) => fail("pause", describeFailure(e, "set_paused")));
            }}
          />
        )}
      </Row>
      <Row
        label={`Show the Iamb icon in the ${mac ? "menu bar" : "notification area"}`}
        help={`Iamb keeps running without it. Open it from the ${mac ? "Dock" : "Start menu"}.`}
        message={messages.tray ?? null}
      >
        {(ids) => (
          <Switch
            on={trayShown}
            labelledBy={ids.label}
            describedBy={ids.describedBy}
            disabled={offline}
            onChange={(on) => {
              void data.save("ui.tray_icon", on ? null : "off").then((f) => fail("tray", f));
            }}
          />
        )}
      </Row>
      <Row label="Appearance" message={messages.theme ?? null}>
        {(ids) => (
          <Choice
            label={ids.label}
            value={theme}
            options={THEMES}
            onChange={(next) => {
              applyTheme(next);
              void data.save("ui.theme", next === "system" ? null : next).then((f) => {
                fail("theme", f);
                if (f) applyTheme(theme);
              });
            }}
          />
        )}
      </Row>
    </>
  );
}
