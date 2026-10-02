# Fetch the Chrome DevTools Protocol JSON pinned in protocol/cdp/PIN (brief 0023) on Windows, check the tarball's
# SHA-256 and copy json/browser_protocol.json, json/js_protocol.json and LICENSE (as LICENSE.chromium) here.
# devtools-protocol is BSD-3-Clause. Needs tar.exe (Windows 10 1803 and later).
#
#   protocol\cdp\fetch.ps1           # download, verify, copy
#   protocol\cdp\fetch.ps1 -Check    # download, verify, and fail if the checked-in files differ
param([switch]$Check)
$ErrorActionPreference = 'Stop'
$pin = Get-Content (Join-Path $PSScriptRoot 'PIN') | Where-Object { $_ -notmatch '^#' }
$url = (($pin | Where-Object { $_ -match '^url ' }) -split ' ')[1]
$sha = (($pin | Where-Object { $_ -match '^sha256 ' }) -split ' ')[1]
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("eludite-cdp-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
  $tgz = Join-Path $tmp 'package.tgz'
  Write-Host "cdp fetch: $url"
  Invoke-WebRequest -Uri $url -OutFile $tgz
  $actual = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLowerInvariant()
  if ($actual -ne $sha) { throw "SHA-256 mismatch: expected $sha, got $actual" }
  tar -xzf $tgz -C $tmp
  $pairs = @(@('json/browser_protocol.json', 'browser_protocol.json'), @('json/js_protocol.json', 'js_protocol.json'), @('LICENSE', 'LICENSE.chromium'))
  $failed = $false
  foreach ($p in $pairs) {
    $src = Join-Path $tmp ('package/' + $p[0])
    $dst = Join-Path $PSScriptRoot $p[1]
    if ($Check) {
      if ((Get-FileHash $src).Hash -ne (Get-FileHash $dst).Hash) { Write-Host "cdp fetch: $dst differs from the pinned package"; $failed = $true }
    } else {
      Copy-Item -Force $src $dst
    }
  }
  if ($failed) { exit 1 }
} finally {
  Remove-Item -Recurse -Force $tmp
}
