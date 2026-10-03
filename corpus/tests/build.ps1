# Build brief 0035's Test Explorer corpus on Windows, in the Debug configuration the Test Explorer discovers: the four
# .NET test projects and the Cargo package's test executables (`cargo test --no-run`). Extra arguments go to every
# `dotnet build`; `-NoCargo` leaves the Cargo package out (a .NET-only machine).
#   corpus\tests\build.ps1 [-NoCargo] [dotnet build args...]
$ErrorActionPreference = 'Stop'
$withCargo = $true
if ($args.Count -gt 0 -and $args[0] -eq '-NoCargo') {
    $withCargo = $false
    $args = $args[1..($args.Count)] | Where-Object { $_ -ne $null }
}
foreach ($project in 'Corpus.XunitV3', 'Corpus.Xunit2', 'Corpus.MSTest', 'Corpus.NUnit') {
    & dotnet build (Join-Path $PSScriptRoot "$project\$project.csproj") --configuration Debug --nologo @args
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
if ($withCargo) {
    & cargo test --no-run --manifest-path (Join-Path $PSScriptRoot 'rust\Cargo.toml') --quiet
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
