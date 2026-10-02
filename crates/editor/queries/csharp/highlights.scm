; C# highlight query for niello-editor.
;
; Adapted from queries/highlights.scm in tree-sitter-c-sharp 0.23.5
; (MIT, Copyright (c) 2014-2023 Max Brunsfeld, Damien Guard, Amaan Qureshi,
; and contributors). Changes: patterns reordered for Niello's precedence rule
; (when two patterns capture the same node, the earlier one wins; a capture
; inside another capture's node paints over it), the catch-all
; `(identifier) @variable` and punctuation captures dropped (both render in the
; default text color), and captures added for invocations, object creation,
; properties, using directives and preprocessor directives.

;; Comments and literals

(comment) @comment

[
  (character_literal)
  (string_literal)
  (raw_string_literal)
  (verbatim_string_literal)
  (interpolation_start)
  (interpolation_quote)
] @string

(string_literal_content) @string
(escape_sequence) @string.escape

[
  (real_literal)
  (integer_literal)
] @number

[
  (boolean_literal)
  (null_literal)
] @constant.builtin

;; Preprocessor

[
  (preproc_region)
  (preproc_endregion)
  (preproc_if)
  (preproc_elif)
  (preproc_else)
  (preproc_define)
  (preproc_undef)
  (preproc_pragma)
  (preproc_nullable)
  (preproc_warning)
  (preproc_error)
  (preproc_line)
] @preprocessor

;; Declarations

(method_declaration name: (identifier) @function)
(local_function_statement name: (identifier) @function)
(constructor_declaration name: (identifier) @type)
(destructor_declaration name: (identifier) @type)

(interface_declaration name: (identifier) @type)
(class_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(struct_declaration name: (identifier) @type)
(record_declaration name: (identifier) @type)
(delegate_declaration name: (identifier) @type)

(namespace_declaration name: (identifier) @module)
(namespace_declaration name: (qualified_name) @module)
(file_scoped_namespace_declaration name: (identifier) @module)
(file_scoped_namespace_declaration name: (qualified_name) @module)
(using_directive (identifier) @module)
(using_directive (qualified_name) @module)

(property_declaration name: (identifier) @property)
(enum_member_declaration name: (identifier) @constant)
(type_parameter name: (identifier) @type)
(type_parameter_constraints_clause (identifier) @type)

(parameter name: (identifier) @variable.parameter)

;; Types in use

(predefined_type) @type.builtin
(implicit_type) @keyword

(method_declaration returns: (identifier) @type)
(object_creation_expression type: (identifier) @type)
(object_creation_expression type: (generic_name (identifier) @type))
(generic_name (identifier) @type)
(type_argument_list (identifier) @type)
(base_list (identifier) @type)
(as_expression right: (identifier) @type)
(is_expression right: (identifier) @type)
(typeof_expression type: (identifier) @type)
(_ type: (identifier) @type)

;; Calls

(invocation_expression function: (identifier) @function)
(invocation_expression
  function: (member_access_expression name: (identifier) @function))
(invocation_expression
  function: (member_access_expression name: (generic_name (identifier) @function)))

;; Attributes

(attribute name: (identifier) @attribute)
(attribute name: (qualified_name) @attribute)

;; Keywords

[
  (modifier)
  "this"
] @keyword

[
  "add"
  "alias"
  "as"
  "base"
  "break"
  "case"
  "catch"
  "checked"
  "class"
  "continue"
  "default"
  "delegate"
  "do"
  "else"
  "enum"
  "event"
  "explicit"
  "extern"
  "finally"
  "for"
  "foreach"
  "global"
  "goto"
  "if"
  "implicit"
  "interface"
  "is"
  "lock"
  "namespace"
  "notnull"
  "operator"
  "params"
  "return"
  "remove"
  "sizeof"
  "stackalloc"
  "static"
  "struct"
  "switch"
  "throw"
  "try"
  "typeof"
  "unchecked"
  "using"
  "while"
  "new"
  "await"
  "in"
  "yield"
  "get"
  "set"
  "when"
  "out"
  "ref"
  "from"
  "where"
  "select"
  "record"
  "init"
  "with"
  "let"
] @keyword

;; Operators

[
  "--"
  "-"
  "-="
  "&"
  "&="
  "&&"
  "+"
  "++"
  "+="
  "<"
  "<="
  "<<"
  "<<="
  "="
  "=="
  "!"
  "!="
  "=>"
  ">"
  ">="
  ">>"
  ">>="
  ">>>"
  ">>>="
  "|"
  "|="
  "||"
  "?"
  "??"
  "??="
  "^"
  "^="
  "~"
  "*"
  "*="
  "/"
  "/="
  "%"
  "%="
  ".."
] @operator
