#Requires -Version 7
# The update rehearsal (W9 plan D11, §W9-G, §7 A): a real 0.4.x app, built from its tag with a throwaway signing key and no built-in
# update address, installed with its own setup program, updates itself to the Rust build served from a local HTTPS server, and what
# it leaves is checked. On a throwaway machine only: it installs into %LOCALAPPDATA%\Programs\MightyCode, trusts a throwaway
# certificate and writes the user's uninstall entry. Nothing is published and no real key or address is used.
#
#   $env:MIGHTY_REHEARSAL_DISPOSABLE = '1'
#   .\scripts\update-rehearsal.ps1 -From v0.4.59 -RustZip <MightyCode-windows-x64.zip> [-Work <folder>]
#   .\scripts\update-rehearsal.ps1 -From v0.4.59 -RustZip <zip> -Interactive     (§7 A: stops before the update; the default profile)
#
# The automatic run (CI's rehearsal job):
#   1. C# from the tag (a detached worktree), built by that tag's scripts/build-windows.ps1 -Installer with MIGHTY_UPDATE_PUBLIC_KEY =
#      the throwaway key's public half and no MIGHTY_UPDATE_URL, so the typed address of the profile is honoured (Windows rule 3);
#   2. its setup program installs it silently; the uninstall entry says the tag's version;
#   3. a throwaway certificate for 127.0.0.1 is trusted (LocalMachine\Root when elevated, else CurrentUser\Root), the server starts, and
#      latest.json is signed with the throwaway key by make-update-manifest.py (the version is the zip's build-info.json version, which
#      must be newer than the tag) and read back through C#'s own AppUpdateManifest.Parse (tools/parity `update manifest`);
#   4. the installed app starts on a new profile whose workspace-state.json turns the automatic update on with the server's address and
#      holds one workspace; it checks at start, downloads and stages <profile>\updates\<version>\staged;
#   5. its window is closed (WM_CLOSE: the app's quit path, the install on quit, --relaunch 0); the Rust exe it staged runs as the helper;
#   6. checked: install.log's line (replaced=True … started=False backupRemoved=True), the install holds the zip's exe and its
#      build-info.json (engine tauri), DisplayVersion is the Rust version's major.minor.patch, no MightyCode.backup-* or .update-new
#      beside it, and the server was asked for latest.json and the zip;
#   7. the installed Rust app passes the smoke (native/tauri/scripts/smoke.ps1 -NoRuns, its own seeded profile);
#   8. it starts on the rehearsal profile with --read-only: its host.log loads the state with the one workspace, writes off, and the
#      profile's state file is left as it was;
#   9. the relaunch path (--relaunch 1): the tag's setup program installs 0.4.x again, C#'s AppUpdateInstallPlan.Create (tools/parity
#      `update plan-args`; the 0.4.59 and 0.4.61 hand-offs are the same code) gives the helper's arguments for the staged update, and the
#      staged Rust exe is started with them as 0.4.x starts it (only SystemRoot, no window, %TEMP%) once a stand-in app process has
#      exited: exit 0, install.log's relaunch=True started=True line, the Rust app started from the install with the user's default
#      profile (its host.log), then closed. Before that, a default profile that does not exist yet gets a state file with the automatic
#      update off, so the release build started there asks no update server;
#  10. the certificate, the server, the install and the worktree are removed (the machine is thrown away anyway).
# Every check is printed; the result is <Work>\rehearsal-result.json. Exit 0 when all hold.
# Needs git (the tags), PowerShell 7, the .NET 10 SDK, Node 22, Python 3 with `cryptography`, Inno Setup 6 (ISCC.exe or MIGHTY_ISCC)
# and the Rust toolchain (the smoke seeds its profile with cargo).
param(
    [Parameter(Mandatory = $true)][ValidatePattern('^v0\.4\.\d+$')][string]$From,
    [Parameter(Mandatory = $true)][string]$RustZip,
    [string]$Work = (Join-Path ([IO.Path]::GetTempPath()) ('mighty-rehearsal-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))),
    [switch]$Interactive
)
$ErrorActionPreference = 'Stop'
if ($env:MIGHTY_REHEARSAL_DISPOSABLE -ne '1') {
    throw 'The rehearsal installs Mighty Code for this user, trusts a throwaway certificate and writes the uninstall entry: run it on a throwaway machine with MIGHTY_REHEARSAL_DISPOSABLE=1 (CI''s rehearsal job, the §7 VM).'
}
if (-not $IsWindows) { throw 'The rehearsal runs on Windows.' }
$repo = Split-Path -Parent $PSScriptRoot
$python = (Get-Command python -ErrorAction Stop).Source
$server = Join-Path $PSScriptRoot 'update-rehearsal-server.py'
$architecture = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$installDirectory = Join-Path $env:LOCALAPPDATA 'Programs\MightyCode'
$entry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{F2F99AEF-FB35-4476-A3EE-49453FA0527F}_is1'
$fromVersion = $From.Substring(1)
if ((Test-Path $entry) -or (Test-Path $installDirectory)) { throw "Mighty Code is already installed for this user ($installDirectory); the rehearsal starts from a machine without it." }
$RustZip = (Resolve-Path $RustZip).Path
$Work = [IO.Path]::GetFullPath($Work)
if ((Test-Path $Work) -and (Get-ChildItem -Force $Work | Select-Object -First 1)) { throw "give a new or empty folder for -Work: $Work" }
New-Item -ItemType Directory -Force $Work | Out-Null
$iscc = @($env:MIGHTY_ISCC, (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'), (Join-Path $env:ProgramFiles 'Inno Setup 6/ISCC.exe'),
    (Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 6/ISCC.exe')) | Where-Object { $_ -and (Test-Path $_ -PathType Leaf) } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 (ISCC.exe) was not found; install it or name it in MIGHTY_ISCC' }

$checks = [ordered]@{}
$failures = [Collections.Generic.List[string]]::new()
function Check([string]$what, [bool]$condition, [string]$detail = '') {
    $checks[$what] = $condition
    if ($condition) { Write-Host "  ok   $what" } else { Write-Host "  FAIL $what $detail"; $failures.Add($what) }
}
function Step([string]$text) { Write-Host ''; Write-Host "== $text" }
function Invoke-Checked([string]$what, [scriptblock]$command) {
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}
function Wait-Until([string]$what, [int]$seconds, [scriptblock]$condition) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while (-not (& $condition)) {
        if ((Get-Date) -gt $deadline) { throw "timed out after $seconds s waiting for $what" }
        Start-Sleep -Milliseconds 500
    }
}
function Get-Sha256([string]$path) { (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Write-Json([string]$path, $value) { [IO.File]::WriteAllText($path, ($value | ConvertTo-Json -Depth 10) + "`n", [Text.UTF8Encoding]::new($false)) }

# WM_CLOSE to every visible top-level window of a process: what the window's close button does (C#: AppWindow.Closing → QuitAsync;
# the Rust app: its close request → the quit path).
Add-Type -Namespace Rehearsal -Name Windows -MemberDefinition @'
public delegate bool EnumWindowsProc(System.IntPtr hwnd, System.IntPtr lParam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, System.IntPtr lParam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hwnd, out uint processId);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hwnd);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool PostMessageW(System.IntPtr hwnd, uint message, System.IntPtr w, System.IntPtr l);
public static int CloseWindowsOf(uint pid) {
    int closed = 0;
    EnumWindows((hwnd, _) => {
        uint owner;
        GetWindowThreadProcessId(hwnd, out owner);
        if (owner == pid && IsWindowVisible(hwnd) && PostMessageW(hwnd, 0x0010, System.IntPtr.Zero, System.IntPtr.Zero)) closed++;
        return true;
    }, System.IntPtr.Zero);
    return closed;
}
'@
function Close-App([Diagnostics.Process]$process, [int]$seconds) {
    if ($process.HasExited) { return $true }
    $closed = [Rehearsal.Windows]::CloseWindowsOf([uint32]$process.Id)
    Write-Host "  WM_CLOSE to $closed window(s) of $($process.Id)"
    if ($process.WaitForExit($seconds * 1000)) { return $true }
    Write-Host "  $($process.Id) did not end within $seconds s; stopped"
    $process.Kill($true)
    $false
}
function Get-AppsUnder([string]$folder) {
    $prefix = $folder.TrimEnd('\') + '\'
    @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) })
}
function Get-LastLine([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return '' }
    "$(Get-Content -LiteralPath $path -Encoding utf8 | Where-Object { $_ } | Select-Object -Last 1)"
}
# The text a running app's log holds from byte `$offset` on: opened sharing read, write and delete, as the app keeps it open for writing
# (File.ReadAllText shares read only, which a file open for writing refuses with a sharing violation).
function Read-Shared([string]$path, [long]$offset = 0) {
    $stream = [IO.FileStream]::new($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
    try {
        [void]$stream.Seek([Math]::Min($offset, $stream.Length), [IO.SeekOrigin]::Begin)
        $reader = [IO.StreamReader]::new($stream, [Text.UTF8Encoding]::new($false))
        $reader.ReadToEnd()
    } finally { $stream.Dispose() }
}
function Get-Beside { @(Get-ChildItem -LiteralPath (Split-Path $installDirectory) -Directory -Force | Where-Object { $_.Name -like 'MightyCode.backup-*' -or $_.Name -eq 'MightyCode.update-new' } | ForEach-Object Name) }
function Install-Setup([string]$setup, [string]$log) {
    $process = Start-Process -FilePath $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"" -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "setup exited with $($process.ExitCode) (log: $log)" }
}
# The id of a process that has already exited: the app that started the helper is gone.
function Get-ExitedPid {
    $process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\cmd.exe') -ArgumentList '/d', '/c', 'exit' -WindowStyle Hidden -PassThru
    $process.WaitForExit()
    $process.Id
}

$worktree = Join-Path $Work "csharp-$From"
$certThumbprint = $null; $certStore = $null; $serverProcess = $null; $parity = $null
try {
    Step "the Rust package $RustZip"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $rust = Join-Path $Work 'rust'
    [IO.Compression.ZipFile]::ExtractToDirectory($RustZip, $rust)
    $info = Get-Content -LiteralPath (Join-Path $rust 'build-info.json') -Raw | ConvertFrom-Json
    $rustVersion = [string]$info.version
    $displayVersion = ($rustVersion -split '-')[0]
    $rustExe = Get-Sha256 (Join-Path $rust 'MightyCode.exe')
    Write-Host "  version $rustVersion ($($info.architecture), engine $($info.engine)), exe $rustExe"
    if ($info.engine -ne 'tauri' -or $info.architecture -ne $architecture) { throw "the package is not the Rust app for ${architecture}: $($info | ConvertTo-Json -Compress)" }
    if ([version]$displayVersion -le [version]$fromVersion) { throw "the package's version $rustVersion is not newer than $fromVersion" }
    $parity = Join-Path $Work 'parity'
    Invoke-Checked 'the C# parity tool build' { dotnet build (Join-Path $repo 'native\tauri\tools\parity\Parity.csproj') -c Release -o $parity --nologo -v quiet }

    Step "C# $From with a throwaway key and no built-in address"
    Invoke-Checked 'git worktree add' { git -C $repo worktree add --detach $worktree $From }
    $keyFile = Join-Path $Work 'keys\update-signing.key'
    $publicKey = (& $python $server keygen $keyFile | Select-Object -Last 1).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $publicKey) { throw 'keygen failed' }
    $savedUrl = $env:MIGHTY_UPDATE_URL; $savedKey = $env:MIGHTY_UPDATE_PUBLIC_KEY; $savedIscc = $env:MIGHTY_ISCC
    try {
        Remove-Item Env:\MIGHTY_UPDATE_URL -ErrorAction SilentlyContinue
        $env:MIGHTY_UPDATE_PUBLIC_KEY = $publicKey
        $env:MIGHTY_ISCC = $iscc
        Invoke-Checked "npm ci ($From page)" { npm ci --no-audit --no-fund --prefix (Join-Path $worktree 'native\windows\web') }
        Invoke-Checked "build-windows.ps1 ($From)" { pwsh -NoProfile -File (Join-Path $worktree 'scripts\build-windows.ps1') -Architecture $architecture -Configuration Release -Installer }
    } finally {
        $env:MIGHTY_UPDATE_URL = $savedUrl; $env:MIGHTY_UPDATE_PUBLIC_KEY = $savedKey; $env:MIGHTY_ISCC = $savedIscc
    }
    $csharpSetup = Join-Path $worktree "release\MightyCode-windows-$architecture-setup.exe"
    $csharpInfo = Get-Content -LiteralPath (Join-Path $worktree "release\native-windows-$architecture\build-info.json") -Raw | ConvertFrom-Json
    Check "the $From build checks for updates with no built-in address" ($csharpInfo.updateChecks -eq $true -and -not $csharpInfo.updateManifestUrl) ($csharpInfo | ConvertTo-Json -Compress)

    Step "install $From with its setup program"
    Install-Setup $csharpSetup (Join-Path $Work 'setup-1.log')
    $csharpExe = Get-Sha256 (Join-Path $installDirectory 'MightyCode.exe')
    Check "the uninstall entry says $fromVersion" ((Get-ItemProperty $entry).DisplayVersion -eq $fromVersion) "$((Get-ItemProperty $entry).DisplayVersion)"

    Step 'the throwaway certificate and the update server'
    $certs = Join-Path $Work 'cert'
    $certThumbprint = (& $python $server cert $certs | Select-Object -Last 1).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'cert failed' }
    $elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    # LocalMachine\Root asks nothing of an elevated shell; CurrentUser\Root has Windows ask the user once (the VM checklist).
    $certStore = if ($elevated) { 'Cert:\LocalMachine\Root' } else { 'Cert:\CurrentUser\Root' }
    Import-Certificate -FilePath (Join-Path $certs 'cert.cer') -CertStoreLocation $certStore | Out-Null
    $served = Join-Path $Work 'served'
    New-Item -ItemType Directory $served | Out-Null
    $package = Join-Path $served "MightyCode-windows-$architecture.zip"
    Copy-Item -LiteralPath $RustZip -Destination $package
    $portFile = Join-Path $Work 'server.port'
    $serverLog = Join-Path $Work 'server.log'
    $serverProcess = Start-Process -FilePath $python -ArgumentList @($server, 'serve', $served, '--cert', (Join-Path $certs 'cert.pem'), '--key', (Join-Path $certs 'key.pem'), '--port-file', $portFile, '--log', $serverLog) `
        -RedirectStandardOutput (Join-Path $Work 'server.out') -RedirectStandardError (Join-Path $Work 'server.err') -WindowStyle Hidden -PassThru
    Wait-Until 'the server' 30 { (Test-Path $portFile) -or $serverProcess.HasExited }
    if ($serverProcess.HasExited) { throw "the server ended: $(Get-Content (Join-Path $Work 'server.err') -Raw)" }
    $base = "https://127.0.0.1:$((Get-Content $portFile -Raw).Trim())"
    $manifest = Join-Path $served 'latest.json'
    $savedSigner = $env:MIGHTY_SIGNER
    try {
        $env:MIGHTY_SIGNER = 'cryptography'
        Invoke-Checked 'make-update-manifest.py' { & $python (Join-Path $PSScriptRoot 'make-update-manifest.py') "--windows-$architecture" $package --base-url $base --out $manifest --sign-key $keyFile --version $rustVersion }
    } finally { $env:MIGHTY_SIGNER = $savedSigner }
    # C#'s own verdict on the envelope, with the key the build carries.
    $cases = Join-Path $Work 'manifest-cases.json'
    Write-Json $cases @{ fixture = $publicKey; other = $publicKey; cases = @(@{ name = 'served'; file = $manifest }) }
    Invoke-Checked 'update manifest' { dotnet (Join-Path $parity 'parity.dll') update manifest $cases $cases }
    $verdict = (Get-Content -LiteralPath $cases -Raw | ConvertFrom-Json).csharp[0]
    Check "C#'s AppUpdateManifest.Parse reads the served manifest as $rustVersion with a $architecture package" ($verdict.verdict -eq 'ok' -and $verdict.version -eq $rustVersion -and $verdict.windows.$architecture.url -eq "$base/MightyCode-windows-$architecture.zip") ($verdict | ConvertTo-Json -Compress -Depth 5)
    $address = "$base/latest.json"
    $probe = Invoke-WebRequest -Uri $address -UseBasicParsing
    Check 'the server answers over the trusted certificate' ($probe.StatusCode -eq 200)

    if ($Interactive) {
        # §7 A: the person makes real data on the default profile and updates from the app; the server stays up until Enter.
        $default = Join-Path $env:APPDATA 'MightyCode'
        $stateFile = Join-Path $default 'workspace-state.json'
        New-Item -ItemType Directory -Force $default | Out-Null
        $state = if (Test-Path $stateFile) { Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json } else { [pscustomobject]@{ version = 1; workspaces = @(); sessions = @() } }
        $state | Add-Member -Force -NotePropertyName appUpdateManifestUrlOverride -NotePropertyValue $address
        $state | Add-Member -Force -NotePropertyName appUpdateAutoCheck -NotePropertyValue $true
        Write-Json $stateFile $state
        Start-Process -FilePath (Join-Path $installDirectory 'MightyCode.exe') | Out-Null
        Write-Host ''
        Write-Host "The installed $From runs on the default profile with the update address $address (automatic update on)."
        Write-Host 'Make the data of plan §7 A step 1, then update from Settings > 앱 업데이트 or by quitting (steps 2-4).'
        Read-Host 'Press Enter here once done to stop the server and remove the certificate (the install stays)'
        return
    }

    Step "$From updates itself on quit"
    $profileDirectory = Join-Path $Work 'profile'
    $workspace = Join-Path $Work 'workspace'
    New-Item -ItemType Directory -Force $profileDirectory, $workspace | Out-Null
    Write-Json (Join-Path $profileDirectory 'workspace-state.json') ([ordered]@{
            version = 1
            workspaces = @([ordered]@{ id = 'rehearsal-workspace'; name = 'Rehearsal'; path = $workspace; createdAt = '2026-11-15T00:00:00.0000000+00:00' })
            sessions = @()
            appUpdateAutoCheck = $true
            appUpdateManifestUrlOverride = $address
        })
    $versionDirectory = Join-Path $profileDirectory "updates\$rustVersion"
    $staged = Join-Path $versionDirectory 'staged'
    $installLog = Join-Path $versionDirectory 'install.log'
    $zip = [IO.Compression.ZipFile]::OpenRead($RustZip)
    try { $entries = @($zip.Entries | Where-Object { $_.Name }).Count } finally { $zip.Dispose() }
    $app = Start-Process -FilePath (Join-Path $installDirectory 'MightyCode.exe') -ArgumentList '--profile', $profileDirectory -PassThru
    Wait-Until 'the staged update' 300 {
        if ($app.HasExited) { throw "the $From app ended (exit $($app.ExitCode)) before it staged the update" }
        (Test-Path (Join-Path $staged 'MightyCode.exe')) -and @(Get-ChildItem -LiteralPath $staged -Recurse -File -ErrorAction SilentlyContinue).Count -ge $entries
    }
    # Staging ends with the phase Ready (the install on quit's condition) right after the last file.
    Start-Sleep -Seconds 5
    Check 'the app staged the whole package' (@(Get-ChildItem -LiteralPath $staged -Recurse -File).Count -eq $entries)
    $closedCleanly = Close-App $app 120
    Check "the $From app quit through its window" $closedCleanly
    Wait-Until 'install.log' 420 { (Get-LastLine $installLog) -and (Get-AppsUnder $staged).Count -eq 0 }
    $line = Get-LastLine $installLog
    Write-Host "  install.log: $line"
    Check 'install.log: replaced on quit' ($line -like '* replaced=True rolledBack=False relaunch=False skipped=False started=False backupRemoved=True *') $line
    Check 'the install holds the Rust exe' ((Get-Sha256 (Join-Path $installDirectory 'MightyCode.exe')) -eq $rustExe)
    $installedInfo = Get-Content -LiteralPath (Join-Path $installDirectory 'build-info.json') -Raw | ConvertFrom-Json
    Check "the install's build-info.json is the Rust app's" ($installedInfo.engine -eq 'tauri' -and $installedInfo.version -eq $rustVersion)
    Check "DisplayVersion is $displayVersion" ((Get-ItemProperty $entry).DisplayVersion -eq $displayVersion) "$((Get-ItemProperty $entry).DisplayVersion)"
    Check 'the uninstall entry still points at the install' ((Get-ItemProperty $entry).InstallLocation.TrimEnd('\') -ieq $installDirectory)
    Check 'no backup or copy beside the install' ((Get-Beside).Count -eq 0) ((Get-Beside) -join ', ')
    $requests = Get-Content -LiteralPath $serverLog -Raw
    Check 'the server was asked for latest.json and the package' ($requests -match 'GET /latest.json' -and $requests -match "GET /MightyCode-windows-$architecture.zip")

    Step 'the installed Rust app passes the smoke'
    $smokeExit = try {
        & (Join-Path $repo 'native\tauri\scripts\smoke.ps1') -Exe (Join-Path $installDirectory 'MightyCode.exe') -NoRuns -Out (Join-Path $Work 'smoke') | Out-Host
        $LASTEXITCODE
    } catch { Write-Host "  $($_.Exception.Message)"; -1 }
    Check 'smoke passed' ($smokeExit -eq 0) "exit $smokeExit"

    Step 'the Rust app reads the rehearsal profile (read-only)'
    $stateBefore = Get-Sha256 (Join-Path $profileDirectory 'workspace-state.json')
    $logs = Join-Path $Work 'read-only-logs'
    $hostLog = Join-Path $logs 'host.log'
    $reader = Start-Process -FilePath (Join-Path $installDirectory 'MightyCode.exe') -ArgumentList '--profile', $profileDirectory, '--read-only', '--log-dir', $logs -PassThru
    Wait-Until 'the read-only start' 120 { $reader.HasExited -or ((Test-Path $hostLog) -and (Get-Content -LiteralPath $hostLog -Raw) -match 'load ') }
    Start-Sleep -Seconds 5
    $startLine = "$(Get-Content -LiteralPath $hostLog -ErrorAction SilentlyContinue | Where-Object { $_ -match ' start ' } | Select-Object -First 1)"
    Write-Host "  host.log: $startLine"
    Check 'the Rust app loaded the 0.4.x state with its workspace, writes off' ($startLine -match "start $([regex]::Escape($rustVersion)) " -and $startLine -match 'load state \(1 workspaces' -and $startLine -match 'writes off$') $startLine
    Check 'the Rust app ran until closed' (-not $reader.HasExited)
    [void](Close-App $reader 60)
    Check 'the read-only run left the state file as it was' ((Get-Sha256 (Join-Path $profileDirectory 'workspace-state.json')) -eq $stateBefore)

    Step "the relaunch path: $From again, then the staged Rust exe as its helper with --relaunch 1"
    Install-Setup $csharpSetup (Join-Path $Work 'setup-2.log')
    Check "$From installed again" ((Get-Sha256 (Join-Path $installDirectory 'MightyCode.exe')) -eq $csharpExe -and (Get-ItemProperty $entry).DisplayVersion -eq $fromVersion)
    $default = Join-Path $env:APPDATA 'MightyCode'
    if (-not (Test-Path (Join-Path $default 'workspace-state.json'))) {
        New-Item -ItemType Directory -Force $default | Out-Null
        Write-Json (Join-Path $default 'workspace-state.json') ([ordered]@{ version = 1; workspaces = @(); sessions = @(); appUpdateAutoCheck = $false })
    }
    $defaultLog = Join-Path $default 'tauri-logs\host.log'
    $logLength = if (Test-Path $defaultLog) { (Get-Item $defaultLog).Length } else { 0 }
    $packagePath = Join-Path $versionDirectory 'MightyCode-windows.zip'
    $plan = Join-Path $Work 'plan.json'
    Write-Json $plan @{ create = @(@{
                staged = $staged; install = $installDirectory; package = $packagePath; sha256 = (Get-Sha256 $packagePath); size = (Get-Item $packagePath).Length
                pid = (Get-ExitedPid); backup = "$installDirectory.backup-$([DateTimeOffset]::UtcNow.ToString('yyyyMMddTHHmmssfff'))"; relaunch = $true
            }); parse = @()
    }
    Invoke-Checked 'update plan-args' { dotnet (Join-Path $parity 'parity.dll') update plan-args $plan $plan }
    $arguments = @((Get-Content -LiteralPath $plan -Raw | ConvertFrom-Json).create[0].csharp)
    Write-Host "  helper: $($arguments -join ' ')"
    # MainWindow.AppUpdate.cs:65-84: the staged exe, UseShellExecute false, CreateNoWindow, %TEMP%, the environment cleared to SystemRoot.
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $staged 'MightyCode.exe'))
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WorkingDirectory = [IO.Path]::GetTempPath()
    foreach ($argument in $arguments) { $start.ArgumentList.Add([string]$argument) }
    $start.Environment.Clear()
    $start.Environment['SystemRoot'] = 'C:\Windows'
    $helper = [Diagnostics.Process]::Start($start)
    if (-not $helper.WaitForExit(420000)) { $helper.Kill(); throw 'the helper did not end within 7 minutes' }
    Check 'the helper exited with 0' ($helper.ExitCode -eq 0) "exit $($helper.ExitCode)"
    $line = Get-LastLine $installLog
    Write-Host "  install.log: $line"
    Check 'install.log: replaced and started' ($line -like '* replaced=True rolledBack=False relaunch=True skipped=False started=True backupRemoved=True *') $line
    Check 'the install holds the Rust exe again' ((Get-Sha256 (Join-Path $installDirectory 'MightyCode.exe')) -eq $rustExe)
    Check "DisplayVersion is $displayVersion again" ((Get-ItemProperty $entry).DisplayVersion -eq $displayVersion)
    Check 'no backup or copy beside the install after the relaunch' ((Get-Beside).Count -eq 0) ((Get-Beside) -join ', ')
    $started = $null
    try {
        Wait-Until "the relaunched app's host.log" 120 {
            (Test-Path $defaultLog) -and (Get-Item $defaultLog).Length -gt $logLength -and
            ((Read-Shared $defaultLog $logLength) -match "start $([regex]::Escape($rustVersion)) ")
        }
        $started = (Read-Shared $defaultLog $logLength).Split("`n") | Where-Object { $_ -match ' start ' } | Select-Object -First 1
    } catch { Write-Host "  $($_.Exception.Message)" }
    Write-Host "  host.log: $started"
    Check 'the relaunched Rust app started on the default profile' ([bool]$started -and $started -match [regex]::Escape("profile $default ")) "$started"
    $relaunched = @(Get-AppsUnder $installDirectory)
    Check 'the relaunched app runs from the install' ($relaunched.Count -ge 1)
    foreach ($process in $relaunched) { [void](Close-App $process 60) }
} finally {
    Step 'clean up'
    foreach ($process in @(Get-AppsUnder $installDirectory) + @(Get-AppsUnder $Work)) { try { $process.Kill($true) } catch { } }
    # What a failed run is read from (CI keeps <Work>\logs): install.log, the 0.4.x app's logs, the Rust app's host.log files.
    $kept = Join-Path $Work 'logs'
    New-Item -ItemType Directory -Force $kept | Out-Null
    foreach ($source in @(
            @{ from = (Join-Path $Work 'profile'); filter = '*.log'; to = 'profile' },
            @{ from = (Join-Path $env:APPDATA 'MightyCode\tauri-logs'); filter = '*.log'; to = 'default-profile-tauri-logs' })) {
        if (-not (Test-Path $source.from)) { continue }
        Get-ChildItem -LiteralPath $source.from -Recurse -File -Filter $source.filter -ErrorAction SilentlyContinue | ForEach-Object {
            $relative = $_.FullName.Substring($source.from.Length).TrimStart('\')
            $destination = Join-Path (Join-Path $kept $source.to) $relative
            New-Item -ItemType Directory -Force (Split-Path $destination) | Out-Null
            Copy-Item -LiteralPath $_.FullName -Destination $destination -ErrorAction SilentlyContinue
        }
    }
    if ($serverProcess -and -not $serverProcess.HasExited) { $serverProcess.Kill() }
    if ($certThumbprint -and $certStore) { Remove-Item -LiteralPath (Join-Path $certStore $certThumbprint) -ErrorAction SilentlyContinue }
    if (-not $Interactive -and (Test-Path $entry)) {
        $uninstaller = ((Get-ItemProperty $entry).UninstallString).Trim('"')
        if (Test-Path $uninstaller) { Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait | Out-Null }
    }
    if (Test-Path $worktree) { git -C $repo worktree remove --force $worktree 2>&1 | Out-Null }
}

if (-not $Interactive) {
    Write-Json (Join-Path $Work 'rehearsal-result.json') ([ordered]@{ from = $From; to = $rustVersion; architecture = $architecture; passed = $failures.Count -eq 0; checks = $checks })
    Write-Host ''
    if ($failures.Count -gt 0) { Write-Host "rehearsal from ${From}: $($failures.Count) check(s) failed: $($failures -join '; ')"; exit 1 }
    Write-Host "rehearsal from ${From}: passed ($($checks.Count) checks)"
}
exit 0
