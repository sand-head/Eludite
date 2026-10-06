#!/bin/sh
# Record one OpenAI-compatible server's `GET /models` and one streamed chat completion that offers a tool, as test
# fixtures: tests/fixtures/NAME.models.json and tests/fixtures/NAME.sse.
#
#   tools/record.sh NAME BASE_URL MODEL
#
# The key, when the server needs one, is read from ELUDITE_OPENAI_API_KEY and sent as a header; it is never written
# to the files (they hold the server's answers only). Real model calls: keep them few.
set -eu
name=$1 base=${2%/} model=$3
dir=$(cd "$(dirname "$0")/../tests/fixtures" && pwd)
auth=""
if [ -n "${ELUDITE_OPENAI_API_KEY:-}" ]; then auth="Authorization: Bearer $ELUDITE_OPENAI_API_KEY"; fi
curl -sS --fail-with-body ${auth:+-H "$auth"} "$base/models" > "$dir/$name.models.json"
body=$(cat <<JSON
{"model": "$model", "stream": true, "stream_options": {"include_usage": true}, "max_tokens": 512,
 "messages": [{"role": "system", "content": "You are a coding agent inside an IDE. Use the tools."},
              {"role": "user", "content": "Read notes.txt with the eludite-file-read tool."}],
 "tools": [{"type": "function", "function": {"name": "eludite-file-read", "description": "Read a file with line numbers.",
   "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}}]}
JSON
)
curl -sS -N --fail-with-body ${auth:+-H "$auth"} -H "Content-Type: application/json" \
  -d "$body" "$base/chat/completions" > "$dir/$name.sse"
if [ -n "${ELUDITE_OPENAI_API_KEY:-}" ] && grep -qF "$ELUDITE_OPENAI_API_KEY" "$dir/$name.sse" "$dir/$name.models.json"; then
  echo "the key appears in a recording; not keeping it" >&2
  rm -f "$dir/$name.sse" "$dir/$name.models.json"
  exit 1
fi
echo "recorded $dir/$name.models.json and $dir/$name.sse"
