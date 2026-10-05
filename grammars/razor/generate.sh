#!/usr/bin/env bash
# Regenerates src/grammar.json from grammar.js with the pinned tree-sitter CLI
# (PIN) and runs the grammar's own test corpus with it. The only way
# src/grammar.json changes. Needs Node.js and npm (the CLI is fetched by npx);
# nothing is installed into the repository.
#
# The CLI also writes src/parser.c, src/node-types.json and src/tree_sitter/
# and its test run needs them; they are removed after, because build.rs
# generates them from src/grammar.json at build time with the same generator
# (ADR-0012), so they are never checked in.
set -euo pipefail
cd "$(dirname "$0")"
pin="$(tr -d '[:space:]' < PIN)"
cleanup() { rm -rf src/parser.c src/node-types.json src/tree_sitter; }
trap cleanup EXIT
npx -y "tree-sitter-cli@${pin}" generate
npx -y "tree-sitter-cli@${pin}" test
