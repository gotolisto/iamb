# Install smoke test, Windows (docs/release.md, "Install smoke test"): on a clean runner, run the
# per-user installer silently, check what was installed, start it, check Authenticode signatures
# when the build is signed, uninstall silently, check it is gone.
#
#   windows.ps1 -Installer <path to setup.exe> -Version <expected version> -Signed <true|false>
param(
  [Parameter(Mandatory = $true)] [string] $Installer,
  [Parameter(Mandatory = $true)] [string] $Version,
  [Parameter(Mandatory = $true)] [string] $Signed
)
$ErrorActionPreference = 'Stop'
$summary = if ($env:GITHUB_STEP_SUMMARY) { $env:GITHUB_STEP_SUMMARY } else { $null }
function Note([string] $line) {
  Write-Host $line
  if ($summary) { Add-Content -Path $summary -Value "- $line" }
}
function Fail([string] $why) { Note "FAIL: $why"; exit 1 }
function Ok([string] $what) { Note "ok: $what" }

# Runs an installed executable with redirected output (the UI host is a GUI program, so its
# output reaches a file, not the console) and returns its exit code and output.
function Invoke-Exe([string] $exe, [string[]] $arguments, [int] $seconds) {
  $out = New-TemporaryFile
  $err = New-TemporaryFile
  $p = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -NoNewWindow `
    -RedirectStandardOutput $out -RedirectStandardError $err
  $null = $p.Handle  # keeps the handle, so ExitCode is available after the wait
  if (-not $p.WaitForExit($seconds * 1000)) {
    $p.Kill()
    Fail "$exe $arguments did not exit within $seconds s"
  }
  $p.WaitForExit()
  [pscustomobject]@{
    Code = $p.ExitCode
    Out  = (Get-Content -Raw $out).Trim()
    Err  = (Get-Content -Raw $err)
  }
}

$installDir = Join-Path $env:LOCALAPPDATA 'Iamb'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Iamb'
$desktop = Join-Path $installDir 'iamb-desktop.exe'
$service = Join-Path $installDir 'iamb-service.exe'
$uninstaller = Join-Path $installDir 'uninstall.exe'
if ($summary) { Add-Content -Path $summary -Value '### Install smoke test: Windows' }

if (Test-Path $installDir) { Fail "$installDir exists before the install: not a clean machine" }

# Install, silently, as the current user (no elevation is requested by a per-user installer).
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -PassThru
$null = $p.Handle
$p.WaitForExit()
if ($p.ExitCode -ne 0) { Fail "the installer exited with $($p.ExitCode)" }
Ok "installed $(Split-Path -Leaf $Installer) silently into $installDir"

# What was installed.
foreach ($f in @($desktop, $service, $uninstaller)) {
  if (-not (Test-Path $f)) { Fail "$f is missing" }
}
if (Test-Path 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Iamb') {
  Fail 'the installer registered itself machine-wide; it must be per user'
}
$entry = Get-ItemProperty -Path $uninstallKey -ErrorAction SilentlyContinue
if (-not $entry) { Fail "no uninstall entry under $uninstallKey" }
if ($entry.DisplayVersion -ne $Version) { Fail "the uninstall entry says version $($entry.DisplayVersion)" }
$fileVersion = (Get-Item $desktop).VersionInfo.ProductVersion
Ok "UI host, service and uninstaller present; per-user uninstall entry at version $Version (file version $fileVersion)"

$r = Invoke-Exe $desktop @('--version') 30
if ($r.Code -ne 0 -or $r.Out -ne "iamb-desktop $Version") { Fail "iamb-desktop --version: code $($r.Code), said '$($r.Out)'" }
$r = Invoke-Exe $service @('--version') 30
if ($r.Code -ne 0 -or $r.Out -ne "iamb-service $Version") { Fail "iamb-service --version: code $($r.Code), said '$($r.Out)'" }
Ok "both executables run and report $Version"

# Start the installed UI host: one cold open of the library window in a scratch data root.
$data = Join-Path $env:RUNNER_TEMP 'iamb-smoke-data'
$r = Invoke-Exe $desktop @('--data-dir', "`"$data`"", '--measure-open', '1') 120
if ($r.Code -ne 0) { Write-Host $r.Out; Write-Host $r.Err; Fail "the installed UI host exited with $($r.Code)" }
Write-Host $r.Out
Ok 'the installed UI host opened its library window (--measure-open 1)'

# The installed UI host reaches the installed service (architecture §6.1, the installed-binary
# check; finding 11 and its strict review, finding 2): start the installed supervisor as the Run
# value does (without the UI host), then connect as the UI host, headless (--check-service). A
# copy of the UI host outside the installation directory connects without checking the service
# (--skip-server-check), so that the refusal can only be the service's, and the worker's log
# must record it. The refusal of an unsigned UI host in place of the real one by a signed
# service is not exercised here: the runner has no unsigned build of a signed release.
$serviceData = Join-Path $env:RUNNER_TEMP 'iamb-smoke-service'
$svc = Start-Process -FilePath $service -ArgumentList @('--supervise', '--no-ui-host', '--data-dir', "`"$serviceData`"") -PassThru
$null = $svc.Handle
function Stop-Service-Processes {
  Get-Process -Name 'iamb-service' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
$workerLog = Join-Path $serviceData 'logs\worker.log'
$rejected = 'rejected a client: the peer is not the installed Iamb binary'
function Get-Rejections {
  if (-not (Test-Path $workerLog)) { return 0 }
  @(Select-String -Path $workerLog -SimpleMatch -Pattern $rejected).Count
}
function Show-Service-Logs {
  Get-ChildItem -Path (Join-Path $serviceData 'logs') -Filter '*.log' -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Host "--- $($_.Name)"; Get-Content -Tail 40 $_.FullName }
}
$r = Invoke-Exe $desktop @('--check-service', '--data-dir', "`"$serviceData`"") 120
if ($r.Code -ne 0 -or -not $r.Out.StartsWith("service ok: iamb-service $Version")) {
  Write-Host $r.Out; Write-Host $r.Err; Show-Service-Logs; Stop-Service-Processes
  Fail "the installed service refused the installed UI host, or never answered (--check-service): code $($r.Code)"
}
if ((Get-Rejections) -ne 0) { Show-Service-Logs; Stop-Service-Processes; Fail 'the worker logged a rejection for the installed UI host' }
Ok "the installed UI host connected to the installed service as the UI host ($($r.Out))"
$copyDir = Join-Path $env:RUNNER_TEMP 'iamb-copy'
New-Item -ItemType Directory -Force -Path $copyDir | Out-Null
Copy-Item -Path $desktop -Destination $copyDir
$before = Get-Rejections
$r = Invoke-Exe (Join-Path $copyDir 'iamb-desktop.exe') @('--check-service', '--skip-server-check', '--data-dir', "`"$serviceData`"") 120
if ($r.Code -eq 0) { Show-Service-Logs; Stop-Service-Processes; Fail 'a copy of the UI host outside the installation directory was accepted' }
$deadline = (Get-Date).AddSeconds(10)
while ((Get-Rejections) -le $before) {
  if ((Get-Date) -gt $deadline) {
    Write-Host $r.Out; Write-Host $r.Err; Show-Service-Logs; Stop-Service-Processes
    Fail "the service did not log the copy's rejection ('$rejected'): its own check was not what refused it"
  }
  Start-Sleep -Milliseconds 200
}
Ok 'the installed service refused a copy of the UI host outside the installation directory, and logged it'
Stop-Service-Processes
Remove-Item -Recurse -Force $copyDir

# Signatures.
$files = @($Installer, $desktop, $service, $uninstaller)
if ($Signed -eq 'true') {
  foreach ($f in $files) {
    $s = Get-AuthenticodeSignature -FilePath $f
    if ($s.Status -ne 'Valid') { Fail "$(Split-Path -Leaf $f): signature $($s.Status) ($($s.StatusMessage))" }
    if (-not $s.TimeStamperCertificate) { Fail "$(Split-Path -Leaf $f): signature has no timestamp" }
  }
  Ok 'signed: installer, UI host, service and uninstaller carry valid, timestamped Authenticode signatures'
} else {
  $states = ($files | ForEach-Object { "$(Split-Path -Leaf $_)=$((Get-AuthenticodeSignature -FilePath $_).Status)" }) -join ', '
  Ok "unsigned build: $states"
}

# Uninstall, silently. The NSIS uninstaller copies itself away and returns at once, so wait for
# the files and the entry to go.
$p = Start-Process -FilePath $uninstaller -ArgumentList '/S' -PassThru
$null = $p.Handle
$p.WaitForExit()
if ($p.ExitCode -ne 0) { Fail "the uninstaller exited with $($p.ExitCode)" }
$deadline = (Get-Date).AddSeconds(120)
while ((Test-Path $desktop) -or (Test-Path $service) -or (Test-Path $uninstallKey)) {
  if ((Get-Date) -gt $deadline) { Fail 'files or the uninstall entry are still there 120 s after the uninstall' }
  Start-Sleep -Seconds 2
}
$run = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'Iamb' -ErrorAction SilentlyContinue
if ($run) { Fail 'the login item (Run value Iamb) is still there' }
$shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Iamb.lnk'
if (Test-Path $shortcut) { Fail 'the Start menu shortcut is still there' }
Ok 'uninstalled: program files, uninstall entry, Start menu shortcut and login item gone'
exit 0
