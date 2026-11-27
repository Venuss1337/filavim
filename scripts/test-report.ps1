# Pure report parsing; no process execution or interpretation of exit status.
function Get-CargoTestSummary([string]$Text) {
    $passed = 0; $failed = 0; $ignored = 0
    foreach ($match in [regex]::Matches($Text, 'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;')) {
        $passed += [int]$match.Groups[1].Value
        $failed += [int]$match.Groups[2].Value
        $ignored += [int]$match.Groups[3].Value
    }
    [pscustomobject]@{
        passed=$passed; failed=$failed; ignored=$ignored
        ignored_tests=@([regex]::Matches($Text, '(?m)^test (.+) \.\.\. ignored[^\r\n]*') | ForEach-Object Value)
    }
}
function Assert-CargoTestsExecuted($Summary) {
    if (($Summary.passed + $Summary.failed) -eq 0) {
        throw 'Selected no executable tests; a required suite must not silently pass'
    }
}
