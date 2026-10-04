; JSX highlight query for eludite-editor (brief 0050): placed before the JavaScript query (and, for TSX, after the
; TypeScript one), so its captures win for the same node under Eludite's precedence rule (an attribute name is not a
; plain property). Adapted from queries/highlights-jsx.scm in tree-sitter-javascript 0.25.0 (MIT, Copyright (c) 2014
; Max Brunsfeld): element names as tags and attribute names as `@tag.attribute`, the bracket captures dropped
; (default text color).

(jsx_opening_element (identifier) @tag (#match? @tag "^[a-z][^.]*$"))
(jsx_closing_element (identifier) @tag (#match? @tag "^[a-z][^.]*$"))
(jsx_self_closing_element (identifier) @tag (#match? @tag "^[a-z][^.]*$"))

(jsx_attribute (property_identifier) @tag.attribute)
