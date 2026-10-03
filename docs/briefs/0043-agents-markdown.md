# Brief 0043: Markdown in the Agents window's transcript

Status: done (2026-10-03, Linux)
Phase: 2
Plan reference: PLAN.md sections 2 (principles 1, 3), 5.2, 8
Depends on: brief 0016 (the Agents window), brief 0013 (`crates/ui::markdown`, the tooltips' Markdown)

## Goal

The agent's messages in the Agents window render as Markdown: headings, paragraphs with bold, italic, strikethrough,
code spans and links, fenced code blocks, bullet, numbered and task lists (nested), block quotes, rules and GitHub
tables. Until now each line showed as plain text with the marks in it. The tooltips get the same parser.

## Files in scope

- `Cargo.toml` (`pulldown-cmark`), `crates/ui/Cargo.toml`, `crates/ui/src/markdown.rs` (the parser on
  `pulldown-cmark`, the block model, plain text, the renderer, `block_starts`), `crates/ui/src/transcript.rs`
  (`agent_block` replaces `agent_line`), `crates/ui/src/lib.rs` (docs).
- `crates/eludite/src/shell/agents/transcript.rs` (one row per top-level block), `crates/eludite/src/shell/agents/window.rs`.
- `docs/briefs/README.md`, this file.

## Contract

- Markdown is parsed by `pulldown-cmark` (MIT) with tables, strikethrough and task lists. HTML shows as its text,
  except `<br>`; images show their alt text; links show their text in the accent color (not clickable yet).
- Agent text is one transcript row per top-level block. A chunk is appended to the open (last) block, which is parsed
  again and split where new blocks start; every block but the last is final. Streaming re-measures and re-parses
  only the open block, so the virtualized list keeps brief 0005's cost per chunk. Each row keeps its source exactly as
  streamed, so `agent_message` and the transcript's JSON record are unchanged.
- The streamed rows parse to the same blocks as parsing the whole message at once.
- `plain_text` (what an agent reads from a tooltip) lists items with `•` or `1.`, quotes with `> `, table cells with
  ` | `; rules are left out.

## Proving test

- `eludite-ui` `markdown` tests: the Roslyn hover and the old subset still read the same, nested lists, quotes, task
  lists, tables, strikethrough, HTML, and `block_starts` on paragraphs, fences (an unclosed fence is one block), lists
  and rules.
- `eludite` `agents::transcript` tests: a message streamed three bytes at a time (a heading, a list, a fenced block
  with a blank line in it, a table) gives five rows whose blocks equal the whole message's, and `agent_message` equals
  the source; only the open block is dirty while it streams.

## Out of scope

- Clickable links, syntax highlighting in code blocks, copying a code block, selecting transcript text.
- Markdown in the user's prompts and in tool call results.

## Dependency

`pulldown-cmark` 0.13, SPDX `MIT`, default features off (no binary, no HTML writer, no SIMD).
