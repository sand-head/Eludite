; JSON highlight query for eludite-editor (brief 0050), for JSON and JSON with comments (`.jsonc`, `tsconfig.json`):
; tree-sitter-json's grammar accepts comments.
;
; Adapted from queries/highlights.scm in tree-sitter-json 0.24.8 (MIT, Copyright (c) 2014 Max Brunsfeld). Changes:
; keys captured as `@property.key` (Visual Studio draws them apart from string values); it comes first, so it wins
; over `(string) @string` for the same node under Eludite's precedence rule.

(pair
  key: (_) @property.key)

(string) @string

(number) @number

[
  (null)
  (true)
  (false)
] @constant.builtin

(escape_sequence) @string.escape

(comment) @comment
