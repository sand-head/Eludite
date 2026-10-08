# Build a machine-wide MSI from shell.sh's Windows layout. Run on Windows with .NET SDK 10:
#   pwsh tools/package/windows.ps1 -Layout target/package/eludite-26.10.3-windows-x86_64
# The MSI ProductVersion is the layout's version, CI's <YY>.<M>.<iteration> (tools/package/version.sh), which
# increases with every build of main.
param(
    [Parameter(Mandatory = $true)][string]$Layout,
    [string]$Out = 'target/package'
)
$ErrorActionPreference = 'Stop'
$layoutPath = (Resolve-Path $Layout).Path
if (-not (Test-Path (Join-Path $layoutPath 'eludite.exe'))) { throw 'The Windows package layout must contain eludite.exe.' }
if ((Split-Path $layoutPath -Leaf) -notmatch '^eludite-(\d+)\.(\d+)\.(\d+)-windows-x86_64$') {
    throw 'Expected the x86_64 Windows layout from tools/package/shell.sh.'
}
$major = [int]$Matches[1]
$minor = [int]$Matches[2]
$patch = [int]$Matches[3]
if ($major -gt 255 -or $minor -gt 255 -or $patch -gt 65535) {
    throw 'MSI version fields must be major/minor <= 255 and patch <= 65535.'
}
$msiVersion = "$major.$minor.$patch"
$outPath = [System.IO.Path]::GetFullPath($Out, (Get-Location).Path)
New-Item -ItemType Directory -Force -Path $outPath | Out-Null
$toolPath = Join-Path $outPath '.wix-tool'
$wix = Join-Path $toolPath 'wix.exe'
if (-not (Test-Path $wix)) {
    dotnet tool install wix --version 5.0.2 --tool-path $toolPath
    if ($LASTEXITCODE -ne 0) { throw 'Could not install the pinned WiX 5.0.2 build tool.' }
}
$name = "$(Split-Path $layoutPath -Leaf).msi"
$msi = Join-Path $outPath $name
& $wix build -arch x64 -bindpath "Layout=$layoutPath" -d "MsiVersion=$msiVersion" -o $msi (Join-Path $PSScriptRoot 'windows.wxs')
if ($LASTEXITCODE -ne 0) { throw 'WiX failed to build the MSI.' }
& $wix msi validate $msi
if ($LASTEXITCODE -ne 0) { throw 'MSI failed Windows Installer validation.' }
Write-Output $msi
