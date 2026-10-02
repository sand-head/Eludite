# Brief 0003 runner for Windows. UNTESTED: no Windows machine was available when this was written (see
# docs/briefs/0003-report.md). Mirrors run.sh: Build Tools MSBuild (located with vswhere, never bundled) replaces
# Mono, and an extra step compares the evaluator's Compile items with `MSBuild.exe -getItem:Compile` for three projects.
#
#   tools\legacy-load\run.ps1
#   tools\legacy-load\run.ps1 -PrepareOnly
param([switch]$PrepareOnly, [string]$Phases = "", [string]$Entries = "", [string]$RoslynModes = "")
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = Resolve-Path (Join-Path $here "..\..")
$src = if ($env:ROSLYN_SRC_DIR) { $env:ROSLYN_SRC_DIR } else { Join-Path $env:USERPROFILE ".cache\niello\roslyn" }
$lsDll = if ($env:NIELLO_ROSLYN_LS) { $env:NIELLO_ROSLYN_LS } else { Join-Path $src "artifacts\bin\Microsoft.CodeAnalysis.LanguageServer\Release\net10.0\Microsoft.CodeAnalysis.LanguageServer.dll" }

# corpus/legacy/fetch.sh needs bash (Git for Windows ships one).
& bash (Join-Path $repo "corpus/legacy/fetch.sh")
if ($LASTEXITCODE -ne 0) { throw "fetch failed" }

$refdl = Join-Path $here "runner\obj\refdl"
New-Item -ItemType Directory -Force $refdl | Out-Null
$pkgs = "net40;net45;net451;net452;net46;net461;net462;net47;net471;net472;net48;net481".Split(";") | ForEach-Object { "Microsoft.NETFramework.ReferenceAssemblies.$_" }
@"
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><TargetFramework>net10.0</TargetFramework><ManagePackageVersionsCentrally>false</ManagePackageVersionsCentrally></PropertyGroup>
  <ItemGroup><PackageDownload Include="$($pkgs -join ';')" Version="[1.0.3]" /></ItemGroup>
</Project>
"@ | Set-Content (Join-Path $refdl "refdl.csproj")
Copy-Item (Join-Path $repo "global.json") $refdl
Push-Location $refdl; dotnet restore -v q; Pop-Location

dotnet build (Join-Path $here "runner\LegacyLoad.Runner.csproj") -c Release -v q -nologo
if ($PrepareOnly) { exit 0 }

$bin = Join-Path $here "runner\bin\Release\net10.0"
$results = Join-Path $here ("results\" + (Get-Date -Format "yyyyMMdd-HHmmss"))
New-Item -ItemType Directory -Force $results | Out-Null
$runArgs = @("run", "--manifest", (Join-Path $repo "corpus\legacy\manifest.json"), "--checkout", (Join-Path $repo "corpus\legacy\.checkout"),
          "--results", $results, "--host", (Join-Path $bin "niello-host.dll"))
if (Test-Path $lsDll) { $runArgs += @("--roslyn-ls", $lsDll) } else { Write-Warning "Roslyn LS not found; skipping the roslyn phase" }
if ($Phases) { $runArgs += @("--phases", $Phases) }
if ($Entries) { $runArgs += @("--entries", $Entries) }
if ($RoslynModes) { $runArgs += @("--roslyn-modes", $RoslynModes) }
& (Join-Path $bin "legacy-load.exe") @runArgs

# Reference comparison: real MSBuild's Compile items (-getItem needs MSBuild 17.8+) versus the evaluator's.
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$msbuild = & $vswhere -latest -products * -requires Microsoft.Component.MSBuild -find "MSBuild\**\Bin\MSBuild.exe" | Select-Object -First 1
$checkout = Join-Path $repo "corpus\legacy\.checkout"
$compare = @(
  "aspnet-samples\samples\aspnet\Identity\ChangePK\PrimaryKeysConfigTest\PrimaryKeysConfigTest.csproj",
  "dotnet-samples\framework\wcf\Basic\Services\ConfigSimplificationIn40\Web-Hosted\CS\Service\service.csproj",
  "sharex\ShareX.HelpersLib\ShareX.HelpersLib.csproj",
  "umbraco7\src\Umbraco.Core\Umbraco.Core.csproj")
foreach ($rel in $compare) {
  $proj = Join-Path $checkout $rel
  $real = (& $msbuild $proj -nologo -getItem:Compile | ConvertFrom-Json).Items.Compile | ForEach-Object { $_.FullPath } | Sort-Object
  $evalFile = Get-ChildItem -Recurse (Join-Path $results "eval") -Filter ([IO.Path]::GetFileNameWithoutExtension($proj) + ".json") |
    Where-Object { $_.Directory.Name -eq "buildtools-msbuild" } | Select-Object -First 1
  $ours = (Get-Content $evalFile.FullName | ConvertFrom-Json).results[0].compileItems | Sort-Object
  $diff = Compare-Object $real $ours
  "{0}: real {1}, evaluator {2}, differences {3}" -f $rel, $real.Count, $ours.Count, $diff.Count | Tee-Object -Append (Join-Path $results "getitem-compare.txt")
  $diff | Out-String | Add-Content (Join-Path $results "getitem-compare.txt")
}
Write-Host "results: $results"
