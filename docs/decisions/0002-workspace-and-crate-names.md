# ADR-0002: One Cargo workspace; crates named `iamb-*`

Status: accepted (approved by the owner on 2026-11-18)

**Context.** The name `iamb` is already taken on the public crate registry by an unrelated project.

**Decision.** The product and repository are called Iamb. Every crate is prefixed (`iamb-core`, `iamb-store`, …); no crate is named plain `iamb`. Crates are marked `publish = false` until there is a reason to publish. The command-line binary is called `iamb`. All crates live in one workspace under `crates/`; the Tauri app lives in `apps/desktop`; server migrations live in `server/`.

**Consequences.** No registry collision. One lock file and one CI pipeline. If the binary name ever clashes on a user's machine, it can be renamed without touching crate names.

**Rejected.** Renaming the product (the owner chose the name). Separate repositories per component (slower cross-cutting changes, no benefit at this size).
