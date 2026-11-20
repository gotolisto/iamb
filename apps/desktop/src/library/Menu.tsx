import { type ReactNode, useEffect, useRef } from "react";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  /** Shown after the label, as the platform writes it. */
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  /** The shortcut as `aria-keyshortcuts` writes it, when it is not the one shown (spec §5.6). */
  keys?: string;
}

interface MenuProps {
  items: MenuItem[];
  /** Where the menu opens, in window coordinates. */
  x: number;
  y: number;
  label: string;
  onClose: () => void;
}

/** `aria-keyshortcuts` for a shortcut as the menu writes it ("Ctrl+D", "⌘D"). */
function shortcutKeys(shown: string): string {
  return shown.replace("Ctrl", "Control").replace("⌘", "Meta+").replace("⇧", "Shift+");
}

/** A menu of the design system (§4.11): arrows move, Enter chooses, Esc closes. */
export function Menu({ items, x, y, label, onClose }: MenuProps) {
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    root.current?.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)')?.focus();
    const away = (e: PointerEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("pointerdown", away, true);
    return () => {
      document.removeEventListener("pointerdown", away, true);
      if (before?.isConnected) before.focus();
    };
  }, [onClose]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const buttons = [
      ...(root.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? []),
    ];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      buttons[(at + 1) % buttons.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      buttons[(at - 1 + buttons.length) % buttons.length]?.focus();
    } else if (e.key === "Home") {
      e.preventDefault();
      buttons[0]?.focus();
    } else if (e.key === "End") {
      e.preventDefault();
      buttons[buttons.length - 1]?.focus();
    }
  };

  return (
    <div
      ref={root}
      className="menu"
      role="menu"
      aria-label={label}
      style={{ left: x, top: y }}
      onKeyDown={onKeyDown}
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          className={item.danger ? "menu-item danger" : "menu-item"}
          disabled={item.disabled}
          aria-keyshortcuts={item.keys ?? (item.shortcut ? shortcutKeys(item.shortcut) : undefined)}
          onClick={() => {
            onClose();
            item.onSelect();
          }}
        >
          <span>{item.label}</span>
          {item.shortcut && <kbd className="kbd">{item.shortcut}</kbd>}
        </button>
      ))}
    </div>
  );
}

/** What a menu needs to be opened: where, and what it holds. */
export interface OpenMenu {
  x: number;
  y: number;
  label: string;
  items: MenuItem[];
}

/** The position under an element, for a menu opened from its button. */
export function below(el: Element): { x: number; y: number } {
  const r = el.getBoundingClientRect();
  return { x: Math.max(4, r.left), y: r.bottom + 2 };
}

export function MenuHost({ menu, onClose }: { menu: OpenMenu | null; onClose: () => void }): ReactNode {
  if (!menu) return null;
  return <Menu items={menu.items} x={menu.x} y={menu.y} label={menu.label} onClose={onClose} />;
}
