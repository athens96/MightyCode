# Mac stage: type-checks Rust for the Mac on this Windows PC, with no Mac (`cargo clippy --target aarch64-apple-darwin`). The build
# scripts that compile C or Objective-C for the Mac (objc2-exception-helper, bundled SQLite) get a stand-in compiler (fakecc.rs) that
# writes empty objects: a check links nothing. Linking, running and tests need the Mac runner (CI: native-tauri.yml, mac-spike.yml).
#
#   .\native\tauri\scripts\mac-check.ps1                          the workspace, all targets, -D warnings
#   .\native\tauri\scripts\mac-check.ps1 -Packages mighty-platform-macos,mightycode
#   .\native\tauri\scripts\mac-check.ps1 -Spikes                  native/tauri/spikes/mac (its own workspace, release)
#
# Needs `rustup target add aarch64-apple-darwin`. The target folder is CARGO_TARGET_DIR (else native/tauri/target).
param(
    [string[]]$Packages,
    [switch]$Spikes
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$tools = Join-Path $target 'mac-check'
New-Item -ItemType Directory -Force $tools | Out-Null
$cc = Join-Path $tools 'fakecc.exe'
if (-not (Test-Path $cc) -or (Get-Item $cc).LastWriteTimeUtc -lt (Get-Item (Join-Path $PSScriptRoot 'fakecc.rs')).LastWriteTimeUtc) {
    rustc -O -o $cc (Join-Path $PSScriptRoot 'fakecc.rs')
    if ($LASTEXITCODE -ne 0) { throw 'building the stand-in compiler failed' }
}
Copy-Item -Force $cc (Join-Path $tools 'fakear.exe')
$env:CC_aarch64_apple_darwin = $cc
$env:CXX_aarch64_apple_darwin = $cc
$env:AR_aarch64_apple_darwin = Join-Path $tools 'fakear.exe'
# The cc crate asks xcrun for the SDK unless SDKROOT names one; the stand-in ignores it.
$env:SDKROOT = $tools
# cargo reports progress on stderr, which Windows PowerShell would turn into a stopping error under 'Stop'.
$ErrorActionPreference = 'Continue'
if ($Spikes) {
    $env:CARGO_TARGET_DIR = Join-Path $target 'spikes'
    Push-Location (Join-Path $root 'spikes\mac')
    try { cargo clippy --workspace --release --target aarch64-apple-darwin } finally { Pop-Location }
} else {
    [string[]]$selection = if ($Packages) { $Packages | ForEach-Object { '-p', $_ } } else { @('--workspace') }
    Push-Location $root
    try { cargo clippy @selection --all-targets --target aarch64-apple-darwin -- -D warnings } finally { Pop-Location }
}
exit $LASTEXITCODE
