# Install FsAutoComplete, the F# language server, at the version pinned in tools/fsautocomplete/PIN (brief 0063) on
# Windows as a .NET tool into %USERPROFILE%\.cache\eludite\fsautocomplete\<version>\ with `dotnet tool install
# --tool-path`, on the machine's .NET SDK (nothing is bundled). FsAutoComplete is MIT. Prints the executable's path,
# which Eludite searches by itself after the eludite folder and ELUDITE_FSAUTOCOMPLETE, and before
# %USERPROFILE%\.dotnet\tools and PATH.
#
#   tools\fsautocomplete\fetch.ps1                      # install (once); print the executable's path
#   tools\fsautocomplete\fetch.ps1 -Dest D:\tools\fsac  # another folder (Eludite then needs ELUDITE_FSAUTOCOMPLETE)
#
# Needs the .NET SDK (`dotnet` on PATH) and the network once (nuget.org); idempotent.
param([string]$Dest)
$ErrorActionPreference = 'Stop'
$pin = Get-Content (Join-Path $PSScriptRoot 'PIN') | Where-Object { $_ -notmatch '^#' }
$version = (($pin | Where-Object { $_ -match '^version ' }) -split ' ')[1]
if (-not $Dest) { $Dest = Join-Path $env:USERPROFILE ".cache\eludite\fsautocomplete\$version" }
$exe = Join-Path $Dest 'fsautocomplete.exe'
if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) { throw 'dotnet was not found on PATH (install the .NET SDK)' }
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_NOLOGO = '1'
$installed = ''
if (Test-Path $exe) { $installed = (& $exe --version 2>$null | Select-Object -First 1) }
if ($installed -and $installed.StartsWith($version)) { $exe; exit 0 }
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
if (Test-Path $exe) {
  Write-Host "fsautocomplete fetch: replacing $installed with $version in $Dest"
  & dotnet tool update fsautocomplete --version $version --tool-path $Dest | Write-Host
} else {
  Write-Host "fsautocomplete fetch: dotnet tool install fsautocomplete $version into $Dest"
  & dotnet tool install fsautocomplete --version $version --tool-path $Dest | Write-Host
}
if ($LASTEXITCODE) { throw 'dotnet tool install failed' }
& $exe --version | Write-Host
if ($PSBoundParameters.ContainsKey('Dest')) {
  Write-Host "fsautocomplete fetch: Eludite searches %USERPROFILE%\.cache\eludite\fsautocomplete\$version, %USERPROFILE%\.dotnet\tools and PATH; for this folder set ELUDITE_FSAUTOCOMPLETE=$exe"
}
$exe
