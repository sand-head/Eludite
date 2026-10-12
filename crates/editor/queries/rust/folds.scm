; Rust fold query for eludite-editor (brief 0067); kinds as in queries/csharp/folds.scm. `// region:` folds come from
; the text.

[
  (line_comment)
  (block_comment)
] @fold.comment

(use_declaration) @fold.imports

(mod_item) @fold.namespace

[
  (struct_item)
  (enum_item)
  (union_item)
  (trait_item)
  (impl_item)
] @fold.type

[
  (function_item)
  (macro_definition)
] @fold.member

[
  (if_expression)
  (for_expression)
  (while_expression)
  (loop_expression)
  (match_expression)
  (closure_expression)
  (unsafe_block)
  (async_block)
  (macro_invocation)
  (struct_expression)
] @fold
