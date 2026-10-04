; TypeScript highlight query for eludite-editor (brief 0050), prepended to the JavaScript query (TypeScript's grammar
; extends JavaScript's, and so does its query: tree-sitter-typescript's highlights.scm only adds to
; tree-sitter-javascript's).
;
; Adapted from queries/highlights.scm in tree-sitter-typescript 0.23.2 (MIT, Copyright (c) 2017 GitHub). Changes: it
; comes first, so its captures win over the JavaScript query's for the same node under Eludite's precedence rule; the
; punctuation captures dropped.

; Types

(type_identifier) @type
(predefined_type) @type.builtin

; Parameters

(required_parameter (identifier) @variable.parameter)
(optional_parameter (identifier) @variable.parameter)

; Keywords

[ "abstract"
  "declare"
  "enum"
  "export"
  "implements"
  "interface"
  "keyof"
  "namespace"
  "private"
  "protected"
  "public"
  "type"
  "readonly"
  "override"
  "satisfies"
] @keyword
