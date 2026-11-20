import { useId, useState } from "react";

import { type Defaults, type Folder, MAX_FOLDER_DEPTH, TAG_COLORS, type Tag, type TagColor } from "./api";
import { Dialog } from "./Dialog";
import { folderLevel, folderOptions, subtreeIds } from "./model";

/** Button order: the main action is rightmost on macOS and leftmost on Windows (design system §4.1). */
export function Buttons({
  mac,
  primary,
  cancel,
  extra,
}: {
  mac: boolean;
  primary: React.ReactNode;
  cancel: React.ReactNode;
  extra?: React.ReactNode;
}) {
  return mac ? (
    <>
      {extra}
      <span className="spacer" />
      {cancel}
      {primary}
    </>
  ) : (
    <>
      {primary}
      {cancel}
      <span className="spacer" />
      {extra}
    </>
  );
}

// ---- Leaving with unsaved changes -----------------------------------------------------------

export function LeaveDialog({
  label,
  mac,
  busy,
  error,
  onSave,
  onDiscard,
  onCancel,
}: {
  label: string;
  mac: boolean;
  busy: boolean;
  error: string | null;
  onSave: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}) {
  return (
    <Dialog
      title={`Save your changes to “${label}”?`}
      onCancel={onCancel}
      role="alertdialog"
      actions={
        <Buttons
          mac={mac}
          primary={
            <button type="button" className="button primary" data-primary data-autofocus disabled={busy} onClick={onSave}>
              Save
              <kbd className="kbd">⏎</kbd>
            </button>
          }
          cancel={
            <button type="button" className="button" onClick={onCancel}>
              Cancel
            </button>
          }
          extra={
            <button type="button" className="button" disabled={busy} onClick={onDiscard}>
              Do not save
            </button>
          }
        />
      }
    >
      <p>If you do not save, the changes you made to this snippet are lost.</p>
      {error && (
        <p className="field-message error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}

// ---- Confirm --------------------------------------------------------------------------------

export function ConfirmDialog({
  title,
  body,
  confirm,
  mac,
  onConfirm,
  onCancel,
}: {
  title: string;
  body: string;
  confirm: string;
  mac: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Dialog
      title={title}
      role="alertdialog"
      onCancel={onCancel}
      actions={
        <Buttons
          mac={mac}
          primary={
            <button type="button" className="button danger" onClick={onConfirm}>
              {confirm}
            </button>
          }
          cancel={
            <button type="button" className="button" data-autofocus onClick={onCancel}>
              Cancel
            </button>
          }
        />
      }
    >
      <p>{body}</p>
    </Dialog>
  );
}

// ---- Folders --------------------------------------------------------------------------------

export interface FolderForm {
  name: string;
  parentId: string | null;
  defaults: Defaults;
}

/** A new folder, or the settings of one: name, where it is, and the defaults of new snippets. */
export function FolderDialog({
  folder,
  folders,
  initialParent,
  mac,
  error,
  busy,
  onSave,
  onCancel,
}: {
  folder: Folder | null;
  folders: Folder[];
  initialParent: string | null;
  mac: boolean;
  error: string | null;
  busy: boolean;
  onSave: (form: FolderForm) => void;
  onCancel: () => void;
}) {
  const ids = [useId(), useId(), useId(), useId(), useId()] as const;
  const [name, setName] = useState(folder?.name ?? "");
  const [parentId, setParentId] = useState<string | null>(folder ? folder.parentId : initialParent);
  const [prefix, setPrefix] = useState(folder?.defaults.abbreviationPrefix ?? "");
  const [trigger, setTrigger] = useState(folder?.defaults.trigger ?? "");
  const [caseMode, setCaseMode] = useState(folder?.defaults.caseMode ?? "");
  // A folder cannot move into itself or below its own folders, nor deeper than the limit.
  const own = folder ? subtreeIds(folders, folder.id) : new Set<string>();
  const height = folder ? Math.max(...[...own].map((id) => folderLevel(folders, id) - folderLevel(folders, folder.id) + 1)) : 1;
  const parents = folderOptions(folders).filter(
    (o) => !own.has(o.id) && folderLevel(folders, o.id) + height <= MAX_FOLDER_DEPTH,
  );
  const trimmed = name.trim();
  const submit = () => {
    if (trimmed === "") return;
    onSave({
      name: trimmed,
      parentId,
      defaults: {
        abbreviationPrefix: prefix === "" ? null : prefix,
        trigger: trigger === "" ? null : (trigger as Defaults["trigger"]),
        caseMode: caseMode === "" ? null : (caseMode as Defaults["caseMode"]),
      },
    });
  };
  return (
    <Dialog
      title={folder ? "Folder settings" : "New folder"}
      onCancel={onCancel}
      actions={
        <Buttons
          mac={mac}
          primary={
            <button type="button" className="button primary" data-primary disabled={trimmed === "" || busy} onClick={submit}>
              {folder ? "Save" : "Create"}
            </button>
          }
          cancel={
            <button type="button" className="button" onClick={onCancel}>
              Cancel
            </button>
          }
        />
      }
    >
      <div className="field">
        <label htmlFor={ids[0]}>Name</label>
        <input id={ids[0]} className="input" data-autofocus value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field">
        <label htmlFor={ids[1]}>Inside</label>
        <select id={ids[1]} className="input" value={parentId ?? ""} onChange={(e) => setParentId(e.target.value || null)}>
          <option value="">Top level</option>
          {parents.map((o) => (
            <option key={o.id} value={o.id}>
              {" ".repeat(o.depth - 1)}
              {o.label}
            </option>
          ))}
        </select>
      </div>
      <div className="field">
        <label htmlFor={ids[2]}>
          Abbreviation prefix <span className="optional">optional</span>
        </label>
        <input
          id={ids[2]}
          className="input mono"
          value={prefix}
          onChange={(e) => setPrefix(e.target.value.replace(/\s/g, ""))}
          aria-describedby={`${ids[2]}-help`}
        />
        <p id={`${ids[2]}-help`} className="help">
          New snippets in this folder start with it.
        </p>
      </div>
      <div className="field">
        <label htmlFor={ids[3]}>New snippets expand</label>
        <select id={ids[3]} className="input" value={trigger} onChange={(e) => setTrigger(e.target.value as typeof trigger)}>
          <option value="">As Settings says</option>
          <option value="delimiter">After a space or punctuation</option>
          <option value="immediate">Immediately</option>
        </select>
      </div>
      <div className="field">
        <label htmlFor={ids[4]}>Capital letters in new snippets</label>
        <select id={ids[4]} className="input" value={caseMode} onChange={(e) => setCaseMode(e.target.value as typeof caseMode)}>
          <option value="">As Settings says</option>
          <option value="exact">Exactly as written</option>
          <option value="insensitive">Ignore</option>
          <option value="adaptive">Ignore, and follow mine</option>
        </select>
      </div>
      {error && (
        <p className="field-message error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}

// ---- Tags -----------------------------------------------------------------------------------

export function TagDialog({
  tag,
  mac,
  error,
  busy,
  onSave,
  onDelete,
  onCancel,
}: {
  tag: Tag | null;
  mac: boolean;
  error: string | null;
  busy: boolean;
  onSave: (name: string, color: TagColor) => void;
  onDelete?: () => void;
  onCancel: () => void;
}) {
  const nameId = useId();
  const [name, setName] = useState(tag?.name ?? "");
  const [color, setColor] = useState<TagColor>(tag?.color ?? "blue");
  const trimmed = name.trim();
  return (
    <Dialog
      title={tag ? "Edit tag" : "New tag"}
      onCancel={onCancel}
      actions={
        <Buttons
          mac={mac}
          primary={
            <button type="button" className="button primary" data-primary disabled={trimmed === "" || busy} onClick={() => onSave(trimmed, color)}>
              {tag ? "Save" : "Create"}
            </button>
          }
          cancel={
            <button type="button" className="button" onClick={onCancel}>
              Cancel
            </button>
          }
          extra={
            onDelete && (
              <button type="button" className="button danger-quiet" disabled={busy} onClick={onDelete}>
                Delete tag
              </button>
            )
          }
        />
      }
    >
      <div className="field">
        <label htmlFor={nameId}>Name</label>
        <input id={nameId} className="input" data-autofocus value={name} maxLength={40} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field" role="radiogroup" aria-label="Colour">
        <span className="label">Colour</span>
        <div className="swatches">
          {TAG_COLORS.map((c) => (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={color === c}
              aria-label={c}
              className={color === c ? "swatch checked" : "swatch"}
              tabIndex={color === c ? 0 : -1}
              onClick={() => setColor(c)}
              onKeyDown={(e) => {
                const i = TAG_COLORS.indexOf(c);
                const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
                if (step !== 0) {
                  e.preventDefault();
                  const next = TAG_COLORS[(i + step + TAG_COLORS.length) % TAG_COLORS.length] ?? c;
                  setColor(next);
                  (e.currentTarget.parentElement?.querySelector<HTMLElement>(`[aria-label="${next}"]`))?.focus();
                }
              }}
            >
              <span className="dot big" style={{ background: `var(--color-tag-${c})` }} aria-hidden="true" />
            </button>
          ))}
        </div>
      </div>
      {error && (
        <p className="field-message error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}
