# Compares the Agent IO protocol bytes of the C# app (MightyCode.Core) and the Rust port (mighty-delegation), W7-C:
#   1. the binding's argv (Claude's --mcp-config, Codex's -c lists) for every agent, mode and switch, two executables (one with a
#      space, non-ASCII and the characters Wire.Json escapes);
#   2. the answers of mighty-terminal and mighty-delegation to one JSON-RPC transcript each (tools/parity/agent-io/*.jsonl: initialize
#      at every version, ping, tools/list, bad ids, an oversize line, notifications, unknown and malformed tool names, non-string
#      arguments, lone surrogates, the no-connection ToolError), line by line;
#   3. the stdout of the Rust exe's `--agent-io-mcp` and `--agent-delegation-mcp` (no pipe in the environment) fed the same
#      transcripts, as the CLI feeds them: every answer line, in any order, the same as C#'s.
# Exits 1 on any difference.
#
#   .\native\tauri\tools\parity\mcp.ps1 [-Work <folder>] [-Exe <the Rust exe>]
#
# Needs the .NET 10 SDK and the Rust toolchain (scripts/env.ps1). -Exe defaults to the checks' relaunched exe (mighty-delegation-fake),
# which serves the modes through the same library calls as MightyCode.exe's main.rs.
param(
    [string]$Work = (Join-Path $env:TEMP 'mighty-parity-mcp'),
    [string]$Exe = ''
)
$ErrorActionPreference = 'Stop'
$tauri = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $tauri 'scripts\env.ps1')
$corpus = Join-Path $PSScriptRoot 'agent-io'

$csOut = Join-Path $tauri 'target\parity-cs'
Write-Host "building the C# tool -> $csOut"
dotnet build (Join-Path $PSScriptRoot 'Parity.csproj') -c Release -o $csOut --nologo -v quiet | Out-Host
if ($LASTEXITCODE -ne 0) { throw 'the C# parity tool did not build' }
Push-Location $tauri
try {
    cargo build -p mighty-delegation --example agent_io_parity --bin mighty-delegation-fake --release --quiet
    if ($LASTEXITCODE -ne 0) { throw 'the Rust parity tool did not build' }
} finally { Pop-Location }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $tauri 'target' }
$rs = Join-Path $target 'release\examples\agent_io_parity.exe'
if (-not $Exe) { $Exe = Join-Path $target 'release\mighty-delegation-fake.exe' }

if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
$csDir = Join-Path $Work 'cs'; $rsDir = Join-Path $Work 'rs'
dotnet (Join-Path $csOut 'parity.dll') agent-io $corpus $csDir
if ($LASTEXITCODE -ne 0) { throw 'the C# tool failed' }
& $rs $corpus $rsDir
if ($LASTEXITCODE -ne 0) { throw 'the Rust tool failed' }

$failed = $false
foreach ($name in 'bindings.txt', 'terminal.txt', 'delegation.txt') {
    $a = [IO.File]::ReadAllBytes((Join-Path $csDir $name)); $b = [IO.File]::ReadAllBytes((Join-Path $rsDir $name))
    if ([Convert]::ToBase64String($a) -ne [Convert]::ToBase64String($b)) {
        $failed = $true
        $csLines = [IO.File]::ReadAllText((Join-Path $csDir $name)) -split "`n"; $rsLines = [IO.File]::ReadAllText((Join-Path $rsDir $name)) -split "`n"
        for ($i = 0; $i -lt [Math]::Max($csLines.Count, $rsLines.Count); $i++) {
            if ($csLines[$i] -ne $rsLines[$i]) { Write-Host "DIFF $name line $($i + 1)`n  C#:   $($csLines[$i])`n  Rust: $($rsLines[$i])" }
        }
    } else { Write-Host "same: $name ($($a.Length) bytes)" }
}

# The exe's stdout over the transcript (the oversize line spelled out), with no pipe or token in its environment.
$utf8 = New-Object Text.UTF8Encoding($false)
$oversize = '{"jsonrpc":"2.0","id":1,"method":"ping","x":"' + ('x' * 1048576) + '"}'
foreach ($pair in @(@('terminal', '--agent-io-mcp'), @('delegation', '--agent-delegation-mcp'))) {
    $name = $pair[0]
    $lines = [IO.File]::ReadAllText((Join-Path $corpus "$name.jsonl"), $utf8) -split "`n" | Where-Object { $_.Length -gt 0 } | ForEach-Object { if ($_ -eq '@oversize') { $oversize } else { $_ } }
    $stdin = Join-Path $Work "$name.stdin"
    [IO.File]::WriteAllText($stdin, (($lines -join "`n") + "`n"), $utf8)
    $info = New-Object Diagnostics.ProcessStartInfo $Exe, $pair[1]
    $info.UseShellExecute = $false; $info.RedirectStandardInput = $true; $info.RedirectStandardOutput = $true
    $info.StandardOutputEncoding = $utf8
    foreach ($key in 'MIGHTY_AGENT_IO_PIPE', 'MIGHTY_PANE_TOKEN') { [void]$info.EnvironmentVariables.Remove($key) }
    $process = [Diagnostics.Process]::Start($info)
    $reading = $process.StandardOutput.ReadToEndAsync()
    $bytes = [IO.File]::ReadAllBytes($stdin)
    $process.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length); $process.StandardInput.Close()
    $out = $reading.GetAwaiter().GetResult(); $process.WaitForExit()
    $got = @($out -split "`r`n" | Where-Object { $_.Length -gt 0 } | Sort-Object -CaseSensitive)
    $want = @([IO.File]::ReadAllText((Join-Path $csDir "$name.txt"), $utf8) -split "`n" | Where-Object { $_.Length -gt 0 -and $_ -ne '<none>' } | Sort-Object -CaseSensitive)
    $same = ($process.ExitCode -eq 0) -and ($got.Count -eq $want.Count) -and (-not (Compare-Object $got $want -CaseSensitive -SyncWindow 0))
    if ($same) { Write-Host "same: the exe's $($pair[1]) stdout ($($got.Count) lines, CRLF-ended, exit 0)" }
    else { $failed = $true; Write-Host "DIFF the exe's $($pair[1]) stdout: exit $($process.ExitCode), $($got.Count) lines against $($want.Count)" }
}
if ($failed) { exit 1 }
Write-Host 'Agent IO parity: no difference'
