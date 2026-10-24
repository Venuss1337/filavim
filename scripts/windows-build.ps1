param([switch]$Release)
$ErrorActionPreference = 'Stop'
$previousFxc = $env:GPUI_FXC_PATH
Push-Location (Join-Path $PSScriptRoot '..')
try {
    if ($Release) {
        $candidates = @($previousFxc, (Join-Path $PWD '.local/tooling/fxc/fxc.exe'))
        $candidates += @(Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\fxc.exe" -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | ForEach-Object FullName)
        $compiler = $candidates | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } | Select-Object -First 1
        if (!$compiler) { throw 'GPUI release builds need Windows SDK fxc.exe. Set GPUI_FXC_PATH or install the Windows SDK. See GPUI_CHECKPOINT1.md.' }
        $env:GPUI_FXC_PATH = (Resolve-Path -LiteralPath $compiler).Path
        cargo build --release --locked
    } else {
        cargo build --locked
    }
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed ($LASTEXITCODE)" }
} finally {
    $env:GPUI_FXC_PATH = $previousFxc
    Pop-Location
}
