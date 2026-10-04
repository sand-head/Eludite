# Fetch vscode-js-debug's standalone DAP server, pinned in tools/js-debug/PIN (brief 0038), on Windows: check its
# SHA-256 and unpack it to %USERPROFILE%\.cache\eludite\js-debug\<version>\. Prints the path of
# js-debug\src\dapDebugServer.js, which Eludite runs with the Node.js found on the machine.
#
#   tools\js-debug\fetch.ps1                    # download, verify, unpack
#   tools\js-debug\fetch.ps1 -Dest D:\cache     # another cache folder (Eludite then needs ELUDITE_JS_DEBUG)
#   $env:JS_DEBUG_PIN_FROM_DOWNLOAD = '1'; tools\js-debug\fetch.ps1
#                                               # PIN has no checksum yet: download once, print the line for PIN
#
# Upstream: https://github.com/microsoft/vscode-js-debug  License: MIT (SPDX: MIT). Needs tar.exe (Windows 10 1803 and
# later), which reads .tar.gz.
param([string]$Dest)
$ErrorActionPreference = 'Stop'
$pin = Get-Content (Join-Path $PSScriptRoot 'PIN') | Where-Object { $_ -notmatch '^#' }
function Field($name) {
  $line = $pin | Where-Object { $_ -match "^$name " } | Select-Object -First 1
  if ($line) { ($line -split ' ')[1] } else { $null }
}
$version = Field 'version'
$base = Field 'base'
$file = Field 'file'
$sha = Field 'sha256'
if (-not $Dest) { $Dest = Join-Path $env:USERPROFILE '.cache\eludite\js-debug' }
$out = Join-Path $Dest $version
$server = Join-Path $out 'js-debug\src\dapDebugServer.js'
if ((Test-Path $server) -and (Test-Path (Join-Path $out '.complete'))) { $server; exit 0 }
if (-not $sha -and $env:JS_DEBUG_PIN_FROM_DOWNLOAD -ne '1') {
  Write-Error ("js-debug fetch: tools\js-debug\PIN names version $version but no sha256 yet. On a machine that reaches " +
    "GitHub run `$env:JS_DEBUG_PIN_FROM_DOWNLOAD = '1'; tools\js-debug\fetch.ps1, add the sha256 line it prints to PIN " +
    "and commit it.")
  exit 1
}
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$stage = Join-Path $Dest ('.fetch.' + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
try {
  $archive = Join-Path $stage $file
  Write-Host "js-debug fetch: $base/$file"
  Invoke-WebRequest -Uri "$base/$file" -OutFile $archive
  $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
  if (-not $sha) {
    Write-Host "js-debug fetch: downloaded $file, SHA-256 $actual. Add this line to tools\js-debug\PIN:"
    Write-Host "sha256 $actual"
  } elseif ($actual -ne $sha) {
    throw "SHA-256 mismatch for ${file}: expected $sha, got $actual"
  }
  tar -xzf $archive -C $stage
  if ($LASTEXITCODE -ne 0) { throw "tar failed with $LASTEXITCODE" }
  if (-not (Test-Path (Join-Path $stage 'js-debug\src\dapDebugServer.js'))) { throw "$file has no js-debug\src\dapDebugServer.js" }
  if (Test-Path $out) { Remove-Item -Recurse -Force $out }
  New-Item -ItemType Directory -Force -Path $out | Out-Null
  Move-Item (Join-Path $stage 'js-debug') (Join-Path $out 'js-debug')
  New-Item -ItemType File -Path (Join-Path $out '.complete') | Out-Null
} finally {
  Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
Write-Host "js-debug fetch: vscode-js-debug $version in $out (MIT)"
$server
