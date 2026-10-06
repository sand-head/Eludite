; F# highlight query for eludite-editor (brief 0057).
;
; Adapted from queries/highlights.scm in tree-sitter-fsharp 0.3.12 (MIT,
; Copyright (c) 2023 Nikolaj Sidorenco). Changes: capture names mapped to the
; editor's (`@keyword.*` to `@keyword`, `@module` for namespaces, `@boolean`
; to `@constant.builtin`, `@number.float` to `@number`, directives to
; `@preprocessor`), patterns reordered for Eludite's precedence rule (when two
; patterns capture the same node, the earlier one wins; a capture inside
; another capture's node paints over it; the upstream file resolves the other
; way round), and the `@variable`, `@variable.member`, `@spell`,
; `@character.special`, punctuation and operator captures dropped (all render
; in the default text color, like the C# query's punctuation).

;; Comments: `///` documentation first, then every other comment.

(xml_doc) @comment.documentation

[
  (line_comment)
  (block_comment)
] @comment

; Inactive branch of a directive the grammar could not place structurally.
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

; An interpolation's expression is not string; its own captures paint over.
(format_string_eval) @embedded

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
(attribute (_type) @attribute)

;; Types and definitions

(type_name type_name: (_) @type)
(exception_definition exception_name: (_) @type)

; The built-in type names, before the generic type rule on the same node.
((simple_type
   (long_identifier
     (identifier) @type.builtin))
 (#any-of? @type.builtin "bool" "byte" "sbyte" "int16" "uint16" "int" "uint" "int64" "uint64" "nativeint" "unativeint" "decimal" "float" "double" "float32" "single" "char" "string" "unit" "obj" "exn"))

(simple_type (long_identifier (identifier) @type))

[
 (_type)
 (atomic_type)
] @type

; Type parameters: `'T` / `^a` in declarations, constraints and annotations.
(type_argument) @type

; Units of measure: `1.0<m/s>`, `float<kg>`.
(measure_atom (simple_type) @type)

; Union and enum cases are constructors (`| Circle of ...`, `| A = 1`).
(union_type_case (identifier) @type)
(enum_type_case . (identifier) @type)

; Named union-case fields: `| Circle of radius: float`.
(union_type_field . (identifier) @property (_))

;; Modules and namespaces

(fsi_directive_decl . (string) @module)
(import_decl . (_) @module)
(named_module name: (_) @module)
(namespace name: (_) @module)
; The name is the identifier child, not the first child: a nested module may
; carry attributes (`[<AutoOpen>] module Inner = ...`).
(module_defn (identifier) @module)
(namespace name: (long_identifier (identifier) @module))
(named_module name: (long_identifier (identifier) @module))
(import_decl (long_identifier (identifier) @module))
(module_abbrev (long_identifier (identifier) @module))

;; Members and properties

; The member named by an SRTP constraint: `(static member Zero : ^a)`.
(trait_member_constraint (identifier) @function)

(member_signature
  .
  (identifier) @function)

(member_signature
  (curried_spec
    (arguments_spec
      (argument_spec
        (argument_name_spec
          name: (_) @variable.parameter)))))

(field_initializer
  field: (_) @property)

(record_fields
  (record_field
    .
    (identifier) @property))

; A computation expression's builder: `async { }`, `task { }`, `seq { }`.
(ce_expression
  .
  (_) @function)

;; Functions, parameters and members at their declarations

; Before the declaration rules below, so `let private f x` keeps `private` a
; keyword where a rule's first-child capture would also take it.
(access_modifier) @keyword

(primary_constr_args (_) @variable.parameter)

(class_as_reference
  (_) @variable.builtin)

(function_declaration_left
  . (_) @function)
; `let rec private fib n`: the name follows the access modifier.
(function_declaration_left
  (access_modifier) . (_) @function)

(argument_patterns) @variable.parameter
(typed_pattern
  (_pattern) @variable.parameter
  (_type) @type)

; A member name has two mutually-exclusive shapes. Bare `member M(x)` -> M is
; the method; instance `member this.M` -> `this` is the self parameter, M the
; method.
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

; Auto-property: `member val P = 1 with get, set` has no method_or_prop_defn.
(member_defn
  (property_or_ident (identifier) @function))

; `[<Literal>] let maxSize = 100`: the bound name is a constant.
((value_declaration
   (attributes
     (attribute
       (simple_type
         (long_identifier
           (identifier) @attribute))))
   (function_or_value_defn
     (value_declaration_left
       .
       (identifier_pattern (long_identifier_or_op (identifier) @constant)))))
 (#eq? @attribute "Literal"))

((declaration_expression
   (attributes
     (attribute
       (simple_type
         (long_identifier
           (identifier) @attribute))))
   (function_or_value_defn
     (value_declaration_left
       .
       (identifier_pattern (long_identifier_or_op (identifier) @constant)))))
 (#eq? @attribute "Literal"))

;; Keywords

[
  "if"
  "then"
  "else"
  "elif"
  "when"
  "match"
  "match!"
  "and"
  "or"
  "not"
  "upcast"
  "downcast"
  "return"
  "return!"
  "yield"
  "yield!"
  "for"
  "while"
  "downto"
  "to"
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
  "let!"
  "use"
  "use!"
  "and!"
  "member"
  "enum"
  "type"
  "exception"
  "inherit"
  "interface"
  "class"
  "struct"
  "as"
  "assert"
  "begin"
  "end"
  "done"
  "default"
  "in"
  "do"
  "do!"
  "fun"
  "function"
  "get"
  "set"
  "lazy"
  "new"
  "of"
  "val"
  "module"
  "namespace"
  "with"
  "try"
  "finally"
  (access_modifier)
] @keyword

;; Identifiers in expressions
;;
;; Everything from here on captures identifier nodes that other rules may also
;; capture, so the order follows the precedence rule: specific first.

; `base.M(...)`, `this.M(...)`: the current object. `base` is a keyword in F#;
; `this`/`self` are conventional self-identifier names.
((long_identifier . (identifier) @variable.builtin)
 (#any-of? @variable.builtin "base" "this" "self"))
((long_identifier_or_op (identifier) @variable.builtin)
 (#any-of? @variable.builtin "base" "this" "self"))

((identifier) @keyword
 (#any-of? @keyword "failwith" "failwithf" "raise" "reraise"))

; `not` is a library function, but reads as an operator like `&&`; the
; keyword list above only covers the places the grammar lexes it as a token.
((long_identifier_or_op (identifier) @keyword)
 (#eq? @keyword "not"))

; Intrinsics that take a type argument (`typeof<int>`, `sizeof<T>`) and
; `nameof`; reserved names, so matching the identifier anywhere is safe.
((identifier) @function.builtin
 (#any-of? @function.builtin "typeof" "typedefof" "sizeof" "nameof"))

((identifier) @module
 (#any-of? @module "Array" "Async" "Directory" "File" "List" "Option" "Path" "Map" "Set" "Lazy" "Seq" "Task" "String" "Result" ))

; A member of a compound expression (`(f x).Name`, `arr.[0].Length`,
; `this.Id.GetHashCode()`): a call when applied, else a property. Before the
; capitalised-name rule below, which would otherwise take a `.Member` head.
(application_expression
  .
  (dot_expression
    field: (long_identifier_or_op
      (identifier) @function))
  .
  (_))

(dot_expression
  field: (long_identifier_or_op
    (identifier) @property))

; A capitalised first segment of a dotted path is a type or module
; (`Customer.Create`, `Status.Draft`, `Math.PI`); after the module rules, so
; `List.map` and `open System.IO` keep theirs.
((long_identifier . (identifier) @type (identifier))
 (#match? @type "^[A-Z]"))

; A capitalised name used on its own, as an application head (`Some x`,
; `Ok value`, `Circle 1.0`) or as a pattern head (`| Some v ->`) is a union
; case, exception or active-pattern case by F# convention. Module- and
; type-qualified heads (`List.map`, `Foo.Bar`) are excluded by the anchors
; and keep their own colours. Before the call rules so it wins for heads.
((long_identifier_or_op . (identifier) @type .)
 (#match? @type "^[A-Z]"))

; A pattern applying a case to an argument (`| Some v ->`): the head is the case.
(identifier_pattern
  .
  (_) @type
  .
  (_))

; The head of an application is the function being called: a bare name
; (`f x`) or the last segment of a qualified one (`List.map f`, `x.M y`).
(application_expression
  .
  (long_identifier_or_op
    (identifier) @function)
  .
  (_))

(application_expression
  .
  (long_identifier_or_op
    (long_identifier
      (identifier) @function .))
  .
  (_))

(application_expression
  .
  (typed_expression
    (long_identifier_or_op
      (identifier) @function)
    (_))
  .
  (_))

(application_expression
  .
  (typed_expression
    (long_identifier_or_op
      (long_identifier
        (identifier) @function .))
    (_))
  .
  (_))

(application_expression
  .
  (typed_expression
    (dot_expression
      field: (long_identifier_or_op
        (identifier) @function))
    (_))
  .
  (_))

; The function side of a pipe or composition (`x |> f`, `x |> List.map g`,
; `f >> string`, `xs |> Seq.sum`): applied even though it is not the head of
; an application node (the grammar parses `xs |> List.map g` as
; `(xs |> List.map) g`, so the bare qualified-name alternatives matter).
((infix_expression
  .
  (_)
  .
  (infix_op) @_op
  .
  [
    (long_identifier_or_op (identifier) @function)
    (long_identifier_or_op (long_identifier (identifier) @function .))
    (application_expression . (long_identifier_or_op (identifier) @function))
    (application_expression . (long_identifier_or_op (long_identifier (identifier) @function .)))
    (application_expression . (dot_expression field: (long_identifier_or_op (identifier) @function)))
  ])
 (#any-of? @_op "|>" "||>" "|||>"))

((infix_expression
  .
  [
    (long_identifier_or_op (identifier) @function)
    (long_identifier_or_op (long_identifier (identifier) @function .))
    (application_expression . (long_identifier_or_op (identifier) @function))
    (application_expression . (long_identifier_or_op (long_identifier (identifier) @function .)))
    (application_expression . (dot_expression field: (long_identifier_or_op (identifier) @function)))
  ]
  .
  (infix_op) @_op
  .
  (_))
 (#any-of? @_op "<|" "<||" "<|||"))

((infix_expression
  .
  [
    (long_identifier_or_op (identifier) @function)
    (long_identifier_or_op (long_identifier (identifier) @function .))
    (application_expression . (long_identifier_or_op (identifier) @function))
    (application_expression . (long_identifier_or_op (long_identifier (identifier) @function .)))
  ]
  .
  (infix_op) @_op
  .
  [
    (long_identifier_or_op (identifier) @function)
    (long_identifier_or_op (long_identifier (identifier) @function .))
    (application_expression . (long_identifier_or_op (identifier) @function))
    (application_expression . (long_identifier_or_op (long_identifier (identifier) @function .)))
  ])
 (#any-of? @_op ">>" "<<"))

; The last segment of a value-rooted path is a property (`ex.Message`,
; `item.Tags.Count`); a call head (`x.M y`) was captured above.
((long_identifier_or_op
   (long_identifier . (identifier) @_root (identifier) @property .))
 (#match? @_root "^[a-z_]")
 (#not-any-of? @_root "this" "base" "self"))

; `query { ... }` custom operations whose names are unambiguous.
((application_expression
   .
   (long_identifier_or_op (identifier) @keyword))
 (#any-of? @keyword
   "leftOuterJoin" "groupJoin" "groupValBy"
   "sortByDescending" "thenBy" "thenByDescending"
   "sortByNullable" "sortByNullableDescending"
   "thenByNullable" "thenByNullableDescending"
   "sumByNullable" "minByNullable" "maxByNullable" "averageByNullable"))

((sequential_expression (long_identifier_or_op (identifier) @keyword))
 (#any-of? @keyword
   "headOrDefault" "lastOrDefault" "exactlyOne" "exactlyOneOrDefault"))
