; JavaScript highlight query for eludite-editor (brief 0050), also the base of the TypeScript and TSX queries.
;
; Adapted from queries/highlights.scm in tree-sitter-javascript 0.25.0 (MIT, Copyright (c) 2014 Max Brunsfeld).
; Changes: patterns reordered for Eludite's precedence rule (when two patterns capture the same node, the earlier one
; wins; a capture inside another capture's node paints over it), so function definitions and calls come before
; properties, and all-caps constants before constructors; the catch-all `(identifier) @variable` and the punctuation
; captures dropped (both render in the default text color); a template substitution captured as `@embedded` (the
; default text color) so its expression is not drawn as string; `#is-not? local` predicates removed (Eludite has no
; locals query).

; Comments and literals
;----------------------

(comment) @comment

[
  (string)
  (template_string)
] @string

(template_substitution) @embedded

(regex) @string.special
(number) @number

[
  (true)
  (false)
  (null)
  (undefined)
] @constant.builtin

(this) @variable.builtin
(super) @variable.builtin

; Function and method definitions
;--------------------------------

(function_expression
  name: (identifier) @function)
(function_declaration
  name: (identifier) @function)
(generator_function_declaration
  name: (identifier) @function)
(method_definition
  name: (property_identifier) @function.method)

(pair
  key: (property_identifier) @function.method
  value: [(function_expression) (arrow_function)])

(assignment_expression
  left: (member_expression
    property: (property_identifier) @function.method)
  right: [(function_expression) (arrow_function)])

(variable_declarator
  name: (identifier) @function
  value: [(function_expression) (arrow_function)])

(assignment_expression
  left: (identifier) @function
  right: [(function_expression) (arrow_function)])

; Function and method calls
;--------------------------

((identifier) @function.builtin
 (#eq? @function.builtin "require"))

(call_expression
  function: (identifier) @function)

(call_expression
  function: (member_expression
    property: (property_identifier) @function.method))

; Properties
;-----------

(property_identifier) @property

; Special identifiers
;--------------------

([
    (identifier)
    (shorthand_property_identifier)
    (shorthand_property_identifier_pattern)
 ] @constant
 (#match? @constant "^[A-Z_][A-Z\\d_]+$"))

((identifier) @constructor
 (#match? @constructor "^[A-Z]"))

((identifier) @variable.builtin
 (#match? @variable.builtin "^(arguments|module|console|window|document)$"))

; Operators and keywords
;-----------------------

[
  "-"
  "--"
  "-="
  "+"
  "++"
  "+="
  "*"
  "*="
  "**"
  "**="
  "/"
  "/="
  "%"
  "%="
  "<"
  "<="
  "<<"
  "<<="
  "="
  "=="
  "==="
  "!"
  "!="
  "!=="
  "=>"
  ">"
  ">="
  ">>"
  ">>="
  ">>>"
  ">>>="
  "~"
  "^"
  "&"
  "|"
  "^="
  "&="
  "|="
  "&&"
  "||"
  "??"
  "&&="
  "||="
  "??="
] @operator

[
  "as"
  "async"
  "await"
  "break"
  "case"
  "catch"
  "class"
  "const"
  "continue"
  "debugger"
  "default"
  "delete"
  "do"
  "else"
  "export"
  "extends"
  "finally"
  "for"
  "from"
  "function"
  "get"
  "if"
  "import"
  "in"
  "instanceof"
  "let"
  "new"
  "of"
  "return"
  "set"
  "static"
  "switch"
  "target"
  "throw"
  "try"
  "typeof"
  "var"
  "void"
  "while"
  "with"
  "yield"
] @keyword
