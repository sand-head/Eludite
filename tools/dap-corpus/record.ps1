# Re-record brief 0033's DAP conformance corpus (corpus\dap\) on Windows from every real adapter on this machine,
# through the shell's conformance tests (crates\eludite\src\shell\debug\conformance_tests.rs): lldb-dap and
# netcoredbg (.NET Framework under Mono is not debugged on Windows, so Mono is always skipped here). Prints which
# adapters it records and which it skips, and why. vscode-js-debug's scenarios (brief 0038) are recorded on Linux
# only (tools/dap-corpus/record.sh js-debug): they need the embedded engine, which runs on Linux so far. The checked-in Mono and lldb-dap recordings are made on Linux
# (tools/dap-corpus/record.sh); a Windows recording is for comparing, not for checking in over them.
#
#   tools\dap-corpus\record.ps1 [-Check] [lldb] [netcoredbg]
#
# With no adapter named, every one found is recorded. Without -Check the recordings and goldens are written into
# corpus\dap\. With -Check they go to a temporary folder (or $env:RECORD_DAP) and each must reproduce the checked-in
# one, timing aside. Recordings are only ever made this way, never edited by hand.
param(
    [switch]$Check,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$Adapters
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Set-Location $root

$required = $true
if (-not $Adapters -or $Adapters.Count -eq 0) {
    $Adapters = @('mono', 'lldb', 'netcoredbg')
    $required = $false
}
function Have($name) { [bool](Get-Command $name -ErrorAction SilentlyContinue) }

$found = @()
foreach ($a in $Adapters) {
    $why = $null
    switch ($a) {
        'mono' { $why = '.NET Framework under Mono is not debugged on Windows' }
        'lldb' {
            if (-not $env:ELUDITE_LLDB_DAP -and -not (Have 'lldb-dap')) { $why = 'lldb-dap is not on PATH (or set ELUDITE_LLDB_DAP)' }
            elseif (-not (Have 'cargo')) { $why = 'cargo is not on PATH' }
        }
        'netcoredbg' {
            if (-not $env:ELUDITE_NETCOREDBG -and -not (Have 'netcoredbg')) { $why = 'netcoredbg is not found (tools/netcoredbg/fetch.sh prints the path for ELUDITE_NETCOREDBG)' }
            elseif (-not (Have 'dotnet')) { $why = 'the .NET SDK (dotnet) is not on PATH' }
        }
        default { Write-Error "usage: record.ps1 [-Check] [lldb] [netcoredbg]"; exit 2 }
    }
    if ($why) {
        if ($required) { Write-Error "${a}: $why"; exit 1 }
        Write-Output "skipped ${a}: $why"
    } else {
        Write-Output "recording $a"
        $found += $a
    }
}
if ($found.Count -eq 0) { Write-Output 'no adapter to record with'; exit 0 }

$filters = $found | ForEach-Object { "conformance_tests::$($_)_" }
if ($Check) {
    if (-not $env:RECORD_DAP) {
        $env:RECORD_DAP = Join-Path ([System.IO.Path]::GetTempPath()) ("dap-corpus-" + [guid]::NewGuid())
    }
    $env:DAP_CORPUS_CHECK = '1'
    if ($required) { $env:DAP_CORPUS_REQUIRE = ($found -join ',') }
} else {
    $env:RECORD_DAP = Join-Path $root 'corpus\dap'
}
Write-Output "writing to $env:RECORD_DAP"
# One scenario at a time: the real adapters are not shared.
& cargo test -p eludite --bin eludite -- @filters --test-threads=1 --nocapture
exit $LASTEXITCODE
