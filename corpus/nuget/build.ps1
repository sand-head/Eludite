# Build brief 0048's local NuGet feed in place (feed/), as build.sh does: Eludite.Corpus.Logging 1.0.0, then
# Eludite.Corpus.Greeter 1.0.0, 1.1.0 and 2.0.0-beta.1, with `dotnet pack`, offline.
#   corpus/nuget/build.ps1 [corpus folder, default this script's]
param([string]$Root = $PSScriptRoot)
$ErrorActionPreference = 'Stop'
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_NOLOGO = '1'
$dotnet = if ($env:DOTNET) { $env:DOTNET } else { 'dotnet' }
$feed = Join-Path $Root 'feed'
New-Item -ItemType Directory -Force -Path $feed | Out-Null
function Pack([string]$Project, [string]$Version) {
  & $dotnet pack (Join-Path $Root "packages/$Project/$Project.csproj") --configuration Release --nologo -v q `
    --output $feed "-p:Version=$Version" "-p:PackageVersion=$Version"
  if ($LASTEXITCODE -ne 0) { throw "dotnet pack $Project $Version failed" }
}
Pack 'Eludite.Corpus.Logging' '1.0.0'
foreach ($v in '1.0.0', '1.1.0', '2.0.0-beta.1') { Pack 'Eludite.Corpus.Greeter' $v }
Get-ChildItem $feed
