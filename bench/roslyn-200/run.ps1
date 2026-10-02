# Brief 0002 bench, Windows variant of run.sh. UNTESTED: written on Linux, not yet run on Windows.
#   pwsh bench/roslyn-200/run.ps1                # 10 cold + 10 warm
#   $env:COLD=3; $env:WARM=3; pwsh bench/roslyn-200/run.ps1
#   pwsh bench/roslyn-200/run.ps1 -PrepareOnly
# Peak memory on Windows is the PeakWorkingSet64 of eludite-host only; the driver's process-tree sampling is Linux-only.
param([switch]$PrepareOnly)
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = Resolve-Path (Join-Path $here '../..')
$src = if ($env:ROSLYN_SRC_DIR) { $env:ROSLYN_SRC_DIR } else { Join-Path $HOME '.cache/eludite/roslyn' }
$lsDir = Join-Path $src 'artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/Release/net10.0'
$lsDll = if ($env:ELUDITE_ROSLYN_LS) { $env:ELUDITE_ROSLYN_LS } else { Join-Path $lsDir 'Microsoft.CodeAnalysis.LanguageServer.dll' }
$cold = if ($env:COLD) { $env:COLD } else { 10 }
$warm = if ($env:WARM) { $env:WARM } else { 10 }
$t3 = if ($env:T3) { $env:T3 } else { 1000 }
$cancel = if ($env:CANCEL_TRIALS) { $env:CANCEL_TRIALS } else { 50 }

if (-not (Test-Path $lsDll)) { throw "Roslyn language server not found at $lsDll; run tools/roslyn-pin/build.ps1 first" }
if (-not (Test-Path (Join-Path $here 'out/probe.json')) -or $env:REGENERATE -eq '1') {
  python (Join-Path $here 'generate.py') --out (Join-Path $here 'out') | Out-Null
  if ($LASTEXITCODE) { exit $LASTEXITCODE }
}
dotnet restore (Join-Path $here 'out/Bench200.slnx') -v q; if ($LASTEXITCODE) { exit $LASTEXITCODE }
dotnet build (Join-Path $repo 'dotnet/src/Eludite.Host/Eludite.Host.csproj') -c Release -v q -nologo; if ($LASTEXITCODE) { exit $LASTEXITCODE }
dotnet build (Join-Path $here 'driver/Bench.Driver.csproj') -c Release -v q -nologo; if ($LASTEXITCODE) { exit $LASTEXITCODE }
if ($PrepareOnly) { exit 0 }

$results = Join-Path $here ("results/" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force -Path $results | Out-Null
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$os = Get-CimInstance Win32_OperatingSystem
$disk = Get-PhysicalDisk | Select-Object -First 1
@(
  "date: $(Get-Date -Format o)",
  "cpu: $($cpu.Name)",
  "cores: $($cpu.NumberOfLogicalProcessors) logical; $($cpu.NumberOfCores) cores",
  "ram: $([math]::Round($os.TotalVisibleMemorySize / 1MB, 1)) GB",
  "disk: $($disk.FriendlyName) ($($disk.MediaType), $($disk.BusType))",
  "os: $($os.Caption) $($os.Version)",
  "dotnet: $(dotnet --version)",
  "roslyn: $((Get-Content (Join-Path $repo 'tools/roslyn-pin/COMMIT') -Raw).Trim())",
  "runs: cold=$cold warm=$warm t3=$t3 cancel-trials=$cancel"
) | Tee-Object -FilePath (Join-Path $results 'machine.txt')

$reset = (Join-Path $lsDir 'cache') + [IO.Path]::PathSeparator + (Join-Path ([IO.Path]::GetTempPath()) 'roslyn-canonical-misc')
dotnet (Join-Path $here 'driver/bin/Release/net10.0/Bench.Driver.dll') `
  --host (Join-Path $repo 'dotnet/src/Eludite.Host/bin/Release/net10.0/eludite-host.dll') `
  --roslyn-ls $lsDll --probe (Join-Path $here 'out/probe.json') --results $results `
  --cold $cold --warm $warm --t3 $t3 --cancel-trials $cancel --cold-reset-paths $reset
exit $LASTEXITCODE
