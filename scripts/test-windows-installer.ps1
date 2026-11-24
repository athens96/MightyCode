# Installs a setup program built by build-windows.ps1 -Installer silently for this user, checks what it left, then
# uninstalls it and checks that nothing of the app is left. Settings and conversations are not touched: the app is
# never started. Runs in Windows PowerShell 5.1 or PowerShell 7.
# -Layout tauri checks a setup program built by build-tauri-windows.ps1 -Installer (the Rust + Tauri app's folder: the exe
# beside claude-mods, Agents and pets; W9 plan §1.6); -Version is then the version's major.minor.patch, as the setup
# program and the uninstall entry carry it.
param(
    [Parameter(Mandatory = $true)][string]$Setup,
    [Parameter(Mandatory = $true)][string]$Version,
    [ValidateSet('winui', 'tauri')][string]$Layout = 'winui'
)
$ErrorActionPreference = 'Stop'
$installDirectory = Join-Path $env:LOCALAPPDATA 'Programs\MightyCode'
$entry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{F2F99AEF-FB35-4476-A3EE-49453FA0527F}_is1'
if (Test-Path $entry) { throw "Mighty Code is already installed for this user; uninstall it before this check: $installDirectory" }
if (Test-Path $installDirectory) { throw "The install folder already exists; this check never deletes a folder it did not make: $installDirectory" }

function Invoke-Quiet([string]$Path, [string[]]$Arguments) {
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$([IO.Path]::GetFileName($Path)) exited with $($process.ExitCode)" }
}

# Setup empties its folder before installing, so it must refuse any folder but its own, whatever /DIR= says.
$elsewhere = Join-Path ([IO.Path]::GetTempPath()) "mighty-setup-elsewhere-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $elsewhere | Out-Null
Set-Content (Join-Path $elsewhere 'keep.txt') 'x'
$refused = Start-Process -FilePath $Setup -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$elsewhere`"") -Wait -PassThru
if ($refused.ExitCode -eq 0 -or -not (Test-Path (Join-Path $elsewhere 'keep.txt')) -or (Test-Path $entry)) { throw "setup installed into or emptied another folder given by /DIR= (exit $($refused.ExitCode))" }
Remove-Item -Recurse -Force $elsewhere
Write-Output "refused /DIR= elsewhere (exit $($refused.ExitCode))"

$log = Join-Path ([IO.Path]::GetTempPath()) "mighty-setup-$([Guid]::NewGuid().ToString('N')).log"
# A Start menu shortcut left by an install from before the rename.
$oldShortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Mighty Claude.lnk'
Set-Content $oldShortcut 'old'
Invoke-Quiet $Setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"")
if (Test-Path $oldShortcut) { throw "setup left the shortcut from before the rename: $oldShortcut" }
$requiredFiles = if ($Layout -eq 'tauri') {
    @('MightyCode.exe', 'bin\mighty.exe', 'build-info.json', 'claude-mods\.claude-plugin\plugin.json', 'Agents\pi-ask-user.js', 'pets\mighty-raccoon\pet.json')
} else {
    @('MightyCode.exe', 'MightyCode.dll', 'resources.pri', 'Microsoft.UI.Xaml.dll', 'Assets\MightyCode.ico', 'Assets\Web\index.html', 'claude-mods\.claude-plugin\plugin.json')
}
foreach ($required in $requiredFiles) {
    if (-not (Test-Path (Join-Path $installDirectory $required) -PathType Leaf)) { throw "setup did not install $required (log: $log)" }
}
$values = Get-ItemProperty $entry
if ($values.DisplayVersion -ne $Version) { throw "the uninstall entry says $($values.DisplayVersion), expected $Version" }
if ($values.InstallLocation.TrimEnd('\') -ne $installDirectory) { throw "the uninstall entry points at $($values.InstallLocation)" }
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Mighty Code.lnk'
if (-not (Test-Path $shortcut)) { throw "setup made no Start menu shortcut: $shortcut" }
Write-Output "installed $Version into $installDirectory"

# MC-43: the PATH task (on by default) put the mighty command's folder on this user's PATH; uninstall takes it off again.
$commandFolder = Join-Path $installDirectory 'bin'
function Get-UserPath { [string](Get-ItemProperty -Path 'HKCU:\Environment' -Name Path -ErrorAction SilentlyContinue).Path }
function Test-OnUserPath { @((Get-UserPath) -split ';' | Where-Object { $_.Trim().Trim('"').TrimEnd('\') -ieq $commandFolder }).Count -gt 0 }
if ($Layout -eq 'tauri') {
    if (-not (Test-OnUserPath)) { throw "setup did not put $commandFolder on the user's PATH: $(Get-UserPath)" }
    & (Join-Path $commandFolder 'mighty.exe') --help | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "mighty --help exited with $LASTEXITCODE" }
    Write-Output "the mighty command's folder is on the user's PATH and mighty --help answers"
}

$uninstaller = ($values.UninstallString).Trim('"')
if ($uninstaller.StartsWith("$installDirectory\", [StringComparison]::OrdinalIgnoreCase)) { throw "the uninstaller sits in the install folder, which the app's own update replaces: $uninstaller" }

# What the app's own update does (AppUpdateReplacement): the install folder is renamed aside and the new one copied in. Here
# the backup is left behind as by a stopped update, and the new folder has a file the uninstall log does not list.
$backup = "$installDirectory.backup-20260101T000000000"
Rename-Item $installDirectory (Split-Path -Leaf $backup)
Copy-Item -Recurse $backup $installDirectory
Set-Content (Join-Path $installDirectory 'added-by-update.txt') 'x'
if (-not (Test-Path $uninstaller -PathType Leaf)) { throw "the uninstaller did not survive the folder replacement: $uninstaller" }
Invoke-Quiet $uninstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
# The uninstaller copies itself to a temporary folder and returns before it finishes.
$deadline = (Get-Date).AddSeconds(60)
while ((Test-Path $installDirectory) -or (Test-Path $entry) -or (Test-Path $uninstaller)) {
    if ((Get-Date) -gt $deadline) { throw "uninstall left $installDirectory, its entry or its uninstaller behind" }
    Start-Sleep -Milliseconds 500
}
if (Test-Path $backup) { throw "uninstall left the update backup $backup" }
if (Test-Path $shortcut) { throw "uninstall left the Start menu shortcut" }
if ($Layout -eq 'tauri' -and (Test-OnUserPath)) { throw "uninstall left $commandFolder on the user's PATH" }
Remove-Item $log -ErrorAction SilentlyContinue
Write-Output 'uninstalled: no folder, entry, backup or shortcut left'
