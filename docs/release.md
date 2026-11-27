# Releases: installers, signing and the install smoke test

Status: task 5.1 · Decision record: [ADR-0030](decisions/0030-installers.md) (proposed), building on
[ADR-0021](decisions/0021-packaging-and-permissions.md) · Workflows: `.github/workflows/release.yml`,
`.github/workflows/installers.yml` · Files: `apps/desktop/release/`

The pipeline builds three installers. Signing switches on by itself once the owner has added the
secrets below. Until then every build is unsigned, and each build's job summary and the draft
release notes say so.

| OS | Installer | Built on | Signing when the secrets exist |
| --- | --- | --- | --- |
| macOS 13 or later | `Iamb_<version>_universal.dmg`: one `Iamb.app` for Apple silicon and Intel | `macos-latest`, both architectures joined with `lipo` | Developer ID, hardened runtime, notarized; tickets stapled to the app and to the disk image |
| Windows 10/11 x64 | `Iamb_<version>_x64-setup.exe`: NSIS, per user, no administrator rights | `windows-latest` | Authenticode on the installer, `iamb-desktop.exe`, `iamb-service.exe` and the uninstaller, timestamped |
| Linux x64 (development) | `Iamb_<version>_amd64.deb` (package `iamb`) | `ubuntu-latest` | none |

## Cutting a release

1. Set the version in one place: `[workspace.package] version` in the root `Cargo.toml`
   (`MAJOR.MINOR.PATCH`, or with a pre-release such as `1.0.0-rc.1`). Every crate inherits it,
   and `apps/desktop/src-tauri/tauri.conf.json` has no `version`, so the bundler uses the same one.
   Run `cargo xtask release-version` to check this; it prints the version. Commit and merge.
2. Tag the merge commit and push the tag:
   `git tag v1.2.3 && git push origin v1.2.3`.
3. `Release` runs:
   - `version`: `cargo xtask release-version --tag v1.2.3` fails if the tag is not `v` followed
     by the Cargo version;
   - `installers / build (macos|windows|linux)`: builds and, where the secrets exist, signs
     each installer, and writes the signing state into the job summary;
   - `installers / install smoke test (…)`: installs each installer on a fresh runner (below);
   - `publish`: only for a tag, and only after every smoke test has passed. It creates a
     **draft** GitHub release with the three installers, `SHA256SUMS`, and the signing state of
     each file.
4. Read the draft. If a file is marked unsigned, the system will warn the people who open it
   (see "What users see" below). Publish the release by hand.

To try the pipeline without releasing: Actions, `Release`, "Run workflow" on any branch. It
builds and signs as for a tag and runs the smoke tests, and keeps the installers as workflow
artifacts. It publishes nothing. The `CI` workflow can also run them: "Run workflow" with
**installers** ticked. Those builds are never signed. Pull requests never run them, because
they take 30 to 60 minutes; the `installers` job is not part of `ci-ok`.

## Secrets the owner adds

Add these under Settings, Secrets and variables, Actions. The workflows read them only through
step environment variables, never on a command line they print. A job only checks whether each
one is present.

| Name | Kind | What it holds |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | secret | The Developer ID Application certificate and its private key, exported as `.p12`, base64-encoded |
| `APPLE_CERTIFICATE_PASSWORD` | secret | The `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | secret | The identity's name, `Developer ID Application: <name> (<team id>)` |
| `APPLE_ID` | secret | The Apple Account email used for notarization |
| `APPLE_PASSWORD` | secret | An app-specific password for that account (not the account password) |
| `APPLE_TEAM_ID` | secret | The ten-character team identifier |
| `WINDOWS_CERTIFICATE` | secret | Option A: the code-signing certificate and key as `.pfx`, base64-encoded |
| `WINDOWS_CERTIFICATE_PASSWORD` | secret | Option A: the `.pfx` password |
| `WINDOWS_SIGN_COMMAND` | secret | Option B: the command that signs one file, with `%1` where the file's path goes |
| `WINDOWS_SIGN_CREDENTIALS` | secret, optional | Option B: passed to that command as the environment variable `WINDOWS_SIGN_CREDENTIALS` |
| `WINDOWS_TIMESTAMP_URL` | variable, optional | Option A: the RFC 3161 timestamp server (default `http://timestamp.digicert.com`) |

The macOS names are the ones the bundler reads. You must add all six. If only some are set, the
macOS build fails and names the missing ones (names only). On Windows, set either option A or
option B, not both. If no secrets are set, every build is unsigned.

Pushing a `v*` tag starts a run that uses these secrets. Restrict who may create such tags:
Settings, Rules, Rulesets, a tag ruleset for `v*` that only the owner can bypass.

## What users see

### macOS

| Build | First open of the downloaded disk image and app |
| --- | --- |
| Unsigned (ad-hoc signature, not notarized) | "Apple could not verify 'Iamb' is free of malware…" with only *Done* and *Move to Bin*. The user has to open System Settings, Privacy & Security, click *Open Anyway* and confirm with their password; Control-click *Open* no longer gets around this on macOS 15. Accessibility and Input Monitoring grants belong to an ad-hoc identity that changes with every build, so they have to be given again after each update |
| Signed and notarized | The usual first-launch question for any app downloaded from the internet ("'Iamb' is an app downloaded from the Internet. Are you sure you want to open it?"), naming the developer. There is no malware warning. Then onboarding asks for Accessibility and Input Monitoring for "Iamb Service". These grants survive updates because the signing identity stays the same (ADR-0021) |

Installing means dragging `Iamb.app` to Applications. Uninstalling means moving it to the Bin.
macOS then removes the login item. The data in `~/Library/Application Support/Iamb/` stays.

### Windows

| Build | Running the installer |
| --- | --- |
| Unsigned | SmartScreen: "Windows protected your PC… Publisher: Unknown publisher", with *More info*, then *Run anyway*. Browsers may also flag the download as uncommon |
| Signed | The publisher's name is shown. SmartScreen may still warn until the publisher's certificate has built up download reputation. This applies to new certificates, EV ones included. Once it has reputation, there is no warning |

Neither build shows an administrator (UAC) prompt, because the installer is per user. It
installs to `%LOCALAPPDATA%\Iamb` and adds a Start menu entry and an entry in Settings, Apps.
Uninstalling from there removes the files and the login item. The data in `%APPDATA%\Iamb\`
stays unless the user ticks "Delete the application data".

The KPI of task 5.1 ("0 security warnings") can be met on macOS once the build is signed and
notarized. On Windows a signature is necessary, but SmartScreen reputation decides the rest.

## Owner: Apple Developer ID and notarization

1. Join the Apple Developer Program (as an organization if the publisher should be a company
   name). The team ID is under Membership details.
2. Create a **Developer ID Application** certificate: in Xcode, Settings, Accounts, Manage
   Certificates, "+", Developer ID Application. Or create it at developer.apple.com,
   Certificates, from a certificate signing request made in Keychain Access.
3. In Keychain Access, find the certificate under My Certificates, expand it so its private key
   shows, select both, and export them as `.p12` with a strong password.
4. `base64 -i DeveloperID.p12 | pbcopy`, then paste into the secret `APPLE_CERTIFICATE`. Put the
   password in `APPLE_CERTIFICATE_PASSWORD`. Delete the `.p12` file.
5. `security find-identity -v -p codesigning` prints the identity's full name. Put that name in
   `APPLE_SIGNING_IDENTITY`.
6. At account.apple.com, Sign-In and Security, App-Specific Passwords, create one named
   "Iamb notarization" and put it in `APPLE_PASSWORD`. Put the account's email in `APPLE_ID` and
   the team ID in `APPLE_TEAM_ID`.
7. Run `Release` by hand on `main`. The macOS job summary should say "Signed … notarized", and the
   macOS smoke test checks the result (`codesign --verify --deep --strict`, `spctl -a`, and
   `stapler validate` on the app and the disk image, plus the same team and the hardened runtime
   on app and helper).

The entitlements files (`apps/desktop/release/macos/*.entitlements`) are empty on purpose. Each
file says why. Add a key only with its reason next to it.

## Owner: Windows code signing

Pick one option.

- **Option A, a certificate file.** For a code-signing certificate you can export with its
  private key (`.pfx`). Since June 2023, newly issued OV and EV certificates must keep their keys
  on a hardware token or a cloud HSM, so they cannot be exported. This option is mainly for an
  older certificate. Run `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))` in
  PowerShell, put the result in `WINDOWS_CERTIFICATE` and the password in
  `WINDOWS_CERTIFICATE_PASSWORD`. The pipeline imports the certificate into the runner's user
  store, the bundler signs by thumbprint with `signtool` (SHA-256, timestamped through
  `WINDOWS_TIMESTAMP_URL`), and the certificate is removed when the job ends.
- **Option B, a sign command.** For a cloud signing service or an HSM-backed key. You need a
  command-line client that signs one file in place and timestamps it. Put the command line in
  `WINDOWS_SIGN_COMMAND`, with `%1` for the file, for example `signtool-client sign --profile
  iamb --timestamp %1`. The bundler (its `signCommand`) runs it for every binary and for the
  installer. The client's credentials go in `WINDOWS_SIGN_CREDENTIALS`. If the client reads other
  environment variable names, or has to be installed first, add the environment variables or an
  install step to "Build the installer (Windows)" in `installers.yml`. The bundler splits the
  command at spaces; for a path with spaces, give the client a wrapper script.

Check the result: the Windows job summary says "Signed", and the smoke test requires a
`Valid`, timestamped Authenticode signature on the installer, both executables and the
uninstaller.

## Install smoke test

This is the "install on clean machines" test of task 5.1, as far as CI can take it. Each OS
runs on a fresh hosted runner that did not build the installer. Only the smoke scripts
(`apps/desktop/release/smoke/`) are checked out there.

| Step | macOS (`macos.sh`) | Windows (`windows.ps1`) | Linux (`linux.sh`) |
| --- | --- | --- | --- |
| Clean before | `/Applications/Iamb.app` absent | `%LOCALAPPDATA%\Iamb` absent | package `iamb` not installed |
| Install | attach the image, copy `Iamb.app` to `/Applications`, detach | `setup.exe /S` | `apt-get install ./…deb` (dpkg, plus declared dependencies) |
| Files | UI host and helper app present; identifiers `org.iamb.Iamb` and `org.iamb.Iamb.Service`; both universal | `iamb-desktop.exe`, `iamb-service.exe`, `uninstall.exe`; uninstall entry under HKCU, none under HKLM | `/usr/bin/iamb-desktop`, `/usr/bin/iamb-service` |
| Version = Cargo version | both `Info.plist` files, both `--version` | uninstall entry, both `--version` | package version, both `--version` |
| Start | installed UI host, `--measure-open 1` in a scratch data root | same | same, under Xvfb |
| UI host reaches the service | fails at the start if the default data root exists (the helper serves only that one, and the test removes only what it made); the helper's `iamb-service --supervise --no-ui-host` for it, then `iamb-desktop --check-service` must say `service ok` with no rejection in `worker.log`; a copy of `iamb-desktop` outside `Iamb.app`, run with `--skip-server-check` so that only the service can refuse it, must fail and `worker.log` must gain `rejected a client: the peer is not the installed Iamb binary`; signed runs only: an ad hoc re-signed `iamb-desktop` in place of the real one must be refused the same way | `iamb-service.exe --supervise --no-ui-host --data-dir <scratch>`, then `iamb-desktop.exe --check-service` must say `service ok` with no rejection logged; a copy outside the installation directory, with `--skip-server-check`, must fail and be logged as rejected. An unsigned UI host in place of the real one is not exercised (no unsigned build of a signed release on the runner) | `iamb-service --supervise --no-ui-host`, then `iamb-desktop --check-service` must say `service ok` (no copy: Linux has no installed-binary check) |
| Signatures, when signed | `codesign --verify --deep --strict`, `spctl -a` on app and image, `stapler validate` on both, same team, hardened runtime | `Get-AuthenticodeSignature` `Valid` and timestamped on four files | none (reported unsigned) |
| Signatures, when unsigned | `codesign --verify --deep --strict` still runs on the ad-hoc seal; the rest is reported as skipped | states reported, not failed | none |
| Uninstall | remove `Iamb.app` | `uninstall.exe /S`, then wait until files, uninstall entry, Start menu entry and Run value are gone | `apt-get purge`, binaries gone |

Limits:

- A hosted runner is not a user's clean machine. It already has the WebView2 runtime and
  developer tools, and it has no downloaded-file quarantine flag, so Gatekeeper's first-open
  dialog and SmartScreen are not exercised. The signature checks stand in for them. Before a
  1.0 release, also install each signed build once by hand, downloaded through a browser on a
  fresh Mac and a fresh Windows machine.
- The installed service is started by the script, not by the login item, and the UI host's
  connection is checked headless (`--check-service`). The smoke test does not cover granting
  permissions, the login item, or expansion through the installed service. Those need a person
  at the machine.

## Installed layout

| OS | UI host | Service | How the UI host finds the service |
| --- | --- | --- | --- |
| macOS | `/Applications/Iamb.app/Contents/MacOS/iamb-desktop` | `…/Iamb.app/Contents/Library/LoginItems/Iamb Service.app/Contents/MacOS/iamb-service` | Registers the login item by bundle identifier `org.iamb.Iamb.Service` (`SMAppService`). The system starts the helper without arguments, and `iamb-service` from this layout then defaults to `--supervise` (`iamb_service::paths::is_login_item_helper`) |
| Windows | `%LOCALAPPDATA%\Iamb\iamb-desktop.exe` | `%LOCALAPPDATA%\Iamb\iamb-service.exe` | Next to its own executable. The Run value `Iamb` holds `"…\iamb-service.exe" --supervise` |
| Linux | `/usr/bin/iamb-desktop` | `/usr/bin/iamb-service` | (development only) |

The service binary is always built with `cargo build -p iamb-service --release --locked` on its
own, as ADR-0008 requires.

The Windows installer hooks (`apps/desktop/release/windows/hooks.nsh`) stop this user's running
`iamb-service.exe` before replacing or removing files, and start it again after an install if
"start at login" is on. On uninstall they move the Run value to `HKCU\Software\Iamb Setup`, so
that an upgrade which uninstalls first can put it back. After a real uninstall, that one
registry value is all that stays behind.

## Known gaps (not done by task 5.1)

- **Signed installed builds are checked by the smoke test only once signing is set up.** Release
  builds accept as clients only the executables of their own installation (architecture §6.1,
  "The installed-binary check"; finding 11: closed for unsigned builds, written but not yet
  exercised for signed ones). An unsigned or ad-hoc installation accepts its members by path; a
  signed one also requires the same signer (macOS: the team identifier and both bundle
  identifiers; Windows: the same certificate thumbprint). A signed build has its signer compiled
  in (`IAMB_SIGNER_TEAM` from `APPLE_TEAM_ID`; `IAMB_SIGNER_THUMBPRINT` from the imported
  certificate). With `WINDOWS_SIGN_COMMAND` the thumbprint is not known when the build runs, so
  such a build reads its own signature at its start instead. The smoke test connects the
  installed UI host to the installed service on every run and requires the service itself to
  refuse a copy, so the unsigned path is checked on every build; the signed path only on a
  signed run. Unsigned installers are for testing, not for users.
- The supervisor starts the UI host (architecture §3.2), but not the sync agent yet (hook point
  in `crates/iamb-service/src/supervisor.rs`). On Windows, turning on "start at login" writes the
  Run value but does not start the service until the next login (or the next install).
- No updater artifacts. Update signing and channels are task 5.2 (ADR-0023).
- No AppImage and no signed Linux packages.
