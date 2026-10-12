; Razor fold query for eludite-editor (brief 0067): the markup's elements and Razor's blocks, plus the C# inside them
; as queries/csharp/folds.scm folds it. `#region` folds come from the text.

[
  (comment)
  (razor_comment)
] @fold.comment

[
  (razor_using_directive)
  (using_directive)
] @fold.imports

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
  (property_declaration)
  (razor_section)
] @fold.member

[
  (element)
  (script_element)
  (style_element)
  (razor_block)
  (razor_if)
  (razor_for)
  (razor_foreach)
  (razor_while)
  (razor_do_while)
  (razor_switch)
  (razor_try)
  (razor_lock)
  (if_statement)
  (for_statement)
  (foreach_statement)
  (while_statement)
  (switch_statement)
  (try_statement)
  (lambda_expression)
] @fold
