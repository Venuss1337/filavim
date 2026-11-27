param(
    [string]$Executable = "$PSScriptRoot/../target/release/filavim.exe",
    [Parameter(Mandatory=$true)][string]$File,
    [Parameter(Mandatory=$true)][string]$Project
)
$ErrorActionPreference = 'Stop'
$executablePath = (Resolve-Path -LiteralPath $Executable).Path
$filePath = (Resolve-Path -LiteralPath $File).Path
$projectPath = (Resolve-Path -LiteralPath $Project).Path
$artifactRoot = Join-Path $PSScriptRoot '../target/checkpoint2-measurements'
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
$configPath = [IO.Path]::GetFullPath((Join-Path $artifactRoot 'isolated.toml'))
[IO.File]::WriteAllText($configPath, "[lsp]`nenabled=false`n", [Text.UTF8Encoding]::new($false))
$previousConfig = $env:FILAVIM_GPUI_CONFIG
function Measure-NativeRun([string]$inputPath, [int]$settleSeconds, [int]$sampleSeconds) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $executablePath -ArgumentList @(('"' + $inputPath + '"')) -WindowStyle Normal -PassThru
    try {
        do {
            Start-Sleep -Milliseconds 10
            $process.Refresh()
            if ($process.HasExited) { throw "Application exited before a native window appeared" }
        } while ($process.MainWindowHandle -eq 0 -and $timer.ElapsedMilliseconds -lt 10000)
        if ($process.MainWindowHandle -eq 0) { throw 'Native window timeout' }
        $windowMilliseconds = $timer.Elapsed.TotalMilliseconds
        Start-Sleep -Seconds $settleSeconds
        $process.Refresh()
        $beforeCpu = $process.TotalProcessorTime.TotalMilliseconds
        $sample = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds $sampleSeconds
        $process.Refresh()
        $wall = $sample.Elapsed.TotalMilliseconds
        $cpu = $process.TotalProcessorTime.TotalMilliseconds - $beforeCpu
        [pscustomobject]@{
            input = $inputPath
            window_ms = [Math]::Round($windowMilliseconds, 2)
            resident_mib = [Math]::Round($process.WorkingSet64 / 1MB, 2)
            private_mib = [Math]::Round($process.PrivateMemorySize64 / 1MB, 2)
            cpu_ms = $cpu
            sample_wall_ms = [Math]::Round($wall, 2)
            cpu_one_core_percent = [Math]::Round(100 * $cpu / $wall, 3)
        }
    } finally {
        $process.Refresh()
        if (!$process.HasExited) {
            $null = $process.CloseMainWindow()
            if (!$process.WaitForExit(3000)) { Stop-Process -Id $process.Id }
        }
        $process.Dispose()
    }
}
try {
    $env:FILAVIM_GPUI_CONFIG = $configPath
    $warm = @(1..3 | ForEach-Object { Measure-NativeRun $filePath 1 1 })
    $idle = Measure-NativeRun $filePath 4 10
    $projectIdle = Measure-NativeRun $projectPath 4 10
    $result = [pscustomobject]@{ timestamp_utc=[DateTime]::UtcNow.ToString('o'); logical_processors=[Environment]::ProcessorCount; warm_runs=$warm; file_idle=$idle; indexed_project_idle=$projectIdle; limitations='Window metadata, not first paint or display latency. Project run indexes a directory with an empty document. LSP disabled; GPU allocations and external processes excluded.' }
    $json = $result | ConvertTo-Json -Depth 6
    [IO.File]::WriteAllText((Join-Path $artifactRoot 'windows-native-perf.json'), $json, [Text.UTF8Encoding]::new($false))
    $json
} finally {
    $env:FILAVIM_GPUI_CONFIG = $previousConfig
}
