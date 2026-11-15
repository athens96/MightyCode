# The styles parity (W9-D, R9): every golden case of every style in styles/ (the repository manifests, the two bundled styles and the
# fixtures) projected by MightyCode.Core (tools/parity `styles project`) and by mighty-styles (`examples/parity.rs`), compared byte for
# byte; plus the conformance corpus's codes. Reads the frozen files in place and writes only under -Work. Exits 1 on any difference.
#
#   .\native\tauri\tools\parity\styles.ps1 [-Work <folder>]
param([string]$Work = (Join-Path $env:TEMP 'mighty-parity-styles'))
$ErrorActionPreference = 'Stop'
# A relative -Work is taken from where the script was called (the script moves to the workspace for cargo).
$Work = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Work)
$tauri = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$repo = (Resolve-Path (Join-Path $tauri '..\..')).Path
. (Join-Path $tauri 'scripts\env.ps1')
$csOut = Join-Path $tauri 'target\parity-cs'
dotnet build (Join-Path $PSScriptRoot 'Parity.csproj') -c Release -o $csOut --nologo -v quiet | Out-Host
if ($LASTEXITCODE -ne 0) { throw 'the C# parity tool did not build' }
Push-Location $tauri
try { cargo build -p mighty-styles --example parity --release --quiet; if ($LASTEXITCODE -ne 0) { throw 'the Rust parity tool did not build' } } finally { Pop-Location }
$rs = Join-Path $tauri 'target\release\examples\parity.exe'
$cs = Join-Path $csOut 'parity.dll'
if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null

$styles = Join-Path $repo 'styles'
$bundled = Join-Path $repo 'native\macos\Sources\MightyCore\Resources\Styles'
$vocabulary = '"vocabulary":{"acme.thing":["absent","open","complete"]}'
$checklist = '"checklist":"- [x] one\n- [x] two\n- [x] three\n- [ ] four\n- [ ] five\n- [ ] six\n- [ ] seven\n","events":[["subagent.start","plan review"],["tool.call","Read"],["subagent.start","task 1"]]'
$manifests = @()
Get-ChildItem $styles -Filter *.json | ForEach-Object { $manifests += ,@($_.FullName, 'user', '') }
Get-ChildItem $bundled -Filter *.json | ForEach-Object { $manifests += ,@($_.FullName, 'bundled', '') }
Get-ChildItem (Join-Path $styles 'fixtures') -Filter *.json | ForEach-Object { $manifests += ,@($_.FullName, 'user', $vocabulary) }
Get-ChildItem (Join-Path $styles 'conformance\valid'), (Join-Path $styles 'conformance\invalid') -Filter *.json | ForEach-Object { $manifests += ,@($_.FullName, 'user', '') }

$cases = @(
    '{}', '{"prompts":["/x"]}', '{"ready":false,"missing":["the thing"],"hint":"install it"}', '{"group":"near"}', '{"group":"far"}',
    "{$checklist}", "{$checklist,`"stage`":`"planning`"}", "{$checklist,`"stage`":`"awaitingApproval`",`"running`":true}", "{$checklist,`"stage`":`"executing`"}",
    '{"running":true}', '{"language":"en"}'
)
$failed = 0; $compared = 0
foreach ($m in $manifests) {
    $name = [IO.Path]::GetFileNameWithoutExtension($m[0])
    $first = $null
    try { $first = (Get-Content -Raw -Encoding utf8 $m[0] | ConvertFrom-Json) } catch { }
    $prompt = if ($first -and $first.actions) { ($first.actions[0].prompt -replace ' ?\{text\}', '') } else { '' }
    $index = 0
    foreach ($case in $cases) {
        $state = $case.Replace('"/x"', (ConvertTo-Json $prompt))
        $extra = @()
        if ($m[1] -ne 'user') { $extra += '"source":"' + $m[1] + '"' }
        if ($m[2]) { $extra += $m[2] }
        if ($extra.Count -gt 0) { $state = if ($state -eq '{}') { '{' + ($extra -join ',') + '}' } else { $state.TrimEnd('}') + ',' + ($extra -join ',') + '}' } }
        $statePath = Join-Path $Work "$name-$index.state.json"; [IO.File]::WriteAllText($statePath, $state)
        $a = Join-Path $Work "$name-$index.cs"; $b = Join-Path $Work "$name-$index.rs"
        dotnet $cs styles project $m[0] $statePath $a | Out-Null
        & $rs project $m[0] $statePath $b | Out-Null
        $compared++
        if ((Get-FileHash $a).Hash -ne (Get-FileHash $b).Hash) { $failed++; Write-Host "DIFFERS: $name case $index ($state)" }
        $index++
        if (-not $first -or -not $first.actions) { break }
    }
}
Write-Host "styles parity: $compared compared, $failed different"
if ($failed -gt 0) { exit 1 }
exit 0
