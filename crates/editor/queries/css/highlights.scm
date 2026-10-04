; CSS highlight query for eludite-editor (brief 0050), also used for SCSS and Less (the CSS grammar parses their
; common subset; brief 0050 pins no SCSS or Less grammar).
;
; Adapted from queries/highlights.scm in tree-sitter-css 0.25.0 (MIT, Copyright (c) 2018 Max Brunsfeld). Changes:
; selectors (tags, classes, ids, the nesting and universal selectors) captured as `@selector` and property names as
; `@property.key`, as Visual Studio colors them; custom properties (`--x`) captured before the plain property pattern
; for Eludite's precedence rule (the earlier pattern wins for the same node); the punctuation captures dropped.

(comment) @comment

((property_name) @variable
 (#match? @variable "^--"))
((plain_value) @variable
 (#match? @variable "^--"))

(pseudo_element_selector (tag_name) @attribute)
(pseudo_class_selector (class_name) @attribute)

(tag_name) @selector
(nesting_selector) @selector
(universal_selector) @selector
(class_name) @selector
(id_name) @selector
(namespace_name) @selector

(property_name) @property.key
(feature_name) @property.key
(attribute_name) @attribute
(attribute_selector (plain_value) @string)

(function_name) @function

"@media" @keyword
"@import" @keyword
"@charset" @keyword
"@namespace" @keyword
"@supports" @keyword
"@keyframes" @keyword
(at_keyword) @keyword
(to) @keyword
(from) @keyword
(important) @keyword

"and" @operator
"or" @operator
"not" @operator
"only" @operator

(string_value) @string
(color_value) @number

(integer_value) @number
(float_value) @number
(unit) @number
