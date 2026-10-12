; F# fold query for eludite-editor (brief 0067); kinds as in queries/csharp/folds.scm. `#region` folds come from the
; text.

[
  (line_comment)
  (block_comment)
] @fold.comment

(import_decl) @fold.imports

[
  (namespace)
  (named_module)
] @fold.namespace

[
  (module_defn)
  (type_definition)
] @fold.type

[
  (member_defn)
  (function_or_value_defn)
] @fold.member
