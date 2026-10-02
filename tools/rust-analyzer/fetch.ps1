# Fetch the rust-analyzer release pinned in tools/rust-analyzer/PIN (brief 0019) on Windows, check its SHA-256 and
# install it. rust-analyzer is MIT OR Apache-2.0. Eludite finds it beside eludite.exe, at ELUDITE_RUST_ANALYZER, on
# PATH, then as the rustup component.
#
#   tools\rust-analyzer\fetch.ps1                   # into %LOCALAPPDATA%\eludite\rust-analyzer\<tag>\
#   tools\rust-analyzer\fetch.ps1 target\debug      # beside a built eludite.exe
param([string]$Dest)
$ErrorActionPreference = 'Stop'
$pin = Get-Content (Join-Path $PSScriptRoot 'PIN') | Where-Object { $_ -notmatch '^#' }
$tag = ($pin | Where-Object { $_ -match '^tag ' }) -replace '^tag ', ''
$triple = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$sha = (($pin | Where-Object { $_ -match "^sha256 $triple " }) -split ' ')[2]
if (-not $Dest) { $Dest = Join-Path $env:LOCALAPPDATA "eludite\rust-analyzer\$tag" }
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$url = "https://github.com/rust-lang/rust-analyzer/releases/download/$tag/rust-analyzer-$triple.zip"
$zip = Join-Path ([System.IO.Path]::GetTempPath()) "rust-analyzer-$tag.zip"
Write-Host "rust-analyzer fetch: $url"
Invoke-WebRequest -Uri $url -OutFile $zip
$actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
if ($actual -ne $sha) { throw "SHA-256 mismatch: expected $sha, got $actual" }
Expand-Archive -Force -Path $zip -DestinationPath $Dest
Remove-Item $zip
& (Join-Path $Dest 'rust-analyzer.exe') --version
Join-Path $Dest 'rust-analyzer.exe'
