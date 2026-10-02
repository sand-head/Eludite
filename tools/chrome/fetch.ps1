# Fetch Chrome for Testing, pinned in tools/chrome/PIN (brief 0023), on Windows: check its SHA-256 and unpack it to
# %USERPROFILE%\.cache\eludite\chrome\<version>\chrome-win64\, where Eludite's browser tools look for it. Prints the
# executable's path. Eludite searches, in order: the setting browser.chromePath (ELUDITE_CHROME), this cache, chrome
# on PATH, then %ProgramFiles%, %ProgramFiles(x86)% and %LocalAppData% \Google\Chrome\Application\chrome.exe.
#
#   tools\chrome\fetch.ps1                  # download (about 200 MB), verify, unpack
#   tools\chrome\fetch.ps1 -Dest D:\cache   # another cache folder (Eludite then needs ELUDITE_CHROME)
param([string]$Dest)
$ErrorActionPreference = 'Stop'
$pin = Get-Content (Join-Path $PSScriptRoot 'PIN') | Where-Object { $_ -notmatch '^#' }
$version = (($pin | Where-Object { $_ -match '^version ' }) -split ' ')[1]
$base = (($pin | Where-Object { $_ -match '^base ' }) -split ' ')[1]
$sha = (($pin | Where-Object { $_ -match '^sha256 win64 ' }) -split ' ')[2]
if (-not $Dest) { $Dest = Join-Path $env:USERPROFILE ".cache\eludite\chrome" }
$Dest = Join-Path $Dest $version
$exe = Join-Path $Dest 'chrome-win64\chrome.exe'
if (Test-Path $exe) { $exe; exit 0 }
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$url = "$base/$version/win64/chrome-win64.zip"
$zip = Join-Path ([System.IO.Path]::GetTempPath()) "chrome-win64-$version.zip"
Write-Host "chrome fetch: $url"
Invoke-WebRequest -Uri $url -OutFile $zip
$actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
if ($actual -ne $sha) { Remove-Item $zip; throw "SHA-256 mismatch: expected $sha, got $actual" }
Expand-Archive -Force -Path $zip -DestinationPath $Dest
Remove-Item $zip
if (-not (Test-Path $exe)) { throw "expected $exe after unpacking" }
$exe
