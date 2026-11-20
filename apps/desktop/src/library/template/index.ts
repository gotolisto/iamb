// The template editor (task 3.4). `TemplateField` is the Content field of the snippet editor
// (`../Editor.tsx`): template text in and out.

export { TemplateField, type TemplateFieldProps } from "./TemplateField";
export { TemplatePreview, PREVIEW_DELAY_MS, type TemplatePreviewProps } from "./TemplatePreview";
export {
  templateService,
  createTemplateService,
  type TemplateService,
  type ParsedTemplate,
  type TemplatePreview as TemplatePreviewData,
  type TemplateError,
} from "./api";
export { errorMessage, messageText } from "./errors";
