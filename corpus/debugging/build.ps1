# Build brief 0030's seeded-bug corpus on Windows: each program for net10.0 (netcoredbg) and net472, in the Debug
# configuration Eludite's F5 runs. Extra arguments go to every `dotnet build`.
#   corpus\debugging\build.ps1 [dotnet build args...]
$ErrorActionPreference = 'Stop'
foreach ($program in 'OffByOne', 'MissingCase', 'NullField') {
    & dotnet build (Join-Path $PSScriptRoot "$program\$program.csproj") --configuration Debug --nologo @args
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
