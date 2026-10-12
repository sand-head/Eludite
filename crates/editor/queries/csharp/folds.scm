; C# fold query for eludite-editor (brief 0067). Capture names are the fold kinds (crates/editor/src/folding.rs):
; @fold.namespace, @fold.type, @fold.member, @fold.comment and @fold.imports (consecutive ones merge), @fold for any
; other block. A node with a `name` field folds from the name's row, so attributes above a member stay outside.
; `#region` folds come from the text, not from this query.

(comment) @fold.comment
(using_directive) @fold.imports

(namespace_declaration) @fold.namespace

[
  (class_declaration)
  (struct_declaration)
  (interface_declaration)
  (enum_declaration)
  (record_declaration)
] @fold.type

[
  (method_declaration)
  (constructor_declaration)
  (destructor_declaration)
  (operator_declaration)
  (conversion_operator_declaration)
  (property_declaration)
  (indexer_declaration)
  (event_declaration)
  (delegate_declaration)
] @fold.member

[
  (accessor_declaration)
  (local_function_statement)
  (if_statement)
  (for_statement)
  (foreach_statement)
  (while_statement)
  (do_statement)
  (switch_statement)
  (try_statement)
  (using_statement)
  (lock_statement)
  (lambda_expression)
  (anonymous_method_expression)
  (initializer_expression)
  (switch_expression)
] @fold
