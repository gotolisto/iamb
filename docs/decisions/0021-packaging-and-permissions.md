# ADR-0021: The macOS service is a signed helper app; permissions are checked in the worker

Status: accepted (approved by the owner on 2026-11-18)

**Context.** macOS grants Accessibility and Input Monitoring to the specific signed program that creates the event tap, and drops grants when that identity changes. Onboarding must show the true permission state.

**Decision.** On macOS the service ships as a signed helper app with a stable bundle identifier inside `Iamb.app` and is registered as a login item by the main app through the system login-item API. A stable development signing identity is used from the start of Phase 2. Permission status is always determined by the worker and reported over IPC (`GetPermissions`); the UI host never checks its own. On Windows the supervisor is registered under the per-user Run key and no permission prompts exist; windows running at a higher privilege level are detected and reported as not expandable.

**Consequences.** Grants survive updates and rebuilds. Onboarding cannot show "granted" while expansion is still blocked. Installing needs no administrator rights on either system.

**Rejected.** A bare executable as the service on macOS (grants attach unreliably and reset on each build). Checking permissions from the UI host (it is a different program with its own, irrelevant, trust state).

**Addendum (2026-11-22, the beta-gap review's finding 1).** The grants are the helper's only as long as the helper starts no program as its own child: the system judges such a child as the helper. So the supervisor has the system open `Iamb.app` (`open -g -W -a`) to start the UI host, as the UI host has the system open the helper, and a release build of the helper refuses `--ui-host` and runs for the default data root only (architecture §3.2, §8).
