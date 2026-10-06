; F# signature file (`.fsi`) highlight query for eludite-editor (brief 0057).
;
; Adapted from fsharp_signature/queries/highlights.scm in tree-sitter-fsharp
; 0.3.12 (MIT, Copyright (c) 2023 Nikolaj Sidorenco), restricted to the nodes
; the signature grammar produces (no `_type`, `_pattern` or expression
; supertypes), with the same changes as queries/fsharp/highlights.scm: the
; editor's capture names, the precedence rule (the earlier pattern wins on the
; same node), and the `@variable`, `@spell`, `@character.special`, punctuation
; and operator captures dropped.

;; Comments

(xml_doc) @comment.documentation

[
  (line_comment)
  (block_comment)
] @comment

(preproc_inactive) @comment

;; Literals

[
  (xint)
  (int)
  (int16)
  (uint16)
  (int32)
  (uint32)
  (int64)
  (uint64)
  (nativeint)
  (unativeint)
  (ieee32)
  (ieee64)
  (float)
  (decimal)
] @number

[
  (bool)
  (unit)
  "null"
] @constant.builtin

[
  (string)
  (triple_quoted_string)
  (verbatim_string)
  (char)
  (format_string)
  (format_triple_quoted_string)
] @string

;; Directives

(compiler_directive_decl) @preprocessor
(fsi_directive_decl) @preprocessor
(fsi_directive_decl (string) @string)
(preproc_line "#line" @preprocessor)
(preproc_if
  [
    "#if" @preprocessor
    "#endif" @preprocessor
  ]
  condition: (_)? @preprocessor)
(preproc_else "#else" @preprocessor)

;; Attributes

(attribute
  target: (identifier)? @keyword)
(attribute (simple_type (long_identifier (identifier) @attribute)))
(attribute (simple_type) @attribute)

;; Types

(type_name type_name: (_) @type)
(exception_definition exception_name: (_) @type)

((simple_type
   (long_identifier
     (identifier) @type.builtin))
 (#any-of? @type.builtin "bool" "byte" "sbyte" "int16" "uint16" "int" "uint" "int64" "uint64" "nativeint" "unativeint" "decimal" "float" "double" "float32" "single" "char" "string" "unit" "obj" "exn"))

(simple_type (long_identifier (identifier) @type))

[
 (atomic_type)
 (simple_type)
 (generic_type)
 (function_type)
 (compound_type)
 (postfix_type)
 (list_type)
 (static_type)
 (constrained_type)
 (flexible_type)
 (byref_type)
 (paren_type)
 (struct_type)
] @type

(type_argument) @type
(measure_atom (simple_type) @type)

(union_type_case (identifier) @type)
(enum_type_case . (identifier) @type)
(union_type_field . (identifier) @property (_))

;; Modules and namespaces

(fsi_directive_decl . (string) @module)
(import_decl . (_) @module)
(named_module name: (_) @module)
(namespace name: (_) @module)
(module_defn . (_) @module)
(namespace name: (long_identifier (identifier) @module))
(named_module name: (long_identifier (identifier) @module))
(import_decl (long_identifier (identifier) @module))
(module_abbrev (long_identifier (identifier) @module))

;; Members, values and parameters

(member_signature
  .
  (identifier) @function)

(member_signature
  (curried_spec
    (arguments_spec
      (argument_spec
        (argument_name_spec
          name: (_) @variable.parameter)))))

(record_fields
  (record_field
    .
    (identifier) @property))

(primary_constr_args (_) @variable.parameter)

(class_as_reference
  (_) @variable.builtin)

(function_declaration_left
  . (_) @function)

(argument_patterns) @variable.parameter
(typed_pattern
  . (_) @variable.parameter)

(member_defn
  (method_or_prop_defn
    (property_or_ident . (identifier) @function .)
    args: (_)* @variable.parameter))

(member_defn
  (method_or_prop_defn
    (property_or_ident
      instance: (identifier) @variable.builtin
      method: (identifier) @function)
    args: (_)* @variable.parameter))

;; Keywords

[
  "when"
  "then"
  "and"
  "or"
  "not"
  "open"
  "abstract"
  "delegate"
  "extern"
  "static"
  "inline"
  "mutable"
  "override"
  "rec"
  "global"
  "let"
  "member"
  "enum"
  "type"
  "exception"
  "inherit"
  "interface"
  "class"
  "struct"
  "as"
  "begin"
  "end"
  "default"
  "do"
  "get"
  "set"
  "new"
  "of"
  "val"
  "module"
  "namespace"
  "with"
  (access_modifier)
] @keyword

;; Identifiers

((identifier) @module
 (#any-of? @module "Array" "Async" "Directory" "File" "List" "Option" "Path" "Map" "Set" "Lazy" "Seq" "Task" "String" "Result" ))
