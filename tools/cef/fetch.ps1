# Fetch CEF's minimal distribution, pinned in tools/cef/PIN (brief 0031), on Windows: check its SHA-256 and unpack it to
# %USERPROFILE%\.cache\eludite\cef\<version>\ in the layout the `cef` crate's build script expects (download-cef's:
# Release and Resources flattened into one folder with include\, cmake\, libcef_dll\ and archive.json). Prints that
# folder; set CEF_PATH to it to build eludite-chromium with `--features eludite-chromium/cef`.
#
#   tools\cef\fetch.ps1                   # download (173 MB), verify, unpack
#   tools\cef\fetch.ps1 -Dest D:\cache    # another cache folder (Eludite then needs ELUDITE_CEF or CEF_PATH)
#
# Needs tar.exe (bsdtar, part of Windows 10 1803 and later), which reads .tar.bz2.
param([string]$Dest)
$ErrorActionPreference = 'Stop'
$pin = Get-Content (Join-Path $PSScriptRoot 'PIN') | Where-Object { $_ -notmatch '^#' }
$version = (($pin | Where-Object { $_ -match '^version ' }) -split ' ')[1]
$base = (($pin | Where-Object { $_ -match '^base ' }) -split ' ')[1]
$name = (($pin | Where-Object { $_ -match '^file windows64 ' }) -split ' ')[2]
$sha = (($pin | Where-Object { $_ -match '^sha256 windows64 ' }) -split ' ')[2]
$sha1 = (($pin | Where-Object { $_ -match '^sha1 windows64 ' }) -split ' ')[2]
if (-not $Dest) { $Dest = Join-Path $env:USERPROFILE '.cache\eludite\cef' }
$out = Join-Path $Dest $version
if ((Test-Path (Join-Path $out 'archive.json')) -and (Test-Path (Join-Path $out 'libcef.dll'))) { $out; exit 0 }
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$stage = Join-Path $Dest ('.fetch.' + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
try {
  $url = "$base/" + $name.Replace('+', '%2B')
  $archive = Join-Path $stage $name
  Write-Host "cef fetch: $url"
  Invoke-WebRequest -Uri $url -OutFile $archive
  $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
  if ($actual -ne $sha) { throw "SHA-256 mismatch for ${name}: expected $sha, got $actual" }
  Write-Host 'cef fetch: unpacking'
  tar -xjf $archive -C $stage
  if ($LASTEXITCODE -ne 0) { throw "tar failed with $LASTEXITCODE" }
  Remove-Item $archive
  $src = Join-Path $stage ($name -replace '\.tar\.bz2$', '')
  $tmp = Join-Path $stage 'out'
  Move-Item (Join-Path $src 'Release') $tmp
  Get-ChildItem (Join-Path $src 'Resources') | Move-Item -Destination $tmp
  foreach ($f in 'CMakeLists.txt', 'cmake', 'include', 'libcef_dll', 'CREDITS.html', 'LICENSE.txt', 'README.txt') {
    $p = Join-Path $src $f
    if (Test-Path $p) { Move-Item $p $tmp }
  }
  if (Test-Path $out) { Remove-Item -Recurse -Force $out }
  Move-Item $tmp $out
  # archive.json last: its presence means the folder is complete.
  "{`n  `"type`": `"minimal`",`n  `"name`": `"$name`",`n  `"sha1`": `"$sha1`"`n}" | Set-Content -NoNewline -Encoding ascii (Join-Path $out 'archive.json')
} finally {
  Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
Write-Host "cef fetch: CEF $version in $out"
$out
