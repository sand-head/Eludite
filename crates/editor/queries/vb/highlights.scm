; Visual Basic highlight query for eludite-editor (brief 0057).
;
; Written for this repository against tree-sitter-vb-dotnet 0.1.0 (MIT,
; Copyright (c) 2025 CodeAnt AI), which ships no queries. It follows the C#
; query's conventions: when two patterns capture the same node, the earlier one
; wins; a capture inside another capture's node paints over it; identifiers,
; punctuation and operators that carry no capture render in the default color.
;
; The grammar defines its keywords (`Class`, `End`, `Sub`, `If`, `Dim`, ...)
; as hidden case-insensitive regex tokens, so they are not nodes in the tree
; and no pattern can capture them. Only `modifier` (`Public`, `Shared`,
; `Overrides`, `Async`, ...) is a visible node; the statement keywords stay
; in the default color until the grammar exposes them.

;; Comments: `'''` XML documentation first, then every other comment.

((comment) @comment.documentation
  (#match? @comment.documentation "^'''"))

(comment) @comment

;; Literals

(string_literal) @string
(interpolated_string_literal) @string
(character_literal) @string
; An interpolation's expression is not string; its own captures paint over.
(interpolation) @embedded

[
  (integer_literal)
  (floating_point_literal)
  (date_literal)
] @number

(boolean_literal) @constant.builtin

; `Nothing` is a hidden token: the only `literal` with no child.
((literal) @constant.builtin
  (#match? @constant.builtin "^[Nn][Oo][Tt][Hh][Ii][Nn][Gg]$"))

;; Preprocessor lines (`#If`, `#Region`, `#Const` inside a procedure)

(preprocessor_directive) @preprocessor

;; The current instance

((identifier) @variable.builtin
  (#match? @variable.builtin "^(?i)(me|mybase|myclass)$"))

;; Declarations

(namespace_block name: (namespace_name) @module)
(imports_statement namespace: (namespace_name) @module)

(class_block name: (identifier) @type)
(module_block name: (identifier) @type)
(structure_block name: (identifier) @type)
(interface_block name: (identifier) @type)
(enum_block name: (identifier) @type)
(delegate_declaration name: (identifier) @type)
(type_parameter name: (identifier) @type)

(method_declaration name: (identifier) @function)
(property_declaration name: (identifier) @property)
(event_declaration name: (identifier) @property)
(enum_member name: (identifier) @constant)
(const_declaration name: (identifier) @constant)

(parameter name: (identifier) @variable.parameter)
(lambda_parameter name: (identifier) @variable.parameter)
(argument name: (identifier) @variable.parameter)

(label_statement label: (identifier) @label)
(goto_statement label: (identifier) @label)

;; Types in use

(primitive_type) @type.builtin
(type (namespace_name) @type)
(generic_type (namespace_name) @type)
(array_type (namespace_name) @type)

;; Calls and members

(invocation target: (identifier) @function)
(invocation target: (member_access member: (identifier) @function))
(member_initializer member: (identifier) @property)
(object_initializer "." (identifier) @property)

;; Attributes

(attribute name: (identifier) @attribute)
(attribute name: (namespace_name) @attribute)

;; Keywords: the modifiers are the only keyword nodes the grammar exposes.

(modifier) @keyword
