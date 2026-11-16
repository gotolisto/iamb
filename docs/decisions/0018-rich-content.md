# ADR-0018: Rich snippets are Markdown plus attachments, rendered in the core

Status: proposed (awaiting owner approval)

**Context.** About 10% of snippets carry formatting, images or media, and must paste well into mail and document apps and fall back to plain text elsewhere.

**Decision.** A rich snippet's body is Markdown (CommonMark with tables and strikethrough) with template macros; images and media are attachments referenced as `attachment:<sha256>`. `iamb-core` renders HTML and a plain-text version; the injector pastes both flavours and then restores the clipboard. Macro output is inserted as literal text, escaped for Markdown.

**Consequences.** One readable, diff-friendly source format that syncs as text. Identical rendering on desktop and mobile. Formatting is limited to what Markdown expresses (no fonts or colours), which is accepted for version 1.

**Rejected.** Storing HTML (unsafe to render, hard to edit and merge). A custom rich format (needs converters for every target).
