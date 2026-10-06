# Fetch dotnet/roslyn at the commit pinned in tools/roslyn-pin/COMMIT and build
# Microsoft.CodeAnalysis.LanguageServer from source (brief 0002), with vb.patch applied so the server
# composes the Visual Basic feature assemblies too (brief 0057; see README.md).
# UNTESTED: written on Linux; not yet run on Windows.
#
#   pwsh tools/roslyn-pin/build.ps1
#   $env:ROSLYN_SRC_DIR = 'D:\roslyn'; pwsh tools/roslyn-pin/build.ps1
$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$commit = (Get-Content (Join-Path $here 'COMMIT') -Raw).Trim()
$src = if ($env:ROSLYN_SRC_DIR) { $env:ROSLYN_SRC_DIR } else { Join-Path $HOME '.cache/eludite/roslyn' }
$config = if ($env:ROSLYN_CONFIGURATION) { $env:ROSLYN_CONFIGURATION } else { 'Release' }
$project = 'src/LanguageServer/Microsoft.CodeAnalysis.LanguageServer/Microsoft.CodeAnalysis.LanguageServer.csproj'

# Roslyn's build refuses to run without Windows long path support (its eng\enable-long-paths.reg sets the same value).
$longPaths = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' -ErrorAction SilentlyContinue).LongPathsEnabled
if ($longPaths -ne 1) {
  throw "Roslyn needs long paths: from an elevated prompt run New-ItemProperty HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem -Name LongPathsEnabled -Value 1 -PropertyType DWord -Force"
}

if (-not (Test-Path (Join-Path $src '.git'))) {
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $src) | Out-Null
  git clone --filter=blob:none --no-checkout https://github.com/dotnet/roslyn.git $src
  if ($LASTEXITCODE) { exit $LASTEXITCODE }
}
# Roslyn's Razor test files pass MAX_PATH under the default clone directory.
git -C $src config core.longpaths true
git -C $src cat-file -e "$commit^{commit}" 2>$null
if ($LASTEXITCODE) { git -C $src fetch origin $commit; if ($LASTEXITCODE) { exit $LASTEXITCODE } }
git -C $src checkout -q --detach $commit
if ($LASTEXITCODE) { exit $LASTEXITCODE }

# Brief 0057: the server's MEF composition lists only the C# feature assemblies; vb.patch adds Visual Basic's.
# Idempotent: skipped when the patch is already in the tree, a loud failure when it fits neither way.
$patch = Join-Path $here 'vb.patch'
git -C $src apply --reverse --check $patch 2>$null
if (-not $LASTEXITCODE) {
  Write-Host 'roslyn-pin: vb.patch already applied'
} else {
  git -C $src apply --check $patch
  if ($LASTEXITCODE) { throw "vb.patch does not apply to $commit; fix tools/roslyn-pin/vb.patch" }
  git -C $src apply $patch
  if ($LASTEXITCODE) { exit $LASTEXITCODE }
  Write-Host 'roslyn-pin: applied vb.patch'
}

$sw = [Diagnostics.Stopwatch]::StartNew()
Push-Location $src
try {
  # Build.cmd adds -build itself and re-parses its arguments in a nested PowerShell, so -nodeReuse takes the literal
  # text '$false' (an expanded $false arrives as the string "False"). -msbuildEngine dotnet builds with the SDK, as
  # on Linux, instead of requiring a Visual Studio MSBuild.
  & .\Build.cmd -restore -configuration $config -solution $project -msbuildEngine dotnet '-nodeReuse:$false'
  if ($LASTEXITCODE) { exit $LASTEXITCODE }
} finally { Pop-Location }
$sw.Stop()
Write-Host "roslyn-pin: built in $([int]$sw.Elapsed.TotalSeconds) s"
$out = Join-Path $src "artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/$config/net10.0/Microsoft.CodeAnalysis.LanguageServer.dll"
if (-not (Test-Path $out)) { throw "expected output not found: $out" }
Write-Output $out
