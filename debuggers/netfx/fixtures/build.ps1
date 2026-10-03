# Builds the brief 0004 debuggee (Counter, .NET Framework 4.8, Debug, portable PDB).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File debuggers/netfx/fixtures/build.ps1 [-ArtifactsPath DIR]
#
# Output goes under -ArtifactsPath (default: target/netfx-fixture at the repo root), never into the source tree.
# Prints the full path of Counter.exe as the last line of output.
param(
    [string]$ArtifactsPath = (Join-Path $PSScriptRoot '..\..\..\target\netfx-fixture')
)
$ErrorActionPreference = 'Stop'
$project = Join-Path $PSScriptRoot 'Counter\Counter.csproj'
& dotnet build $project -c Debug --nologo -v quiet --artifacts-path $ArtifactsPath | Out-Host
if ($LASTEXITCODE -ne 0) { throw "dotnet build failed with exit code $LASTEXITCODE" }
$exe = Get-ChildItem -Path (Join-Path $ArtifactsPath 'bin') -Recurse -Filter 'Counter.exe' | Select-Object -First 1
if (-not $exe) { throw "Counter.exe not found under $ArtifactsPath" }
$pdb = [System.IO.Path]::ChangeExtension($exe.FullName, '.pdb')
if (-not (Test-Path $pdb)) { throw "Counter.pdb not found beside $($exe.FullName)" }
Write-Output $exe.FullName
