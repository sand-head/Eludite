# Regenerates src/grammar.json from grammar.js with the pinned tree-sitter CLI
# (PIN) and runs the grammar's own test corpus with it. The Windows twin of
# generate.sh; needs Node.js and npm. The parser the CLI writes for its test
# run is removed after: build.rs generates it at build time (ADR-0012).
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$pin = (Get-Content PIN -Raw).Trim()
try {
    npx -y "tree-sitter-cli@$pin" generate
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    npx -y "tree-sitter-cli@$pin" test
    exit $LASTEXITCODE
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue src/parser.c, src/node-types.json, src/tree_sitter
}
