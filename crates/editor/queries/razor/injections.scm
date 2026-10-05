; Razor injections for eludite-editor (brief 0056), in the shape of queries/html/injections.scm: the text of a
; `<script>` element is JavaScript, that of a `<style>` element CSS (tree-sitter-razor parses both bodies as
; `raw_text`, never as markup).

((script_element
  (raw_text) @injection.content)
 (#set! injection.language "javascript"))

((style_element
  (raw_text) @injection.content)
 (#set! injection.language "css"))
