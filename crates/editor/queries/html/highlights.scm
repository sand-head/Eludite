; HTML highlight query for eludite-editor (brief 0050).
;
; Adapted from queries/highlights.scm in tree-sitter-html 0.23.2 (MIT, Copyright (c) 2014 Max Brunsfeld). Changes:
; attribute names captured as `@tag.attribute` (Visual Studio draws them apart from C# attributes), the doctype as a
; keyword, the bracket captures dropped (default text color). `<script>` and `<style>` contents are highlighted by the
; JavaScript and CSS queries (the injections in html/injections.scm).

(comment) @comment
(doctype) @keyword
(tag_name) @tag
(erroneous_end_tag_name) @tag
(attribute_name) @tag.attribute
(attribute_value) @string
(quoted_attribute_value) @string
(entity) @string.escape
