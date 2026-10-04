; HTML injections for eludite-editor (brief 0050), from queries/injections.scm in tree-sitter-html 0.23.2 (MIT): the
; text of a `<script>` element is JavaScript, that of a `<style>` element CSS.

((script_element
  (raw_text) @injection.content)
 (#set! injection.language "javascript"))

((style_element
  (raw_text) @injection.content)
 (#set! injection.language "css"))
