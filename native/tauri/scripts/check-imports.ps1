# Checks that a built MightyCode.exe imports no C runtime DLL (W9 plan fact 8, D7): no VCRUNTIME*.dll, MSVCP*.dll or
# api-ms-win-crt-*.dll, so it starts on a machine without the VC++ redistributable - the update helper too. .cargo/config.toml links the
# C runtime statically; src-tauri's test `the_exe_imports_no_c_runtime_dll` checks a test exe built the same way.
#
#   .\native\tauri\scripts\check-imports.ps1                      the debug build (target\debug\MightyCode.exe)
#   .\native\tauri\scripts\check-imports.ps1 -Exe <path>          any built exe (the release build, a packaged one)
#
# Reads the import table itself (no dumpbin needed). Exit code 0 when clean, 1 when a C runtime DLL is imported. Owner after W9 Step 0:
# W9-G (the package job calls it).
param(
    [string]$Exe
)
$ErrorActionPreference = 'Stop'
if (-not $Exe) {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot '..\target' }
    $Exe = Join-Path $target 'debug\MightyCode.exe'
}
if (-not (Test-Path $Exe -PathType Leaf)) { throw "no exe at $Exe" }

$image = [IO.File]::ReadAllBytes((Resolve-Path $Exe).Path)
function U16([int]$at) { [BitConverter]::ToUInt16($image, $at) }
function U32([int]$at) { [int][BitConverter]::ToUInt32($image, $at) }
$pe = U32 0x3C
if ([Text.Encoding]::ASCII.GetString($image, $pe, 4) -ne "PE`0`0") { throw "$Exe is not a PE image" }
$sections = U16 ($pe + 6)
$optional = $pe + 24
if ((U16 $optional) -ne 0x20B) { throw "$Exe is not a PE32+ image" }
$table = $optional + (U16 ($pe + 20))
function Offset([int]$rva) {
    for ($index = 0; $index -lt $sections; $index++) {
        $section = $table + $index * 40
        $size = [Math]::Max((U32 ($section + 8)), (U32 ($section + 16)))
        $address = U32 ($section + 12)
        if ($rva -ge $address -and $rva -lt $address + $size) { return $rva - $address + (U32 ($section + 20)) }
    }
    throw "RVA $rva lies in no section"
}
$dlls = @()
$descriptor = Offset (U32 ($optional + 120))
while ((U32 ($descriptor + 12)) -ne 0) {
    $name = Offset (U32 ($descriptor + 12))
    $end = [Array]::IndexOf($image, [byte]0, $name)
    $dlls += [Text.Encoding]::ASCII.GetString($image, $name, $end - $name).ToLowerInvariant()
    $descriptor += 20
}
$runtime = @($dlls | Where-Object { $_ -like 'vcruntime*' -or $_ -like 'msvcp*' -or $_ -like 'api-ms-win-crt-*' })
Write-Output "imports of ${Exe}: $($dlls -join ', ')"
if ($runtime.Count -gt 0) {
    Write-Output "C RUNTIME IMPORTED (the exe needs the VC++ redistributable): $($runtime -join ', ')"
    exit 1
}
Write-Output 'no C runtime DLL imported'
exit 0
