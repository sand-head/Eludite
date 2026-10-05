; Razor highlight query for eludite-editor (brief 0056), for `.razor` and `.cshtml`.
;
; Concatenated after queries/csharp/highlights.scm, so the C# patterns come first: when two patterns capture the same
; node the earlier one wins, and C# must win inside code (`@code`, `@{ }`, `@(...)`, directive attribute values). The
; capture scheme is Eludite's, matching the HTML query's (queries/html/highlights.scm): tag names as `@tag`, attribute
; names as `@tag.attribute`, values as `@string`. The node names and the `at_*` aliases are tree-sitter-razor's
; (grammars/razor, MIT); the aliases are drawn as keywords, `@` included, as Visual Studio draws a Razor transition and
; its keyword in the keyword color (the library's own queries/highlights.scm splits them over several capture names).
; `<script>` and `<style>` bodies are highlighted by the JavaScript and CSS queries (razor/injections.scm).

;; Markup

(tag_name) @tag
(attribute_name) @tag.attribute
(attribute_value) @string
; A markup attribute value's quotes (anonymous nodes of the element), drawn with the value as HTML's are.
(element ["\"" "'"] @string)
(script_element ["\"" "'"] @string)
(style_element ["\"" "'"] @string)
(entity) @string.escape
(doctype) @keyword

[
  (razor_comment)
  (html_comment)
] @comment

;; Directive attributes: `@bind-Value`, `@onclick`, `@key`, `@ref`, the `@` included.

(razor_attribute_name) @tag.attribute

;; Directives, transitions and control structures

[
  "at_page"
  "at_using"
  "at_model"
  "at_rendermode"
  "at_inject"
  "at_implements"
  "at_layout"
  "at_inherits"
  "at_attribute"
  "at_typeparam"
  "at_namespace"
  "at_preservewhitespace"
  "at_block"
  "at_at_escape"
  "at_colon_transition"
  "at_addtaghelper"
  "at_removetaghelper"
  "at_taghelperprefix"
  "at_template"
  "at_lock"
  "at_section"
  "at_if"
  "at_switch"
  "at_for"
  "at_foreach"
  "at_while"
  "at_do"
  "at_try"
  "at_implicit"
  "at_explicit"
] @keyword

; A render mode by name; `@rendermode @(...)` is C#, highlighted by the C# query.
(razor_rendermode
  [
    "InteractiveServer"
    "InteractiveWebAssembly"
    "InteractiveAuto"
    (identifier)
    (qualified_name)
    (generic_name)
    (alias_qualified_name)
  ] @type)
