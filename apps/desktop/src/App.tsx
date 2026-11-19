import { FormView } from "./form/FormView";
import { LibraryView } from "./library/LibraryView";
import { PaletteView } from "./palette/PaletteView";

/** The three windows load one page; the window's label picks the view (src-tauri/src/app/windows.rs). */
export function App({ label }: { label: string }) {
  switch (label) {
    case "form":
      return <FormView />;
    case "palette":
      return <PaletteView />;
    default:
      return <LibraryView />;
  }
}
