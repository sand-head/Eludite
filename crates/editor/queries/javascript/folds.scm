; JavaScript fold query for eludite-editor (brief 0067), also the base of TypeScript's; kinds as in
; queries/csharp/folds.scm. `// #region` folds come from the text.

(comment) @fold.comment
(import_statement) @fold.imports

[
  (class_declaration)
  (class)
] @fold.type

[
  (function_declaration)
  (generator_function_declaration)
  (method_definition)
] @fold.member

[
  (if_statement)
  (for_statement)
  (for_in_statement)
  (while_statement)
  (do_statement)
  (switch_statement)
  (try_statement)
  (arrow_function)
  (function_expression)
  (object)
  (array)
  (template_string)
] @fold
