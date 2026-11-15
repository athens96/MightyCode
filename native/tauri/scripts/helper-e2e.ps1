# Runs a built MightyCode.exe as the 0.4.x app runs the update helper (W9 plan §W9-B, R1): copied alone into a temporary staged folder,
# started from there with the helper's arguments, an environment of SystemRoot=C:\Windows only, the temporary folder as its working
# folder and no window (MainWindow.AppUpdate.cs:65-84, through .NET's ProcessStartInfo as the C# app does), against a temporary install
# folder holding a stand-in exe. Never the real install folder, %APPDATA% or HKCU.
#
#   .\native\tauri\scripts\helper-e2e.ps1                     the debug build (target\debug\MightyCode.exe)
#   .\native\tauri\scripts\helper-e2e.ps1 -Exe <path>         any built exe (CI: the release build, after the package job built it)
#
# Two runs, both with --relaunch 0 (the install on quit: the helper starts nothing) and a --wait-pid that has already exited:
#   1. the package as staged: exit 0, the install folder holds the built exe and nothing else of the old one, no backup or copy beside
#      it, and install.log's last line is C#'s "replaced=True rolledBack=False relaunch=False skipped=False started=False backupRemoved=True";
#   2. a package changed since it was checked (another SHA-256): exit 1, the install folder untouched, "replaced=False".
# Exit code 0 when both hold, 1 otherwise. Owner: W9-B; CI calls it after the release build (W9-G).
param(
    [string]$Exe
)
$ErrorActionPreference = 'Stop'
if (-not $Exe) {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot '..\target' }
    $Exe = Join-Path $target 'debug\MightyCode.exe'
}
if (-not (Test-Path $Exe -PathType Leaf)) { throw "no exe at $Exe" }
$Exe = (Resolve-Path $Exe).Path
$built = (Get-FileHash -Algorithm SHA256 -LiteralPath $Exe).Hash

function New-Layout([string]$name) {
    $root = Join-Path ([IO.Path]::GetTempPath()) ("mighty-helper-e2e-$name-" + [Guid]::NewGuid().ToString('N'))
    $version = Join-Path $root 'updates\0.5.0'
    $layout = [pscustomobject]@{
        Root    = $root
        Install = Join-Path $root 'MightyCode'
        Version = $version
        Staged  = Join-Path $version 'staged'
        Package = Join-Path $version 'MightyCode-windows.zip'
    }
    New-Item -ItemType Directory -Force $layout.Install, $layout.Staged | Out-Null
    [IO.File]::WriteAllText((Join-Path $layout.Install 'MightyCode.exe'), 'the old app')
    [IO.File]::WriteAllText((Join-Path $layout.Install 'old.txt'), 'old')
    Copy-Item -LiteralPath $Exe -Destination (Join-Path $layout.Staged 'MightyCode.exe')
    New-Item -ItemType Directory -Force (Join-Path $layout.Staged 'claude-mods') | Out-Null
    [IO.File]::WriteAllText((Join-Path $layout.Staged 'claude-mods\mod.json'), '{}')
    [IO.File]::WriteAllBytes($layout.Package, [Text.Encoding]::ASCII.GetBytes('PK the package the 0.4.x app downloaded'))
    $layout
}

# The id of a process that has already exited (the app that started the helper is gone).
function Get-ExitedPid {
    $process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\cmd.exe') -ArgumentList '/d', '/c', 'exit' -WindowStyle Hidden -PassThru
    $process.WaitForExit()
    $process.Id
}

# C#'s launch: UseShellExecute false, CreateNoWindow, the temporary folder, the environment cleared to SystemRoot alone.
function Invoke-Helper($layout, [string]$sha256) {
    $size = (Get-Item -LiteralPath $layout.Package).Length
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = Join-Path $layout.Staged 'MightyCode.exe'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.WorkingDirectory = [IO.Path]::GetTempPath()
    $info.EnvironmentVariables.Clear()
    $info.EnvironmentVariables['SystemRoot'] = 'C:\Windows'
    $arguments = @(
        '--update-helper',
        '--staged', $layout.Staged,
        '--install', $layout.Install,
        '--backup', (Join-Path $layout.Root 'MightyCode.backup-20261115T030405123'),
        '--package', $layout.Package,
        '--sha256', $sha256,
        '--size', "$size",
        '--wait-pid', "$(Get-ExitedPid)",
        '--relaunch', '0'
    )
    # .NET Framework's ProcessStartInfo has no ArgumentList: quote each argument as CommandLineToArgvW reads it (none ends in a backslash
    # or holds a quote here).
    $info.Arguments = ($arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $process = [Diagnostics.Process]::Start($info)
    if (-not $process.WaitForExit(120000)) { $process.Kill(); throw 'the helper did not end within 2 minutes' }
    $process.ExitCode
}

function Get-LastLogLine($layout) {
    $log = Join-Path $layout.Version 'install.log'
    if (-not (Test-Path $log)) { return '' }
    (Get-Content -LiteralPath $log | Select-Object -Last 1)
}

function Get-Beside($layout) {
    @(Get-ChildItem -LiteralPath $layout.Root -Directory | Where-Object { $_.Name -like 'MightyCode.*' } | ForEach-Object { $_.Name })
}

$failures = @()
function Check([bool]$condition, [string]$what) {
    if ($condition) { Write-Host "  ok   $what" } else { Write-Host "  FAIL $what"; $script:failures += $what }
}

Write-Host "helper-e2e: $Exe"
Write-Host '1. the staged package installs on quit'
$swap = New-Layout 'swap'
try {
    $code = Invoke-Helper $swap (Get-FileHash -Algorithm SHA256 -LiteralPath $swap.Package).Hash.ToLowerInvariant()
    $line = Get-LastLogLine $swap
    Check ($code -eq 0) "exit code 0 (got $code; $line)"
    $installed = Join-Path $swap.Install 'MightyCode.exe'
    Check ((Test-Path $installed) -and (Get-FileHash -Algorithm SHA256 -LiteralPath $installed).Hash -eq $built) 'the install holds the built exe'
    Check (-not (Test-Path (Join-Path $swap.Install 'old.txt'))) 'nothing of the old install is left in it'
    Check (Test-Path (Join-Path $swap.Install 'claude-mods\mod.json')) 'the whole staged tree is in it'
    Check ((Get-Beside $swap).Count -eq 0) "no backup or copy beside it ($((Get-Beside $swap) -join ', '))"
    Check ($line -like '* replaced=True rolledBack=False relaunch=False skipped=False started=False backupRemoved=True *') "install.log: $line"
}
finally { Remove-Item -LiteralPath $swap.Root -Recurse -Force -ErrorAction SilentlyContinue }

Write-Host '2. a changed package leaves the install untouched'
$tampered = New-Layout 'tampered'
try {
    $code = Invoke-Helper $tampered ('ab' * 32)
    $line = Get-LastLogLine $tampered
    Check ($code -eq 1) "exit code 1 (got $code; $line)"
    Check ([IO.File]::ReadAllText((Join-Path $tampered.Install 'MightyCode.exe')) -eq 'the old app') 'the install still holds the old app'
    Check (Test-Path (Join-Path $tampered.Install 'old.txt')) 'and all of it'
    Check ((Get-Beside $tampered).Count -eq 0) 'no backup or copy beside it'
    Check ($line -like '* replaced=False rolledBack=False relaunch=False skipped=False started=False backupRemoved=False *') "install.log: $line"
}
finally { Remove-Item -LiteralPath $tampered.Root -Recurse -Force -ErrorAction SilentlyContinue }

if ($failures.Count -gt 0) {
    Write-Host "helper-e2e: $($failures.Count) check(s) failed"
    exit 1
}
Write-Host 'helper-e2e: passed'
exit 0
