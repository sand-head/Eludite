# Build brief 0035's Test Explorer corpus on Windows, in the Debug configuration the Test Explorer discovers: the six
# .NET test projects (C#, and brief 0057's Visual Basic and F#) and the Cargo package's test executables
# (`cargo test --no-run`). Extra arguments go to every
# `dotnet build`; `-NoCargo` leaves the Cargo package out (a .NET-only machine).
#   corpus\tests\build.ps1 [-NoCargo] [dotnet build args...]
$ErrorActionPreference = 'Stop'
$withCargo = $true
if ($args.Count -gt 0 -and $args[0] -eq '-NoCargo') {
    $withCargo = $false
    $args = $args[1..($args.Count)] | Where-Object { $_ -ne $null }
}
# Each entry is the project file: C#, Visual Basic (.vbproj) and F# (.fsproj) alike.
foreach ($project in 'Corpus.XunitV3\Corpus.XunitV3.csproj', 'Corpus.Xunit2\Corpus.Xunit2.csproj', 'Corpus.MSTest\Corpus.MSTest.csproj',
    'Corpus.NUnit\Corpus.NUnit.csproj', 'Corpus.VisualBasic\Corpus.VisualBasic.vbproj', 'Corpus.FSharp\Corpus.FSharp.fsproj') {
    & dotnet build (Join-Path $PSScriptRoot $project) --configuration Debug --nologo @args
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
if ($withCargo) {
    & cargo test --no-run --manifest-path (Join-Path $PSScriptRoot 'rust\Cargo.toml') --quiet
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
