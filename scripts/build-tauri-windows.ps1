# Builds the Rust + Tauri app (native/tauri) for release on Windows in the 0.4.x shape (W9 plan §1.6, D10, D12, D13): the counterpart of
# build-windows.ps1, whose artifact names, zip layout and setup program it keeps, so an installed 0.4.x app updates itself to it.
#
#   .\scripts\build-tauri-windows.ps1 -Architecture x64|arm64 [-Installer] [-Version 0.5.0-rc.7] [-OutputDirectory <new folder>]
#
# The release stamp comes from the environment, as for build-windows.ps1: MIGHTY_UPDATE_PUBLIC_KEY (32 bytes of standard base64, the
# public half only) and MIGHTY_UPDATE_URL (https). Both are required: this script makes release builds (MIGHTY_RELEASE=1, which build.rs
# refuses without them). It never reads a signing key.
#
# The version is src-tauri/Cargo.toml's, which must equal VERSION (scripts/release.py bumps both). -Version builds a candidate instead
# (CI, the update rehearsal): it must be newer than VERSION, so a 0.4.x install takes it as an update; Cargo.toml and Cargo.lock carry it
# for the build only and are put back afterwards. The setup program and the uninstall entry get its major.minor.patch, as the helper's
# DisplayVersion does (C#'s Version.ToString(3)).
#
# Steps: the version check; npm ci (the page, the Tauri CLI); `tauri build --target <triple>` (release, the C runtime static through
# native/tauri/.cargo/config.toml, so RUSTFLAGS must be unset); the exe's PE machine; the staging folder (MightyCode.exe, claude-mods,
# Agents, pets, LICENSE.txt, build-info.json) and its required files, exactly one MightyCode.exe and no link in it; check-imports.ps1 (no C
# runtime DLL); release\MightyCode-windows-<arch>.zip with the exe at its root, at most 512 MiB, and its .sha256; with -Installer the setup
# program from native/windows/installer/MightyCode.iss (only /DSourceDir differs from C#'s) and its .sha256; then the 0.4.x stager itself
# (C#'s AppUpdateService.Stage through tools/parity `update stage`) over the zip: accepted for this architecture with the exe at the
# staged root, refused for the other one ("architecture"). Needs Node, the Rust toolchain with the MSVC linker (scripts/env.ps1 on the
# development PC), the .NET 10 SDK (the stager) and, with -Installer, Inno Setup 6 (ISCC.exe, or MIGHTY_ISCC). Windows PowerShell 5.1 or 7.
param(
    [ValidateSet('x64', 'arm64')][string]$Architecture = 'x64',
    [string]$OutputDirectory,
    [string]$Version,
    [switch]$Installer
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tauri = Join-Path $root 'native\tauri'
$cargoToml = Join-Path $tauri 'src-tauri\Cargo.toml'
$cargoLock = Join-Path $tauri 'Cargo.lock'
$triple = if ($Architecture -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$machine = if ($Architecture -eq 'arm64') { 0xAA64 } else { 0x8664 }
$utf8 = New-Object Text.UTF8Encoding($false)
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem

# The flags in .cargo/config.toml (+crt-static) are replaced, not extended, by RUSTFLAGS: a build with it set would import the C runtime.
if ($env:RUSTFLAGS -or $env:CARGO_ENCODED_RUSTFLAGS) { throw 'RUSTFLAGS (or CARGO_ENCODED_RUSTFLAGS) is set; it would replace native/tauri/.cargo/config.toml''s +crt-static. Unset it.' }

# --- the version
$cargoText = [IO.File]::ReadAllText($cargoToml)
$cargoVersion = [regex]::Match($cargoText, '(?m)^version = "([^"]+)"').Groups[1].Value
$repositoryVersion = ([IO.File]::ReadAllText((Join-Path $root 'VERSION'))).Trim()
if ($repositoryVersion -notmatch '^\d+\.\d+\.\d+$') { throw "VERSION must be x.y.z; got '$repositoryVersion'" }
if ($Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.\-]+)?$') { throw "-Version must be x.y.z or x.y.z-pre (the app's version rule without +build); got '$Version'" }
    if ([version]($Version -split '-')[0] -le [version]$repositoryVersion) { throw "-Version $Version is not newer than VERSION ${repositoryVersion}: a 0.4.x install would not take it as an update" }
    $appVersion = $Version
} else {
    if ($cargoVersion -ne $repositoryVersion) { throw "src-tauri/Cargo.toml's version $cargoVersion is not VERSION $repositoryVersion (scripts/release.py bumps both; -Version builds a candidate)" }
    $appVersion = $cargoVersion
}
$numericVersion = ($appVersion -split '-')[0]

# --- the release stamp (build.rs checks it again)
$updateKey = "$env:MIGHTY_UPDATE_PUBLIC_KEY".Trim()
$updateUrl = "$env:MIGHTY_UPDATE_URL".Trim()
if (-not $updateKey -or -not $updateUrl) { throw 'MIGHTY_UPDATE_PUBLIC_KEY and MIGHTY_UPDATE_URL are required: this is a release build (MIGHTY_RELEASE=1)' }
try { $keyBytes = [Convert]::FromBase64String($updateKey) } catch { throw 'MIGHTY_UPDATE_PUBLIC_KEY is not base64' }
if ($keyBytes.Length -ne 32) { throw "MIGHTY_UPDATE_PUBLIC_KEY must be a raw 32-byte Ed25519 public key ($($keyBytes.Length) bytes)" }
if ($updateUrl -notmatch '^https://[^/?#]+') { throw 'MIGHTY_UPDATE_URL must be an https address with a host' }

# --- where it goes: a new or empty staging folder, never an old zip or setup overwritten
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root "release\tauri-windows-$Architecture" }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path $OutputDirectory) {
    if (-not (Test-Path $OutputDirectory -PathType Container) -or (Get-ChildItem -Force $OutputDirectory | Select-Object -First 1)) {
        throw "give a new or empty folder for the build: $OutputDirectory"
    }
}
$releaseDirectory = Split-Path -Parent $OutputDirectory
$archive = Join-Path $releaseDirectory "MightyCode-windows-$Architecture.zip"
$setup = Join-Path $releaseDirectory "MightyCode-windows-$Architecture-setup.exe"
if (Test-Path $archive) { throw "an earlier zip is never overwritten: $archive" }
if ($Installer -and (Test-Path $setup)) { throw "an earlier setup program is never overwritten: $setup" }
$iscc = $null
if ($Installer) {
    $iscc = @($env:MIGHTY_ISCC, (Get-Command ISCC.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source),
        [IO.Path]::Combine("${env:ProgramFiles(x86)}", 'Inno Setup 6', 'ISCC.exe'), [IO.Path]::Combine("$env:ProgramFiles", 'Inno Setup 6', 'ISCC.exe'),
        [IO.Path]::Combine("$env:LOCALAPPDATA", 'Programs', 'Inno Setup 6', 'ISCC.exe')) | Where-Object { $_ -and (Test-Path $_ -PathType Leaf) } | Select-Object -First 1
    if (-not $iscc) { throw 'Inno Setup 6 (ISCC.exe) was not found; install it or name it in MIGHTY_ISCC' }
}

function Invoke-Checked([string]$what, [scriptblock]$command) {
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

# The PE machine of an image (IMAGE_FILE_HEADER.Machine), as the 0.4.x stager reads it.
function Get-Machine([string]$path) {
    $stream = [IO.File]::OpenRead($path)
    try {
        $reader = New-Object IO.BinaryReader($stream)
        $stream.Position = 0x3C
        $stream.Position = $reader.ReadInt32()
        if ($reader.ReadUInt32() -ne 0x4550) { throw "$path is not a PE image" }
        [int]$reader.ReadUInt16()
    } finally { $stream.Dispose() }
}

$saved = @{}
foreach ($name in 'MIGHTY_RELEASE', 'MIGHTY_UPDATE_PUBLIC_KEY', 'MIGHTY_UPDATE_URL') { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$cargoTomlBytes = [IO.File]::ReadAllBytes($cargoToml)
$cargoLockBytes = [IO.File]::ReadAllBytes($cargoLock)
Push-Location $root
try {
    $env:MIGHTY_RELEASE = '1'
    $env:MIGHTY_UPDATE_PUBLIC_KEY = $updateKey
    $env:MIGHTY_UPDATE_URL = $updateUrl
    if ($Version -and $Version -ne $cargoVersion) {
        # A candidate's version, for this build only (put back in `finally`): the exe's own version, the helper's DisplayVersion.
        [IO.File]::WriteAllText($cargoToml, ([regex]'(?m)^version = "[^"]+"').Replace($cargoText, "version = `"$Version`"", 1), $utf8)
        $lockText = [IO.File]::ReadAllText($cargoLock)
        [IO.File]::WriteAllText($cargoLock, ([regex]'(?m)^(name = "mightycode"\r?\nversion = )"[^"]+"').Replace($lockText, "`${1}`"$Version`"", 1), $utf8)
    }
    Write-Output "Mighty Code $appVersion ($Architecture, $triple): release build"

    Invoke-Checked 'npm ci (page)' { npm ci --no-audit --no-fund --prefix (Join-Path $root 'native\windows\web') }
    Invoke-Checked 'npm ci (Tauri CLI)' { npm ci --no-audit --no-fund --prefix $tauri }
    Push-Location $tauri
    try { Invoke-Checked 'tauri build' { npm run tauri build -- --target $triple } }
    finally { Pop-Location }

    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $tauri 'target' }
    if (-not [IO.Path]::IsPathRooted($target)) { $target = Join-Path (Join-Path $tauri 'src-tauri') $target }
    $built = [IO.Path]::Combine($target, $triple, 'release', 'MightyCode.exe')
    if (-not (Test-Path $built -PathType Leaf)) { throw "the build left no exe at $built" }
    $found = Get-Machine $built
    if ($found -ne $machine) { throw ("the exe's PE machine is 0x{0:X4}, not 0x{1:X4} ({2})" -f $found, $machine, $Architecture) }

    # --- the staging folder: what runs.rs, companion.rs and C#'s build put beside the exe
    New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
    Copy-Item -LiteralPath $built -Destination (Join-Path $OutputDirectory 'MightyCode.exe')
    # The mighty-bridge plugin as claude-mods (C#'s csproj: mods/mighty-bridge/** without tests/**), dot folders included.
    $bridge = Join-Path $root 'mods\mighty-bridge'
    $mods = Join-Path $OutputDirectory 'claude-mods'
    New-Item -ItemType Directory -Force $mods | Out-Null
    Get-ChildItem -LiteralPath $bridge -Force | Where-Object { $_.Name -ne 'tests' } | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $mods -Recurse }
    # The agents' files (C#: the Mac bundle's Resources/Agents, flat).
    $agents = Join-Path $OutputDirectory 'Agents'
    New-Item -ItemType Directory -Force $agents | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $root 'native\macos\Sources\MightyCore\Resources\Agents') -File | Copy-Item -Destination $agents
    # The bundled pets (tauri.conf.json's resource, C#'s Assets\pets).
    Copy-Item -LiteralPath (Join-Path $root 'assets\pets') -Destination (Join-Path $OutputDirectory 'pets') -Recurse
    # The app's icon where C#'s layout keeps it (Assets\MightyCode.ico): the toast registration names it as its IconUri (activator.rs).
    New-Item -ItemType Directory -Force (Join-Path $OutputDirectory 'Assets') | Out-Null
    Copy-Item -LiteralPath (Join-Path $root 'assets\icons\MightyCode.ico') -Destination (Join-Path $OutputDirectory 'Assets\MightyCode.ico')
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $OutputDirectory 'LICENSE.txt')
    $rustc = (& rustc --version) -join ' '
    $info = [ordered]@{
        architecture = $Architecture; version = $appVersion; engine = 'tauri'; rustc = $rustc; sourceCommit = $env:GITHUB_SHA
        deployment = 'unpackaged-folder'; signed = $false
        # Whether this build can check for updates at all (Windows rule 1): a release build always can.
        updateChecks = $true; updateManifestUrl = $updateUrl
    }
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'build-info.json'), ($info | ConvertTo-Json) + "`n", $utf8)

    foreach ($required in @('MightyCode.exe', 'LICENSE.txt', 'build-info.json', 'claude-mods\.claude-plugin\plugin.json', 'Agents\pi-ask-user.js',
            'Agents\opencode-questions.md', 'pets\mighty-raccoon\pet.json', 'pets\mighty-raccoon\spritesheet.webp', 'Assets\MightyCode.ico')) {
        if (-not (Test-Path (Join-Path $OutputDirectory $required) -PathType Leaf)) { throw "the package lacks $required" }
    }
    # What the 0.4.x stager refuses (AppUpdateService.Stage): a second MightyCode.exe anywhere, or a link.
    $everything = @(Get-ChildItem -LiteralPath $OutputDirectory -Recurse -Force)
    $exes = @($everything | Where-Object { -not $_.PSIsContainer -and $_.Name -ieq 'MightyCode.exe' })
    if ($exes.Count -ne 1) { throw "the package holds $($exes.Count) files named MightyCode.exe: $($exes.FullName -join ', ')" }
    $links = @($everything | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint })
    if ($links.Count -gt 0) { throw "the package holds links: $($links.FullName -join ', ')" }

    Invoke-Checked 'check-imports.ps1' { & (Join-Path $tauri 'scripts\check-imports.ps1') -Exe (Join-Path $OutputDirectory 'MightyCode.exe') }

    # --- the zip the app's own update downloads: the exe at its root, every file an entry named with '/' (whatever .NET the shell runs
    # on; Windows PowerShell's ZipFile.CreateFromDirectory may write '\'), deflated.
    $zip = [IO.Compression.ZipFile]::Open($archive, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($file in Get-ChildItem -LiteralPath $OutputDirectory -Recurse -File -Force | Sort-Object FullName) {
            $entry = $file.FullName.Substring($OutputDirectory.Length + 1).Replace('\', '/')
            [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $file.FullName, $entry, [IO.Compression.CompressionLevel]::Optimal)
        }
    } finally { $zip.Dispose() }
    $maxPackageBytes = 280MB
    $size = (Get-Item -LiteralPath $archive).Length
    if ($size -gt $maxPackageBytes) { throw "the zip is $size bytes; release packages must stay within 280 MiB (20 MiB headroom below the 300 MiB R2 upload limit and 512 MiB updater cap)" }
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$archive.sha256", "$hash  $([IO.Path]::GetFileName($archive))`n", [Text.Encoding]::ASCII)
    Write-Output "package: $archive ($size bytes)"

    if ($Installer) {
        # The same .iss as C#'s (AppId, exe name, install folder): only the folder it packs differs.
        Invoke-Checked 'ISCC' { & $iscc /Q "/DAppVersion=$numericVersion" "/DArch=$Architecture" "/DSourceDir=$OutputDirectory" "/DOutputDir=$releaseDirectory" (Join-Path $root 'native\windows\installer\MightyCode.iss') }
        if (-not (Test-Path $setup -PathType Leaf)) { throw "ISCC left no setup program at $setup" }
        $setupSize = (Get-Item -LiteralPath $setup).Length
        if ($setupSize -gt $maxPackageBytes) { throw "the setup program is $setupSize bytes; release packages must stay within 280 MiB (20 MiB headroom below the 300 MiB R2 upload limit)" }
        $setupHash = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant()
        [IO.File]::WriteAllText("$setup.sha256", "$setupHash  $([IO.Path]::GetFileName($setup))`n", [Text.Encoding]::ASCII)
        Write-Output "setup program: $setup ($setupSize bytes)"
    }

    # --- the 0.4.x stager's verdict on the zip (plan fact 3), from C#'s own code
    $work = Join-Path ([IO.Path]::GetTempPath()) ('mighty-stage-check-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory $work | Out-Null
    try {
        $other = if ($Architecture -eq 'arm64') { 'x64' } else { 'arm64' }
        $cases = @{ cases = @(@{ name = 'this'; zip = $archive; arch = $Architecture }, @{ name = 'other'; zip = $archive; arch = $other }) }
        [IO.File]::WriteAllText((Join-Path $work 'cases.json'), ($cases | ConvertTo-Json -Depth 5), $utf8)
        $parity = Join-Path $work 'parity'
        Invoke-Checked 'the C# parity tool build' { dotnet build (Join-Path $tauri 'tools\parity\Parity.csproj') -c Release -o $parity --nologo -v quiet }
        Invoke-Checked 'update stage' { dotnet (Join-Path $parity 'parity.dll') update stage (Join-Path $work 'cases.json') (Join-Path $work 'verdicts.json') }
        $verdicts = ([IO.File]::ReadAllText((Join-Path $work 'verdicts.json')) | ConvertFrom-Json).cases
        $accepted = $verdicts[0].csharp
        $staged = @(Get-ChildItem -LiteralPath $OutputDirectory -Recurse -File).Count
        if ($accepted.verdict -ne 'ok' -or $accepted.folder -ne '.' -or @($accepted.files).Count -ne $staged -or @($accepted.files) -notcontains 'MightyCode.exe') {
            throw "the 0.4.x stager did not accept the package as built: $($accepted | ConvertTo-Json -Compress -Depth 5)"
        }
        $refused = $verdicts[1].csharp
        if ($refused.verdict -ne 'key' -or $refused.key -ne 'appUpdate.error.architecture') {
            throw "the 0.4.x stager did not refuse the package for $other with its architecture sentence: $($refused | ConvertTo-Json -Compress -Depth 5)"
        }
        Write-Output "0.4.x stager: accepted for $Architecture ($staged files, the exe at the staged root), refused for $other"
    } finally { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
} finally {
    Pop-Location
    [IO.File]::WriteAllBytes($cargoToml, $cargoTomlBytes)
    [IO.File]::WriteAllBytes($cargoLock, $cargoLockBytes)
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
