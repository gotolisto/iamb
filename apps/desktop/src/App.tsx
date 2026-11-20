import { FormView } from "./form/FormView";
import { LibraryView } from "./library/LibraryView";
import { NoticeView } from "./notice/NoticeView";
import { OnboardingView } from "./onboarding/OnboardingView";
import { PaletteView } from "./palette/PaletteView";

/** The windows load one page; the window's label picks the view (src-tauri/src/app/windows.rs). */
export function App({ label }: { label: string }) {
  switch (label) {
    case "form":
      return <FormView />;
    case "palette":
      return <PaletteView />;
    case "notice":
      return <NoticeView />;
    case "onboarding":
      return <OnboardingView />;
    default:
      return <LibraryView />;
  }
}
