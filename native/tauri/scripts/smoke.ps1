# Runs the Tauri app's smoke (src-tauri/src/smoke.rs) and prints its result.
#
#   .\native\tauri\scripts\smoke.ps1                         a new demo profile under %TEMP% (seed_demo), English
#   .\native\tauri\scripts\smoke.ps1 -Language ko            the same in Korean
#   .\native\tauri\scripts\smoke.ps1 -Profile <folder>       an existing profile, read-only: the result and the logs go to -Out,
#                                                            and every file of the profile (but -Out and tauri-logs) is compared
#                                                            before/after: any file added, removed or changed fails the run
#   .\native\tauri\scripts\smoke.ps1 -NoRuns                  without the run steps (the composer, cards and Mighty steps run a
#                                                            stand-in Claude, mighty-run's mighty-fake-cli, unless this is given)
#   .\native\tauri\scripts\smoke.ps1 -RealClaude              one real request to the installed claude CLI instead of the run steps
#                                                            (a new demo profile, writes off, a temporary workspace folder)
#   .\native\tauri\scripts\smoke.ps1 -NativeDump <file>       every native and text push also written to <file> (--native-dump; the
#                                                            parity capture, tools\parity\native.ps1)
#
# Exits with the app's code: 0 when the smoke passed (on the Mac: when every check outside smoke.rs' MAC_PENDING_STEPS passed).
#
# Mac stage Step 0: runs under PowerShell 7 on the Mac too (pwsh, as CI's Mac leg runs it): the exe is target/debug/MightyCode, the
# process tree is read with ps, and the user's places it compares are the Mac's (the dev.mightycode.native defaults domain, Application
# Support, ~/.claude and ~/.local/bin) instead of HKCU, the install folder and the firewall rule.
param(
    [string]$Exe,
    [string]$Language = 'en',
    [string]$Profile,
    [string]$Out,
    [int]$RemoteDebuggingPort = 0,
    [string]$NativeDump,
    [switch]$NoRuns,
    [switch]$RealClaude
)
$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 has no $IsWindows (and runs only there).
$onWindows = ($PSVersionTable.PSEdition -ne 'Core') -or $IsWindows
$sep = [IO.Path]::DirectorySeparatorChar
$temp = [IO.Path]::GetTempPath().TrimEnd($sep)
$home_ = [Environment]::GetFolderPath('UserProfile')
$exeSuffix = if ($onWindows) { '.exe' } else { '' }
if (-not $Exe) { $Exe = Join-Path $PSScriptRoot ('..' + $sep + 'target' + $sep + 'debug' + $sep + 'MightyCode' + $exeSuffix) }
if ($onWindows) { . (Join-Path $PSScriptRoot 'env.ps1') }
$Exe = (Resolve-Path $Exe).Path

if (-not $Profile) {
    $Profile = Join-Path $temp ('mighty-tauri-smoke-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    Push-Location (Join-Path $PSScriptRoot '..')
    try { cargo run -q -p mighty-core --example seed_demo -- $Profile $Language; if ($LASTEXITCODE -ne 0) { throw 'seed_demo failed' } }
    finally { Pop-Location }
    if (-not $Out) { $Out = $Profile }
    $readOnly = $false
} else {
    $Profile = (Resolve-Path $Profile).Path
    if (-not $Out) { $Out = Join-Path $temp ('mighty-tauri-smoke-out-' + [guid]::NewGuid().ToString('N').Substring(0, 8)) }
    $readOnly = $true
}
New-Item -ItemType Directory -Force $Out | Out-Null
$result = Join-Path $Out 'smoke-result.json'

# One probe of what must not change, its failure written as its value (and so a difference) rather than stopping the script.
function Probe([scriptblock]$Read) {
    try { & $Read } catch { "probe failed: $($_.Exception.Message)" }
}
# Every file of the profile as "relative path" = "bytes time hash", leaving out -Out (when it is inside) and the logs.
function ProfileTree {
    $values = [ordered]@{}
    $root = $Profile.TrimEnd($sep)
    $outFull = [IO.Path]::GetFullPath($Out).TrimEnd($sep) + $sep
    # Folders too, so an empty one a run made (a hooks folder) is caught as surely as a file.
    $files = Get-ChildItem -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue | Sort-Object FullName
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($root.Length + 1)
        if (($file.FullName + $sep).StartsWith($outFull, [StringComparison]::OrdinalIgnoreCase)) { continue }
        if ($relative -eq 'tauri-logs' -or $relative -like ('tauri-logs' + $sep + '*')) { continue }
        if ($file.PSIsContainer) { $values[$relative + $sep] = 'folder'; continue }
        $values[$relative] = Probe { "$($file.Length) $($file.LastWriteTimeUtc.ToString('o')) $((Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash)" }
    }
    $values
}
# The values under a registry key and all its subkeys, as "key\name" = "value" (a key without values as "key\" = ''). Each key is
# read on its own: one that cannot be read is its own "probe failed" entry and every other key is still compared.
function RegistryTree([string]$Root, $Into) {
    if (-not (Test-Path $Root)) { $Into[$Root] = $null; return }
    $pending = New-Object 'System.Collections.Generic.Queue[string]'
    $pending.Enqueue($Root)
    while ($pending.Count -gt 0) {
        $path = $pending.Dequeue()
        try { $key = Get-Item -LiteralPath $path -ErrorAction Stop } catch { $Into["$path\"] = "probe failed: $($_.Exception.Message)"; continue }
        foreach ($name in $key.GetValueNames()) {
            $Into["$($key.Name)\$name"] = Probe { "$($key.GetValue($name, $null, 'DoNotExpandEnvironmentNames'))" }
        }
        if ($key.ValueCount -eq 0) { $Into["$($key.Name)\"] = '' }
        try {
            foreach ($child in Get-ChildItem -LiteralPath $path -ErrorAction Stop) { $pending.Enqueue($child.PSPath) }
        } catch { $Into["$($key.Name)\*"] = "probe failed: $($_.Exception.Message)" }
    }
}
# The user's settings outside any profile, which a smoke must never change (W6 plan §4.4): HKCU Software\MightyCode (shared with the
# installed C# app) and its subkeys, the app's toast registration, Claude's settings.json and omc's and Ouroboros's configs (their times
# and hashes).
function UserConfig {
    $values = [ordered]@{}
    if (-not $onWindows) {
        # The Mac's (Mac stage): the defaults domain the installed 0.4.x app shares with this one (read with `defaults export`, which
        # writes nothing), the default data folder's top level, Claude's and the CLIs' settings, and the shell command.
        $values['defaults dev.mightycode.native'] = Probe {
            $plist = & /usr/bin/defaults export dev.mightycode.native - 2>$null | Out-String
            $bytes = [Text.Encoding]::UTF8.GetBytes($plist)
            [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($bytes)).Replace('-', '')
        }
        $support = Join-Path $home_ 'Library/Application Support/MightyCode'
        $values[$support] = Probe {
            if (Test-Path -LiteralPath $support) { @(Get-ChildItem -LiteralPath $support -Force -ErrorAction Stop | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.LastWriteTimeUtc.ToString('o'))" }) -join '|' } else { $null }
        }
        foreach ($file in (Join-Path $home_ '.claude/settings.json'), (Join-Path $home_ '.config/claude-omc/config.jsonc'), (Join-Path $home_ '.ouroboros/config.yaml'), (Join-Path $home_ '.local/bin/mighty')) {
            $values[$file] = Probe { if (Test-Path $file) { $info = Get-Item $file; "$($info.LastWriteTimeUtc.ToString('o')) $((Get-FileHash $file -Algorithm SHA256).Hash)" } else { $null } }
        }
        return $values
    }
    foreach ($key in 'HKCU:\Software\MightyCode', 'HKCU:\Software\Classes\AppUserModelId\dev.mightycode.native', 'HKCU:\Software\Classes\CLSID\{E9E66915-2599-4971-9584-4825CAC5DEEE}',
            'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{F2F99AEF-FB35-4476-A3EE-49453FA0527F}_is1') {
        try { RegistryTree $key $values } catch { $values[$key] = "probe failed: $($_.Exception.Message)" }
    }
    # The installed app (W9 plan §4): its folder's top level and exe untouched, and no MightyCode.backup-* or MightyCode.update-new beside it.
    $programs = Join-Path $env:LOCALAPPDATA 'Programs'
    $values["$programs\MightyCode*"] = Probe { @(Get-ChildItem -LiteralPath $programs -Filter 'MightyCode*' -Force -ErrorAction SilentlyContinue | Sort-Object Name | ForEach-Object Name) -join '|' }
    $install = Join-Path $programs 'MightyCode'
    $values[$install] = Probe {
        if (-not (Test-Path -LiteralPath $install)) { $null } else {
            @(Get-ChildItem -LiteralPath $install -Force -ErrorAction Stop | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.LastWriteTimeUtc.ToString('o'))" }) -join '|'
        }
    }
    $exe = Join-Path $install 'MightyCode.exe'
    $values[$exe] = Probe { if (Test-Path -LiteralPath $exe) { (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash } else { $null } }
    foreach ($file in (Join-Path $env:USERPROFILE '.claude\settings.json'), (Join-Path $env:USERPROFILE '.config\claude-omc\config.jsonc'), (Join-Path $env:USERPROFILE '.ouroboros\config.yaml')) {
        $values[$file] = Probe { if (Test-Path $file) { $info = Get-Item $file; "$($info.LastWriteTimeUtc.ToString('o')) $((Get-FileHash $file -Algorithm SHA256).Hash)" } else { $null } }
    }
    $values
}
# What delegation may change outside the profile's own files, which a read-only run must leave alone (W7 plan section 4): the listing of
# %USERPROFILE%\.mightycode\worktrees, the profile's delegation-state.json (its hash), and `git worktree list` of each local workspace the
# profile names (read with optional locks off, so the probe itself writes nothing).
function DelegationPlaces {
    $values = [ordered]@{}
    $worktrees = Join-Path (Join-Path $home_ '.mightycode') 'worktrees'
    $values[$worktrees] = Probe { if (Test-Path -LiteralPath $worktrees) { (Get-ChildItem -LiteralPath $worktrees -Force -ErrorAction Stop | Sort-Object Name | ForEach-Object Name) -join '|' } else { $null } }
    $file = Join-Path $Profile 'delegation-state.json'
    $values[$file] = Probe { if (Test-Path -LiteralPath $file) { (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash } else { $null } }
    $state = Join-Path $Profile 'workspace-state.json'
    $paths = Probe { if (Test-Path -LiteralPath $state) { @((Get-Content -LiteralPath $state -Raw -Encoding utf8 | ConvertFrom-Json).workspaces | Where-Object { $null -eq $_.remote -and $_.path } | ForEach-Object { [string]$_.path }) } else { @() } }
    if ($paths -is [string] -and $paths.StartsWith('probe failed')) { $values[$state] = $paths; return $values }
    $previous = $env:GIT_OPTIONAL_LOCKS; $env:GIT_OPTIONAL_LOCKS = '0'
    try {
        foreach ($path in $paths | Sort-Object -Unique) {
            if (-not (Test-Path -LiteralPath (Join-Path $path '.git'))) { continue }
            $values["git worktree list $path"] = Probe { ((& git -C $path worktree list --porcelain 2>&1) | Out-String).Trim() }
        }
    } finally { $env:GIT_OPTIONAL_LOCKS = $previous }
    $values
}
# What the phone, remote workspaces, the pet and screen share keep, which a read-only run must leave alone (W8 plan section 4): the
# profile's companion-settings.json, remote-connections.json and screen-devices.json (their hashes), its mobile-remote folder (the pairing
# identity and uploads) and remote-keys folder (the DPAPI-sealed connection keys), each file by bytes, time and hash, and the Windows
# Firewall rule the direct path and sharing share (netsh's unelevated read, which changes nothing).
function RemotePlaces {
    $values = [ordered]@{}
    foreach ($name in 'companion-settings.json', 'remote-connections.json', 'screen-devices.json') {
        $file = Join-Path $Profile $name
        $values[$file] = Probe { if (Test-Path -LiteralPath $file) { $info = Get-Item -LiteralPath $file; "$($info.Length) $($info.LastWriteTimeUtc.ToString('o')) $((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash)" } else { $null } }
    }
    foreach ($name in 'mobile-remote', 'remote-keys') {
        $folder = Join-Path $Profile $name
        $values[$folder] = Probe {
            if (-not (Test-Path -LiteralPath $folder)) { $null } else {
                $root = (Get-Item -LiteralPath $folder).FullName.TrimEnd($sep)
                @(Get-ChildItem -LiteralPath $root -Recurse -Force -ErrorAction Stop | Sort-Object FullName | ForEach-Object {
                    $relative = $_.FullName.Substring($root.Length + 1)
                    if ($_.PSIsContainer) { "$relative$sep" } else { "$relative $($_.Length) $($_.LastWriteTimeUtc.ToString('o')) $((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)" }
                }) -join '|'
            }
        }
    }
    # The Mac has no firewall rule (PlatformProfile::direct_connection_firewall).
    if ($onWindows) { $values['firewall rule Mighty Code (Tailscale)'] = Probe { ((& (Join-Path $env:SystemRoot 'System32\netsh.exe') advfirewall firewall show rule 'name=Mighty Code (Tailscale)' verbose 2>&1) | Out-String).Trim() } }
    $values
}
# The names whose value differs between two probes (added and removed ones included).
function Changed($Before, $After) {
    @(@($Before.Keys) + @($After.Keys) | Sort-Object -Unique | Where-Object { $Before[$_] -ne $After[$_] })
}

$treeBefore = if ($readOnly) { ProfileTree } else { $null }; $userBefore = UserConfig
$placesBefore = if ($readOnly) { DelegationPlaces } else { $null }
$remoteBefore = if ($readOnly) { RemotePlaces } else { $null }
$arguments = @('--smoke-test', '--smoke-exit', '--profile', $Profile, '--smoke-out', $result)
if ($RealClaude) {
    $arguments += '--smoke-real-claude'
} elseif (-not $NoRuns) {
    # The run steps' stand-in Claude: mighty-run's fake CLI in its --replay-cli mode (asks a permission and a plan, then replays).
    Push-Location (Join-Path $PSScriptRoot '..')
    try { cargo build -q -p mighty-run --bin mighty-fake-cli; if ($LASTEXITCODE -ne 0) { throw 'building mighty-fake-cli failed' } }
    finally { Pop-Location }
    $arguments += @('--smoke-fake-cli', (Resolve-Path (Join-Path (Split-Path $Exe) ('mighty-fake-cli' + $exeSuffix))).Path)
}
if ($readOnly) { $arguments += @('--log-dir', (Join-Path $Out 'tauri-logs')) }
if ($RemoteDebuggingPort -gt 0) { $arguments += @('--remote-debugging-port', "$RemoteDebuggingPort") }
if ($NativeDump) { $arguments += @('--native-dump', [IO.Path]::GetFullPath($NativeDump)) }
$watch = [Diagnostics.Stopwatch]::StartNew()
# Waits for the app alone (Start-Process -Wait would also wait for every process it left behind, such as a detached git daemon), then
# lists what of its process tree is still running: anything but its console host fails the run, and is stopped so nothing outlives it.
$process = if ($onWindows) { Start-Process -FilePath $Exe -ArgumentList $arguments -PassThru -WindowStyle Normal } else { Start-Process -FilePath $Exe -ArgumentList $arguments -PassThru }
$null = $process.Handle  # keeps the exit code readable once it ended
$appStart = $process.StartTime
$process.WaitForExit()
$watch.Stop()
# The app's process tree by parent ids. A process id is reused once its process ended, so a process counts as a child only when it was
# made after its parent started (the app's start, then each child's own), and each id is visited once (no loop through a reused id).
# The Mac's processes as Win32_Process gives them: `ps` with each one's start (to the second, so a child counts from a second before its
# parent's start).
function MacProcesses {
    foreach ($line in @(& /bin/ps -A -o pid=,ppid=,lstart=,command= 2>$null)) {
        $parts = ($line.Trim() -split '\s+', 8)
        if ($parts.Count -lt 8) { continue }
        $started = [datetime]::MinValue
        $null = [datetime]::TryParseExact(($parts[2..6] -join ' '), 'ddd MMM d HH:mm:ss yyyy', [Globalization.CultureInfo]::InvariantCulture, 'AllowWhiteSpaces', [ref]$started)
        [pscustomobject]@{ ProcessId = [int]$parts[0]; ParentProcessId = [int]$parts[1]; CreationDate = $started; Name = (Split-Path -Leaf ($parts[7] -split ' ')[0]); CommandLine = $parts[7] }
    }
}
function ChildProcesses([int]$parent) {
    if ($onWindows) { @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$parent" -ErrorAction SilentlyContinue) }
    else { @(MacProcesses | Where-Object { $_.ParentProcessId -eq $parent }) }
}
function Descendants([int]$parent, [datetime]$since, $seen) {
    if (-not $onWindows) { $since = $since.AddSeconds(-1) }
    foreach ($child in ChildProcesses $parent) {
        if ($child.CreationDate -lt $since -or -not $seen.Add([int]$child.ProcessId)) { continue }
        $child
        Descendants $child.ProcessId $child.CreationDate $seen
    }
}
function AppTree { Descendants $process.Id $appStart (New-Object 'System.Collections.Generic.HashSet[int]') }
# Stopped only while the id still names the process found (its creation time unchanged).
function StopFound($found) {
    $now = if ($onWindows) { Get-CimInstance Win32_Process -Filter "ProcessId=$($found.ProcessId)" -ErrorAction SilentlyContinue } else { MacProcesses | Where-Object { $_.ProcessId -eq $found.ProcessId } }
    if ($now -and $now.CreationDate -eq $found.CreationDate) { Stop-Process -Id $found.ProcessId -Force -ErrorAction SilentlyContinue }
}
# What the app started may still be ending (a run's CLI, git) on a loaded machine: given up to 15 s before it counts as left over.
$deadline = (Get-Date).AddSeconds(15)
do {
    $leftovers = @(AppTree | Where-Object { $_.Name -ne 'conhost.exe' })
    if ($leftovers.Count -eq 0) { break }
    Start-Sleep -Milliseconds 500
} while ((Get-Date) -lt $deadline)
$treeAfter = if ($readOnly) { ProfileTree } else { $null }; $userAfter = UserConfig
$placesAfter = if ($readOnly) { DelegationPlaces } else { $null }
$remoteAfter = if ($readOnly) { RemotePlaces } else { $null }
$userChanged = Changed $userBefore $userAfter
$exitCode = $process.ExitCode
if ($leftovers.Count -gt 0) {
    Write-Output "LEFTOVER PROCESSES (the app ended, these of its process tree did not; stopped now):"
    foreach ($left in $leftovers) {
        Write-Output ("  {0} {1}: {2}" -f $left.ProcessId, $left.Name, $left.CommandLine)
        StopFound $left
    }
    $exitCode = 1
} else {
    Write-Output 'leftover processes: none'
}
foreach ($console in @(AppTree)) { StopFound $console }

Write-Output "profile:  $Profile"
Write-Output "result:   $result"
Write-Output ("exit:     {0} after {1:N1} s" -f $process.ExitCode, $watch.Elapsed.TotalSeconds)
if ($readOnly) {
    $profileChanged = Changed $treeBefore $treeAfter
    if ($profileChanged.Count -gt 0) {
        Write-Output "PROFILE CHANGED (a read-only run must leave every file alone; another writer, such as an app open on this profile, may be the cause):"
        foreach ($name in $profileChanged) { Write-Output "  $name`: $($treeBefore[$name]) -> $($treeAfter[$name])" }
        $exitCode = 1
    } else {
        Write-Output "profile unchanged: $($treeBefore.Count) files"
    }
    $placesChanged = Changed $placesBefore $placesAfter
    if ($placesChanged.Count -gt 0) {
        Write-Output "DELEGATION PLACES CHANGED (a read-only run must leave the worktrees, the delegation file and the workspaces' worktrees alone):"
        foreach ($name in $placesChanged) { Write-Output "  $name`: $($placesBefore[$name]) -> $($placesAfter[$name])" }
        $exitCode = 1
    } else {
        Write-Output "delegation places unchanged: $($placesBefore.Count) probes (~\.mightycode\worktrees, delegation-state.json, git worktree list)"
    }
    $remoteChanged = Changed $remoteBefore $remoteAfter
    if ($remoteChanged.Count -gt 0) {
        Write-Output "REMOTE PLACES CHANGED (a read-only run must leave the pet's, the phone's, the remote workspaces' and screen share's files and the firewall rule alone):"
        foreach ($name in $remoteChanged) { Write-Output "  $name`: $($remoteBefore[$name]) -> $($remoteAfter[$name])" }
        $exitCode = 1
    } else {
        Write-Output "remote places unchanged: $($remoteBefore.Count) probes (companion-settings.json, remote-connections.json, screen-devices.json, mobile-remote, remote-keys, the firewall rule)"
    }
}
if ($userChanged.Count -gt 0) {
    Write-Output "USER SETTINGS CHANGED (the smoke must leave them alone; another writer, such as the installed app or a Claude session, may be the cause):"
    foreach ($name in $userChanged) { Write-Output "  $name`: $($userBefore[$name]) -> $($userAfter[$name])" }
    $exitCode = 1
} else {
    if ($onWindows) { Write-Output 'user settings unchanged: HKCU Software\MightyCode and its subkeys, the toast registration (AppUserModelId and CLSID), the uninstall entry, the installed app folder, ~\.claude\settings.json, omc and Ouroboros configs' }
    else { Write-Output 'user settings unchanged: the dev.mightycode.native defaults domain, ~/Library/Application Support/MightyCode, ~/.claude/settings.json, omc and Ouroboros configs, ~/.local/bin/mighty' }
}
if (Test-Path $result) { Get-Content $result -Raw -Encoding utf8 } else { Write-Output 'no smoke-result.json' }
if (-not $readOnly) {
    # The seeded profile's WebView2 data (Paths::webview_dir and browser_dir: <local data>\webview|browser\<the profile path as a
    # folder name>) goes with it.
    # This machine's local data: %LOCALAPPDATA% on Windows, ~/Library/Application Support on the Mac (dirs::data_local_dir).
    $localData = if ($onWindows) { $env:LOCALAPPDATA } else { Join-Path $home_ 'Library/Application Support' }
    $name = $Profile -replace '[^A-Za-z0-9-]', '_'
    if ($name.Length -gt 80) { $name = $name.Substring($name.Length - 80) }
    foreach ($kind in 'webview', 'browser', 'image-cache', 'file-originals', 'provider-catalog') {
        Remove-Item -Recurse -Force (Join-Path (Join-Path (Join-Path $localData 'dev.mightycode.native') $kind) $name) -ErrorAction SilentlyContinue
    }
}
# A run that may not write its profile keeps delegation's fixture under %TEMP% (delegation.rs smoke_root): it goes too.
Remove-Item -Recurse -Force (Join-Path $temp ("mighty-delegation-smoke-" + $process.Id)) -ErrorAction SilentlyContinue
exit $exitCode
