# ADR-0018: Rich snippets are Markdown plus attachments, rendered in the core

Status: proposed (awaiting owner approval)

**Context.** About 10% of snippets carry formatting, images or media, and must paste well into mail and document apps and fall back to plain text elsewhere. Rich content from other people must not be able to inject active content.

**Decision.** A rich snippet's body is Markdown (CommonMark with tables and strikethrough, raw HTML disabled, links limited to `http`, `https` and `mailto`) with template macros; images and media are attachments referenced as `attachment:<sha256>`. `iamb-core` renders HTML and a plain-text version. The service resolves attachment references when it builds the clipboard content (images embedded as data URIs; a lone media file also offered as a file), pastes, and restores the clipboard. Macro output is inserted as literal text, escaped for Markdown. A missing attachment falls back to plain text.

**Consequences.** One readable, diff-friendly source format that syncs as text. Identical rendering on desktop and mobile. Formatting is limited to what Markdown expresses (no fonts or colours), which is accepted for version 1.

**Rejected.** Storing HTML (unsafe to render, hard to edit and merge). A custom rich format (needs converters for every target). Allowing raw HTML in Markdown.
