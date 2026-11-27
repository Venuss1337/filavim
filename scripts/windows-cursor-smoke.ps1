param([string]$Executable="$PSScriptRoot/../target/release/filavim.exe")
$ErrorActionPreference='Stop'
$exe=(Resolve-Path -LiteralPath $Executable).Path
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../target/checkpoint4-fixture'))
New-Item -ItemType Directory -Force -Path $root | Out-Null
$file=Join-Path $root 'cursor-fixture.rs'
$source="fn main() {`n`tlet café = 1; // 😀 e$([char]0x301)`n// " + ('long ' * 1000) + "`n" + ((1..200 | ForEach-Object { "`t// cursor movement fixture line $_" }) -join "`n") + "`n}`n"
[IO.File]::WriteAllText($file,$source,[Text.UTF8Encoding]::new($false))
$before=(Get-FileHash -LiteralPath $file).Hash
$previous=$env:FILAVIM_CURSOR_SMOKE
$results=@()
try {
    foreach($mode in @('off','smooth','all')) {
        $env:FILAVIM_CURSOR_SMOKE=$mode
        $report=Join-Path $root "$mode-report.txt"
        [IO.File]::WriteAllText($report,'')
        $timer=[Diagnostics.Stopwatch]::StartNew()
        $process=Start-Process -FilePath $exe -ArgumentList @('--smoke-test',('"'+$report+'"'),('"'+$file+'"')) -WindowStyle Normal -PassThru
        try {
            $windowMs=$null
            do {
                Start-Sleep -Milliseconds 25
                $process.Refresh()
                if($null -eq $windowMs -and $process.MainWindowHandle -ne 0){$windowMs=$timer.Elapsed.TotalMilliseconds}
                $text=[IO.File]::ReadAllText($report)
            } while(!$text.StartsWith('cursor_idle_begin') -and !$process.HasExited -and $timer.Elapsed.TotalSeconds -lt 20)
            if(!$text.StartsWith('cursor_idle_begin')){throw "Missing idle marker for $mode"}
            $cpu=$process.TotalProcessorTime.TotalMilliseconds
            $resident=$process.WorkingSet64/1MB
            $idle=[Diagnostics.Stopwatch]::StartNew()
            if(!$process.WaitForExit(6000)){throw "Idle shutdown timeout for $mode"}
            $process.Refresh()
            $text=[IO.File]::ReadAllText($report)
            if($process.ExitCode -ne 0){throw "Smoke failed for $mode : $text"}
            if($text -notmatch 'cursor_active=false'){throw "Cursor failed to settle for $mode"}
            $results += [pscustomobject]@{
                mode=$mode; window_ms=[Math]::Round($windowMs,2)
                resident_mib=[Math]::Round($resident,2)
                idle_cpu_ms=$process.TotalProcessorTime.TotalMilliseconds-$cpu
                idle_wall_ms=[Math]::Round($idle.Elapsed.TotalMilliseconds,2)
                total_process_cpu_ms=$process.TotalProcessorTime.TotalMilliseconds
                report=$text
            }
        } finally {
            if(!$process.HasExited){Stop-Process -Id $process.Id}
            $process.Dispose()
        }
    }
    if((Get-FileHash -LiteralPath $file).Hash -ne $before){throw 'Source fixture was modified'}
    $json=$results | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText((Join-Path $root 'comparison.json'),$json,[Text.UTF8Encoding]::new($false))
    $json
} finally {$env:FILAVIM_CURSOR_SMOKE=$previous}
