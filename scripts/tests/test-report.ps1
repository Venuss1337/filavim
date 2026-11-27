#requires -Version 7.2
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../test-report.ps1')
$text = "test optional ... ignored, requires external server`ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 8 filtered out;`ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out;"
$summary = Get-CargoTestSummary $text
if ($summary.passed -ne 5 -or $summary.failed -ne 1 -or $summary.ignored -ne 1) { throw 'Multiple target summaries were not summed accurately' }
if ($summary.ignored_tests.Count -ne 1) { throw 'Ignored test reason was lost' }
Assert-CargoTestsExecuted $summary
$compile = Get-CargoTestSummary 'error[E0425]: compilation failed'
if ($compile.passed -ne 0 -or $compile.failed -ne 0) { throw 'Compilation failure invented executed test cases' }
foreach ($text in @('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out;', 'test result: ok. 0 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out;')) {
    $rejected = $false
    try { Assert-CargoTestsExecuted (Get-CargoTestSummary $text) } catch { $rejected = $true }
    if (!$rejected) { throw 'Empty or ignored-only required stage was incorrectly accepted' }
}
Write-Host 'Cargo report parser checks passed'
