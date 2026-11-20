import { useEffect, useMemo, useRef, useState } from "react";

import type { Folder, Tag } from "./api";
import { Icon } from "./Icon";
import type { OpenMenu } from "./Menu";
import { type TreeNode, shortcutLabel, visibleTree } from "./model";
import type { SectionId } from "./settings/section";

export type View =
  | { kind: "all" }
  | { kind: "folder"; id: string }
  | { kind: "tag"; id: string }
  | { kind: "health" }
  | { kind: "diagnostics" }
  | { kind: "settings"; section?: SectionId };

export function sameView(a: View, b: View): boolean {
  if (a.kind !== b.kind) return false;
  return "id" in a && "id" in b ? a.id === b.id : true;
}

interface SidebarProps {
  folders: Folder[];
  tags: Tag[];
  total: number;
  folderCounts: ReadonlyMap<string, number>;
  /** Snippets that will not expand, for the badge of Library health. */
  broken: number;
  view: View;
  onView: (view: View) => void;
  onNewFolder: () => void;
  onFolderMenu: (folder: Folder, x: number, y: number) => void;
  onRenameFolder: (folder: Folder) => void;
  onDeleteFolder: (folder: Folder) => void;
  onNewTag: () => void;
  onTagMenu: (tag: Tag, x: number, y: number) => void;
  status: { text: string; paused: boolean; running: boolean };
  onStatus: (menu: OpenMenu) => void;
  onTogglePause: () => void;
  mac: boolean;
}

function viewOfNode(n: TreeNode): View {
  return { kind: "folder", id: n.folder.id };
}

/** The sidebar: All snippets, the folder tree, the tags, and the entries at the foot. */
export function Sidebar(props: SidebarProps) {
  const { folders, tags, view, onView, mac } = props;
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const nodes = useMemo(
    () => visibleTree(folders, props.folderCounts, collapsed),
    [folders, props.folderCounts, collapsed],
  );
  const treeRef = useRef<HTMLUListElement>(null);
  // The item the Tab key lands on: the selected folder, else the first.
  const selectedId = view.kind === "folder" ? view.id : null;
  const [focusId, setFocusId] = useState<string | null>(null);
  const tabbable = nodes.some((n) => n.folder.id === focusId)
    ? focusId
    : nodes.some((n) => n.folder.id === selectedId)
      ? selectedId
      : (nodes[0]?.folder.id ?? null);

  useEffect(() => {
    // A deleted folder leaves no stale focus target.
    if (focusId !== null && !nodes.some((n) => n.folder.id === focusId)) setFocusId(null);
  }, [nodes, focusId]);

  const focusItem = (id: string) => {
    setFocusId(id);
    document.getElementById(`folder-${id}`)?.focus();
  };

  const onTreeKey = (e: React.KeyboardEvent, node: TreeNode) => {
    const at = nodes.findIndex((n) => n.folder.id === node.folder.id);
    const go = (n: TreeNode | undefined) => {
      if (n) {
        e.preventDefault();
        focusItem(n.folder.id);
      }
    };
    switch (e.key) {
      case "ArrowDown":
        go(nodes[at + 1]);
        break;
      case "ArrowUp":
        go(nodes[at - 1]);
        break;
      case "Home":
        go(nodes[0]);
        break;
      case "End":
        go(nodes[nodes.length - 1]);
        break;
      case "ArrowRight":
        if (node.hasChildren && !node.expanded) {
          e.preventDefault();
          setCollapsed((c) => {
            const next = new Set(c);
            next.delete(node.folder.id);
            return next;
          });
        } else if (node.hasChildren) go(nodes[at + 1]);
        break;
      case "ArrowLeft":
        if (node.hasChildren && node.expanded) {
          e.preventDefault();
          setCollapsed((c) => new Set(c).add(node.folder.id));
        } else {
          go(nodes.find((n) => n.folder.id === node.folder.parentId));
        }
        break;
      case "Enter":
        e.preventDefault();
        if (mac) props.onRenameFolder(node.folder);
        else onView(viewOfNode(node));
        break;
      case " ":
        e.preventDefault();
        onView(viewOfNode(node));
        break;
      case "F2":
        e.preventDefault();
        props.onRenameFolder(node.folder);
        break;
      case "Delete":
        e.preventDefault();
        props.onDeleteFolder(node.folder);
        break;
      case "ContextMenu":
      case "F10": {
        if (e.key === "F10" && !e.shiftKey) return;
        e.preventDefault();
        const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
        props.onFolderMenu(node.folder, r.left + 24, r.bottom);
        break;
      }
      default:
        // Typing jumps to the next folder whose name starts with the letter.
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          const letter = e.key.toLowerCase();
          const n = [...nodes.slice(at + 1), ...nodes.slice(0, at + 1)].find((x) =>
            x.folder.name.toLowerCase().startsWith(letter),
          );
          go(n);
        }
    }
  };

  const statusItems = (): OpenMenu["items"] => [
    {
      label: props.status.paused ? "Resume Iamb" : "Pause Iamb",
      onSelect: props.onTogglePause,
      disabled: !props.status.running,
    },
  ];

  return (
    <nav className="pane sidebar" aria-label="Library" data-pane="sidebar">
      <ul className="nav-list" role="list">
        <li>
          <button
            type="button"
            className={view.kind === "all" ? "nav-item selected" : "nav-item"}
            aria-current={view.kind === "all" ? "page" : undefined}
            data-nav-focus={view.kind === "all" ? "" : undefined}
            onClick={() => onView({ kind: "all" })}
          >
            <Icon name="snippet" />
            <span className="nav-label">All snippets</span>
            <span className="count" aria-label={`${props.total} snippets`}>
              {props.total}
            </span>
          </button>
        </li>
      </ul>

      <div className="nav-group">
        <div className="nav-heading">
          <h2 className="overline" id="folders-heading">
            Folders
          </h2>
          <button
            type="button"
            className="icon-button"
            aria-label="New folder"
            title={`New folder (${shortcutLabel({ shift: true, key: "N" }, mac)})`}
            aria-keyshortcuts={mac ? "Meta+Shift+N" : "Control+Shift+N"}
            onClick={props.onNewFolder}
          >
            <Icon name="plus" />
          </button>
        </div>
        <ul ref={treeRef} className="tree" role="tree" aria-labelledby="folders-heading">
          {nodes.map((n) => {
            const selected = selectedId === n.folder.id;
            return (
              <li key={n.folder.id} role="none">
                <div
                  id={`folder-${n.folder.id}`}
                  role="treeitem"
                  tabIndex={tabbable === n.folder.id ? 0 : -1}
                  aria-level={n.depth}
                  aria-selected={selected}
                  aria-expanded={n.hasChildren ? n.expanded : undefined}
                  aria-label={`${n.folder.name}, ${n.count} ${n.count === 1 ? "snippet" : "snippets"}${
                    n.hasChildren ? (n.expanded ? ", expanded" : ", collapsed") : ""
                  }, level ${n.depth}`}
                  data-nav-focus={selected ? "" : undefined}
                  className={selected ? "tree-item selected" : "tree-item"}
                  style={{ paddingInlineStart: `calc(var(--space-2) + ${(n.depth - 1) * 16}px)` }}
                  onClick={() => {
                    setFocusId(n.folder.id);
                    onView(viewOfNode(n));
                  }}
                  onFocus={() => setFocusId(n.folder.id)}
                  onKeyDown={(e) => onTreeKey(e, n)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    props.onFolderMenu(n.folder, e.clientX, e.clientY);
                  }}
                >
                  <span
                    className={`twisty${n.hasChildren ? "" : " none"}${n.expanded ? " open" : ""}`}
                    aria-hidden="true"
                    onClick={(e) => {
                      if (!n.hasChildren) return;
                      e.stopPropagation();
                      setCollapsed((c) => {
                        const next = new Set(c);
                        if (next.has(n.folder.id)) next.delete(n.folder.id);
                        else next.add(n.folder.id);
                        return next;
                      });
                    }}
                  >
                    <Icon name="right" />
                  </span>
                  <Icon name="folder" />
                  <span className="nav-label">{n.folder.name}</span>
                  <span className="count" aria-hidden="true">
                    {n.count}
                  </span>
                </div>
              </li>
            );
          })}
        </ul>
      </div>

      <div className="nav-group">
        <div className="nav-heading">
          <h2 className="overline" id="tags-heading">
            Tags
          </h2>
          <button type="button" className="icon-button" aria-label="New tag" onClick={props.onNewTag}>
            <Icon name="plus" />
          </button>
        </div>
        <ul className="nav-list" role="list" aria-labelledby="tags-heading">
          {tags.map((t) => {
            const selected = view.kind === "tag" && view.id === t.id;
            return (
              <li key={t.id}>
                <button
                  type="button"
                  className={selected ? "nav-item selected" : "nav-item"}
                  aria-current={selected ? "page" : undefined}
                  data-nav-focus={selected ? "" : undefined}
                  aria-label={`${t.name}, ${t.color}, ${t.snippets} ${t.snippets === 1 ? "snippet" : "snippets"}`}
                  onClick={() => onView({ kind: "tag", id: t.id })}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    props.onTagMenu(t, e.clientX, e.clientY);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "F2") {
                      e.preventDefault();
                      const r = e.currentTarget.getBoundingClientRect();
                      props.onTagMenu(t, r.left + 24, r.bottom);
                    }
                  }}
                >
                  <span className="dot" style={{ background: `var(--color-tag-${t.color})` }} aria-hidden="true" />
                  <span className="nav-label">{t.name}</span>
                  <span className="count" aria-hidden="true">
                    {t.snippets}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      </div>

      <div className="nav-foot">
        <ul className="nav-list" role="list">
          <li>
            <button
              type="button"
              className={view.kind === "health" ? "nav-item selected" : "nav-item"}
              aria-current={view.kind === "health" ? "page" : undefined}
              data-nav-focus={view.kind === "health" ? "" : undefined}
              onClick={() => onView({ kind: "health" })}
            >
              <Icon name="pulse" />
              <span className="nav-label">Library health</span>
              {props.broken > 0 && (
                <span className="count count-warning" aria-label={`${props.broken} to fix`}>
                  {props.broken}
                </span>
              )}
            </button>
          </li>
          <li>
            <button
              type="button"
              className={view.kind === "diagnostics" ? "nav-item selected" : "nav-item"}
              aria-current={view.kind === "diagnostics" ? "page" : undefined}
              data-nav-focus={view.kind === "diagnostics" ? "" : undefined}
              onClick={() => onView({ kind: "diagnostics" })}
            >
              <Icon name="log" />
              <span className="nav-label">Why didn’t it expand?</span>
            </button>
          </li>
          <li>
            <button
              type="button"
              className={view.kind === "settings" ? "nav-item selected" : "nav-item"}
              aria-current={view.kind === "settings" ? "page" : undefined}
              data-nav-focus={view.kind === "settings" ? "" : undefined}
              aria-keyshortcuts={mac ? "Meta+," : "Control+,"}
              onClick={() => onView({ kind: "settings" })}
            >
              <Icon name="settings" />
              <span className="nav-label">Settings</span>
            </button>
          </li>
        </ul>
        <button
          type="button"
          className="status-line"
          aria-haspopup="menu"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            props.onStatus({ x: r.left, y: r.top - 44, label: "Iamb", items: statusItems() });
          }}
        >
          <span
            className={`dot ${props.status.running && !props.status.paused ? "on" : "off"}`}
            aria-hidden="true"
          />
          <span>{props.status.text}</span>
        </button>
      </div>
    </nav>
  );
}
