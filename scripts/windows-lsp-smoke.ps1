param([string]$Executable="$PSScriptRoot/../target/debug/filavim.exe")
$ErrorActionPreference='Stop'
$executablePath=(Resolve-Path -LiteralPath $Executable).Path
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../target/floating-lsp-fixture'))
New-Item -ItemType Directory -Force -Path (Join-Path $root 'src') | Out-Null
[IO.File]::WriteAllText((Join-Path $root 'Cargo.toml'), "[package]`nname=`"filavim_popup_fixture`"`nversion=`"0.1.0`"`nedition=`"2024`"`n[workspace]`n",[Text.UTF8Encoding]::new($false))
$source=@'
/// Widget documentation with Unicode é😀.
struct Widget { value: i32 }
impl Widget {
    /// Computes the widget value.
    fn compute(&self) -> i32 { self.value }
}
fn main() {
    let widget = Widget { value: 7 };
    let _answer = widget.com;
    let _wrong = ;
}
'@
$file=Join-Path $root 'src/main.rs'
[IO.File]::WriteAllText($file,$source,[Text.UTF8Encoding]::new($false))
$before=(Get-FileHash -LiteralPath $file).Hash
$report=Join-Path $root 'native-lsp-report.txt'
$previousOffline=$env:CARGO_NET_OFFLINE
try {
    $env:CARGO_NET_OFFLINE='true'
    $process=Start-Process -FilePath $executablePath -ArgumentList @('--lsp-smoke-test',('"'+$report+'"'),('"'+$file+'"')) -WindowStyle Normal -RedirectStandardError (Join-Path $root 'stderr.txt') -PassThru
    if(!$process.WaitForExit(80000)) { & taskkill /PID $process.Id /T /F | Out-Null; throw 'Real LSP smoke exceeded its bounded deadline' }
    $process.Refresh()
    Get-Content -LiteralPath $report
    if($process.ExitCode -ne 0) {throw "Real LSP smoke failed, exit $($process.ExitCode); inspect $report"}
    if((Get-FileHash -LiteralPath $file).Hash -ne $before){throw 'LSP smoke modified the disk fixture'}
    'Disk fixture unchanged.'
} finally { $env:CARGO_NET_OFFLINE=$previousOffline }
