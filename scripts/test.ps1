#requires -Version 7.2
param(
    [ValidateSet('Fast','Full','LSP','GUI','Performance')][string]$Mode = 'Fast',
    [switch]$CoreOnly,
    [switch]$Interactive,
    [string]$Executable,
    [ValidateRange(1,8)][int]$Jobs = 2,
    [ValidateRange(1,8)][int]$Threads = 2,
    [ValidateRange(1,512)][int]$Cases = 64,
    [UInt64]$Seed = 4045029414,
    [ValidateRange(30,3600)][int]$TimeoutSeconds = 1200
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'test-report.ps1')
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$run = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff') + '-' + $Mode.ToLowerInvariant()
$artifacts = Join-Path $root "target/test-reports/$run"
New-Item -ItemType Directory -Force -Path $artifacts | Out-Null
$suiteClock = [Diagnostics.Stopwatch]::StartNew()
$stages = [Collections.Generic.List[object]]::new()
$report = [ordered]@{
    mode=$Mode; status='running'; timestamp_utc=[DateTime]::UtcNow.ToString('o')
    jobs=$Jobs; test_threads=$Threads; property_seed=$Seed; property_cases=$Cases
    host=[Environment]::OSVersion.ToString(); logical_processors=[Environment]::ProcessorCount
    architecture=[Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString(); stages=$stages; not_executed=@(); metrics=@()
}
$environmentNames = @('CARGO_BUILD_JOBS','RUST_TEST_THREADS','FILAVIM_PROPTEST_SEED','FILAVIM_PROPTEST_CASES','FILAVIM_GPUI_CONFIG','FILAVIM_LSP_HOME')
$previous = @{}
foreach ($name in $environmentNames) { $previous[$name] = [Environment]::GetEnvironmentVariable($name) }
function Stage([string]$Name, [string]$Program, [string[]]$Arguments, [int]$Timeout = $TimeoutSeconds, [switch]$Visible) {
    $out = Join-Path $artifacts "$Name.stdout.log"
    $err = Join-Path $artifacts "$Name.stderr.log"
    Write-Host "[$Mode] $Name"
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $entry = [ordered]@{ name=$Name; status='failed'; command=($Program + ' ' + ($Arguments -join ' ')); stdout=$out; stderr=$err; passed=0; failed=0; ignored=0; ignored_tests=@() }
    $process = $null
    try {
        # ArgumentList is a string on Windows; quote paths without invoking a shell.
        $quoted = @($Arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + $_.Replace('"','\"') + '"' } else { $_ } })
        $options = @{ FilePath=$Program; ArgumentList=$quoted; WorkingDirectory=$root; PassThru=$true; RedirectStandardOutput=$out; RedirectStandardError=$err }
        if ($IsWindows) { $options.WindowStyle = if ($Visible) { 'Normal' } else { 'Hidden' } }
        $process = Start-Process @options
        if (!$process.WaitForExit($Timeout * 1000)) {
            $process.Kill($true)
            $process.WaitForExit()
            $entry.status = 'timed_out'
            throw "$Name exceeded ${Timeout}s; only this owned process tree was stopped"
        }
        $entry.exit_code = $process.ExitCode
        $text = (Get-Content -LiteralPath $out -Raw -ErrorAction SilentlyContinue) + "`n" + (Get-Content -LiteralPath $err -Raw -ErrorAction SilentlyContinue)
        Write-Host $text
        $summary = Get-CargoTestSummary $text
        $entry.passed = $summary.passed; $entry.failed = $summary.failed
        $entry.ignored = $summary.ignored; $entry.ignored_tests = $summary.ignored_tests
        try { $entry.driver_peak_working_set_bytes = $process.PeakWorkingSet64 } catch { $entry.driver_peak_working_set_bytes = $null }
        if ($entry.exit_code -ne 0) { throw "$Name failed (exit $($entry.exit_code)); see $err" }
        if ($Arguments.Count -gt 0 -and $Arguments[0] -eq 'test') { Assert-CargoTestsExecuted $summary }
        $measurements = @([regex]::Matches($text, '(?m)FILAVIM_METRICS (\{[^\r\n]+\})') | ForEach-Object { $_.Groups[1].Value | ConvertFrom-Json })
        if ($measurements.Count) { $report.metrics += $measurements }
        $entry.status = 'passed'
        return $text
    } catch {
        $entry.error = $_.Exception.Message
        throw
    } finally {
        $entry.elapsed_seconds = [Math]::Round($clock.Elapsed.TotalSeconds,3)
        $stages.Add([pscustomobject]$entry)
        if ($process) { $process.Dispose() }
    }
}
Push-Location $root
$exitCode = 0
try {
    $env:CARGO_BUILD_JOBS = "$Jobs"; $env:RUST_TEST_THREADS = "$Threads"
    $env:FILAVIM_PROPTEST_SEED = "$Seed"; $env:FILAVIM_PROPTEST_CASES = "$Cases"
    # Every mode is independent of user preferences/tool selections.
    $env:FILAVIM_GPUI_CONFIG = Join-Path $artifacts 'config.toml'
    $env:FILAVIM_LSP_HOME = Join-Path $artifacts 'lsp'
    [IO.File]::WriteAllText($env:FILAVIM_GPUI_CONFIG, "[lsp]`nenabled=false`n", [Text.UTF8Encoding]::new($false))
    $cargo = (Get-Command cargo -ErrorAction Stop).Source
    $report.toolchain = (& rustc --version) -join ' '
    if ($Mode -in @('Fast','Full')) {
        $pwsh = (Get-Process -Id $PID).Path
        $null = Stage 'report-parser' $pwsh @('-NoProfile','-File',(Join-Path $PSScriptRoot 'tests/test-report.ps1'))
        $null = Stage 'format' $cargo @('fmt','--check')
        if ($Mode -eq 'Fast') {
            $null = Stage 'clippy-core' $cargo @('clippy','--locked','--no-default-features','--all-targets','--','-D','warnings')
            $null = Stage 'core' $cargo @('test','--locked','--no-default-features','--lib','--test','editor','--test','file_safety','--','--test-threads',"$Threads")
            $report.not_executed = @('property sequences (use Full)','GPUI integration (use Full)','native Windows smoke (use GUI -Interactive)','explicit benchmarks (use Performance)','optional external LSP/install checks')
        } else {
            $features = if ($CoreOnly) { @('--no-default-features','--features','test-tools') } else { @('--features','gpui-tests,perf') }
            $null = Stage 'clippy' $cargo (@('clippy','--locked','--all-targets') + $features + @('--','-D','warnings'))
            $null = Stage 'correctness' $cargo (@('test','--locked','--lib','--tests') + $features + @('--','--test-threads',"$Threads"))
            $report.not_executed = @('native Windows smoke (use GUI -Interactive)','explicit benchmarks (use Performance)','optional external LSP/install checks')
            if ($CoreOnly) { $report.not_executed += 'GPUI integration (CoreOnly explicitly selected)' }
        }
    } elseif ($Mode -eq 'LSP') {
        $null = Stage 'protocol' $cargo @('test','--locked','--no-default-features','--features','test-tools','--lib','lsp::','--','--test-threads',"$Threads")
        $null = Stage 'document-protocol-integration' $cargo @('test','--locked','--no-default-features','--features','test-tools','--test','checkpoint2','--test','lsp_manager','--','--test-threads',"$Threads")
        $report.not_executed = @('headless GPUI (use Full)','properties (use Full)','external language servers and installers','desktop interaction and appearance')
    } elseif ($Mode -eq 'Performance') {
        $metrics = Stage 'internal-release' $cargo @('test','--release','--locked','--no-default-features','--test','performance','--','--ignored','--nocapture','--test-threads','1')
        # Stage collects structured metrics from each measured command.
        $null = Stage 'recovery-escaped-io' $cargo @('test','--release','--locked','--no-default-features','--lib','escaped_snapshot_io_measurement','--','--ignored','--nocapture','--test-threads','1')
        $null = Stage 'completion-filter' $cargo @('test','--release','--locked','--no-default-features','--lib','completion_filter_latency','--','--ignored','--nocapture','--test-threads','1')
        $report.not_executed = @('real DirectWrite shaping/GPU/display timings','native startup/idle (separate GUI/performance scripts)','correctness suites')
    } else {
        if (!$IsWindows -or !$Interactive -or ![Environment]::UserInteractive -or (Get-Process -Id $PID).SessionId -eq 0) {
            $report.status = 'not_executed'; $report.reason = 'GUI requires Windows desktop session and explicit -Interactive acknowledgment'; $exitCode = 2
        } else {
            if (!$Executable) {
                $pwsh = (Get-Process -Id $PID).Path
                $null = Stage 'release-build' $pwsh @('-NoProfile','-File',(Join-Path $PSScriptRoot 'windows-build.ps1'),'-Release')
                $Executable = Join-Path $root 'target/release/filavim.exe'
            }
            $exe = (Resolve-Path -LiteralPath $Executable).Path
            $fixture = Join-Path $artifacts 'fixture.rs'
            [IO.File]::WriteAllText($fixture, ("fn main() {`n`tlet café = 1; // 😀`n" + ("// fixture line`n" * 150) + "}`n"), [Text.UTF8Encoding]::new($false))
            $hash = (Get-FileHash -LiteralPath $fixture).Hash
            $nativeReport = Join-Path $artifacts 'native-smoke.txt'
            $report.native_report = $nativeReport
            $null = Stage 'native-smoke' $exe @('--smoke-test',$nativeReport,$fixture) 40 -Visible
            if ((Get-FileHash -LiteralPath $fixture).Hash -ne $hash) { throw 'Native smoke modified its original fixture' }
            $report.native_report = $nativeReport
            $report.not_executed = @('OS keyboard/IME automation','pixel/screenshot comparison','UI Automation selectors (no usable accessibility bridge)','external LSP compatibility')
        }
    }
    if ($report.status -eq 'running') { $report.status = 'passed' }
} catch {
    $report.status = 'failed'; $report.error = $_.Exception.Message; $exitCode = 1
    $report.not_executed += 'Subsequent stages were not executed after failure; see completed stage list'
    Write-Host $_ -ForegroundColor Red
} finally {
    foreach ($name in $environmentNames) { [Environment]::SetEnvironmentVariable($name, $previous[$name]) }
    Pop-Location
    $report.elapsed_seconds = [Math]::Round($suiteClock.Elapsed.TotalSeconds,3)
    $json = $report | ConvertTo-Json -Depth 12
    [IO.File]::WriteAllText((Join-Path $artifacts 'report.json'), $json, [Text.UTF8Encoding]::new($false))
    Write-Host "Report: $artifacts/report.json ($($report.status))"
}
exit $exitCode
