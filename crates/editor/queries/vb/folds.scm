; Visual Basic fold query for eludite-editor (brief 0067); kinds as in queries/csharp/folds.scm. `#Region` folds come
; from the text.

(comment) @fold.comment
(imports_statement) @fold.imports

(namespace_block) @fold.namespace

[
  (class_block)
  (module_block)
  (structure_block)
  (interface_block)
  (enum_block)
] @fold.type

[
  (method_declaration)
  (constructor_declaration)
  (property_declaration)
] @fold.member

[
  (get_accessor)
  (set_accessor)
  (if_statement)
  (for_statement)
  (for_each_statement)
  (while_statement)
  (do_statement)
  (select_case_statement)
  (try_statement)
  (using_statement)
  (with_statement)
  (sync_lock_statement)
  (lambda_expression)
] @fold
