# Install the web language servers and formatters pinned in tools/web-servers/PIN (brief 0050) on Windows into
# %USERPROFILE%\.cache\eludite\web-servers\<pin>\ with `npm ci` from the checked-in package.json and package-lock.json,
# and cache the pinned SchemaStore JSON schemas under <pin>\schemas\ (each checked against its SHA-256). Prints the
# folder, which Eludite searches by itself after the project's node_modules and the ELUDITE_<SERVER> variables.
#
#   tools\web-servers\fetch.ps1                      # install (once); print the folder
#   tools\web-servers\fetch.ps1 -Cache D:\cache      # another cache folder (Eludite then needs ELUDITE_WEB_SERVERS)
#
# Needs Node.js (PIN's `node` major version or later) with npm. Every package and its SPDX license is in PIN.
param([string]$Cache)
$ErrorActionPreference = 'Stop'
$pinFile = Join-Path $PSScriptRoot 'PIN'
$lines = Get-Content $pinFile | Where-Object { $_ -notmatch '^#' }
function Field($k) { (($lines | Where-Object { $_ -match "^$k " } | Select-Object -First 1) -split ' ')[1] }
$pin = Field 'pin'
$nodeMin = [int](Field 'node')
$schemastore = Field 'schemastore'
if (-not $Cache) { $Cache = Join-Path $env:USERPROFILE '.cache\eludite\web-servers' }
$dest = Join-Path $Cache $pin
if (Test-Path (Join-Path $dest '.complete')) { $dest; exit 0 }

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) { throw "Node.js was not found on PATH (needs $nodeMin or later)" }
$major = [int]((& node --version) -replace '^v', '' -split '\.')[0]
if ($major -lt $nodeMin) { throw "Node.js $(& node --version) is older than $nodeMin, which the pinned servers need" }

New-Item -ItemType Directory -Force -Path $Cache | Out-Null
$stage = Join-Path $Cache (".fetch." + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
  Copy-Item (Join-Path $PSScriptRoot 'package.json'), (Join-Path $PSScriptRoot 'package-lock.json') $stage
  Write-Host "web-servers fetch: npm ci in $stage"
  Push-Location $stage
  try { & npm ci --ignore-scripts --no-audit --no-fund --loglevel=error | Write-Host; if ($LASTEXITCODE) { throw 'npm ci failed' } }
  finally { Pop-Location }
  $schemas = Join-Path $stage 'schemas'
  New-Item -ItemType Directory -Path $schemas | Out-Null
  foreach ($line in ($lines | Where-Object { $_ -match '^schema ' })) {
    $parts = $line -split ' '
    $file = $parts[1]; $sha = $parts[2]
    $out = Join-Path $schemas $file
    Invoke-WebRequest -Uri "https://raw.githubusercontent.com/SchemaStore/schemastore/$schemastore/src/schemas/json/$file" -OutFile $out
    $actual = (Get-FileHash -Algorithm SHA256 $out).Hash.ToLowerInvariant()
    if ($actual -ne $sha) { throw "SHA-256 mismatch for ${file}: expected $sha, got $actual" }
  }
  & node (Join-Path $PSScriptRoot 'rewrite-refs.mjs') (Join-Path $dest 'schemas') $schemas
  if ($LASTEXITCODE) { throw 'rewriting the schemas failed' }
  if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
  Move-Item $stage $dest
  New-Item -ItemType File -Path (Join-Path $dest '.complete') | Out-Null
} finally {
  if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
}
Write-Host "web-servers fetch: installed $pin into $dest"
$dest
