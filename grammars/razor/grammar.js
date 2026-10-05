/**
 * @file Razor grammar for tree-sitter
 * @author Tristan Knight <admin@snappeh.com>
 * @license MIT
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// Vendored rather than taken from node_modules: `tree-sitter generate` then
// needs no npm install, so a downstream build can compile this def straight from
// a source tarball. See vendor/tree-sitter-c-sharp/package.json for provenance.
const CSHARP = require("./vendor/tree-sitter-c-sharp/grammar.js").default;

module.exports = grammar(CSHARP, {
  name: "razor",

  // U+FEFF is whitespace here so a file's leading byte order mark (which
  // Visual Studio writes on every `.cshtml` and `.razor` it creates) is an
  // extra like any other: `\s` does not include it in tree-sitter's regexes.
  extras: ($) => [$.razor_comment, $.comment, /[\s\uFEFF]+/],

  conflicts: ($, o) => [
    [
      $.preproc_if,
      $.preproc_if_in_top_level,
      $.preproc_if_in_expression,
      $.preproc_if_in_attribute_list,
    ],
    [$.preproc_if, $.preproc_if_in_top_level],
    [$.preproc_if, $.preproc_if_in_top_level, $.preproc_if_in_expression],
    [$.preproc_if, $.preproc_if_in_top_level, $.preproc_if_in_attribute_list],
    [
      $.preproc_else,
      $.preproc_else_in_top_level,
      $.preproc_else_in_expression,
      $.preproc_else_in_attribute_list,
    ],
    [$.declaration, $.preproc_if_in_top_level],
    [$.type_declaration, $.declaration],
    [$.method_declaration, $._local_function_declaration],
    [$.declaration, $.preproc_else_in_top_level],
    [
      $.preproc_elif,
      $.preproc_elif_in_top_level,
      $.preproc_elif_in_expression,
      $.preproc_elif_in_attribute_list,
    ],

    [$.destructor_declaration, $._simple_name],

    [$.field_declaration, $.local_declaration_statement],

    ...o,
  ],

  rules: {
    // Directives usually lead the file, but Razor takes them anywhere at the
    // top level (after a `@{ #pragma … }` block, say).
    compilation_unit: ($) => repeat(choice($._directive, $._node)),

    _directive: ($) =>
      choice(
        $.shebang_directive, // this is to make sharing highlights easier
        $.razor_page_directive,
        $.razor_using_directive,
        $.razor_model_directive,
        $.razor_rendermode_directive,
        $.razor_inject_directive,
        $.razor_implements_directive,
        $.razor_layout_directive,
        $.razor_inherits_directive,
        $.razor_attribute_directive,
        $.razor_typeparam_directive,
        $.razor_namespace_directive,
        $.razor_preservewhitespace_directive,
        $.razor_addtaghelper_directive,
        $.razor_removetaghelper_directive,
        $.razor_taghelperprefix_directive,
      ),

    _identifier_token: (_) =>
      token(
        // @ts-ignore
        /(\p{XID_Start}|_|\\u[0-9A-Fa-f]{4}|\\U[0-9A-Fa-f]{8})(\p{XID_Continue}|\\u[0-9A-Fa-f]{4}|\\U[0-9A-Fa-f]{8})*/,
      ),
    identifier: ($) => choice($._identifier_token, $._reserved_identifier),

    // C#'s line directives are extras in tree-sitter-c-sharp, but not here:
    // `#region`, `#line` or `#error` as extras would also match inside markup
    // (`href="#line-5"`). They are nodes wherever C# members or statements
    // stand instead.
    _preproc_line: ($) =>
      choice(
        $.preproc_region,
        $.preproc_endregion,
        $.preproc_line,
        $.preproc_pragma,
        $.preproc_nullable,
        $.preproc_error,
        $.preproc_warning,
        $.preproc_define,
        $.preproc_undef,
      ),

    _csharp_nodes: ($) => choice($.statement, $._node, $._preproc_line),

    block: ($) => seq("{", repeat($._csharp_nodes), "}"),

    // Markup is a statement in Razor code, so a `case` inside `@{ }` may
    // render an element before its `break;`.
    switch_section: ($) =>
      prec.left(
        seq(
          choice(
            seq(
              "case",
              choice($.expression, seq($.pattern, optional($.when_clause))),
            ),
            "default",
          ),
          ":",
          repeat(choice($.statement, $._node)),
        ),
      ),

    // A class declared inside `@code` keeps its `#region`s too.
    declaration_list: ($) =>
      seq("{", repeat(choice($.declaration, $._preproc_line)), "}"),

    // razor_comment is deliberately absent: it is an `extra`, so it already
    // appears anywhere. Listing it here as well made it a *node*, and matching
    // one ended the directive run in `compilation_unit` — which is why a `@*…*@`
    // above an `@using` turned that directive into an implicit expression.
    _node: ($) =>
      prec.right(
        choice(
          $.razor_escape,
          $.razor_if,
          $.razor_switch,
          $.razor_for,
          $.razor_foreach,
          $.razor_while,
          $.razor_do_while,
          $.razor_try,
          $.explicit_line_transition,
          $.razor_implicit_expression,
          $.razor_explicit_expression,
          $.razor_section,
          $.razor_compound_using,
          $.razor_lock,
          $.element,
          $.script_element,
          $.style_element,
          $.html_comment,
          $.doctype,
          // `@{ var i = 0; }` is legal *inside* markup, not just at the top of
          // the file — it was only ever reachable from `compilation_unit`.
          $.razor_block,
        ),
      ),

    _razor_marker: (_) => token("@"),

    razor_escape: ($) =>
      seq(alias(/@{2}/, "at_at_escape"), alias($._html_text, $.element)),

    // A Razor Pages page is a bare `@page`; a Blazor routable component
    // names its route.
    razor_page_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "page"), "at_page"),
        optional($.string_literal),
      ),
    razor_using_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "using"), "at_using"),
        choice(
          seq(optional("unsafe"), field("name", $.identifier), "=", $.type),
          seq(optional("static"), optional("unsafe"), $._name),
        ),
        optional(";"),
      ),
    // Right-associative so that inside `@{ }` the `;` is the directive's own
    // rather than an empty statement after it.
    razor_model_directive: ($) =>
      prec.right(
        seq(
          alias(seq($._razor_marker, "model"), "at_model"),
          field("name", $._name),
          optional(";"),
        ),
      ),
    razor_preservewhitespace_directive: ($) =>
      seq(
        alias(
          seq($._razor_marker, "preservewhitespace"),
          "at_preservewhitespace",
        ),
        $.boolean_literal,
      ),
    razor_attribute_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "attribute"), "at_attribute"),
        $.attribute_list,
      ),
    razor_implements_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "implements"), "at_implements"),
        field("name", $._name),
        optional(";"),
      ),
    razor_layout_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "layout"), "at_layout"),
        field("name", $._name),
        optional(";"),
      ),
    razor_inherits_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "inherits"), "at_inherits"),
        field("name", $._name),
        optional(";"),
      ),
    // `@typeparam T where T : Enum` constrains it as C# does.
    razor_typeparam_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "typeparam"), "at_typeparam"),
        field("name", $._name),
        repeat($.type_parameter_constraints_clause),
      ),
    razor_inject_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "inject"), "at_inject"),
        $.variable_declaration,
        optional(";"),
      ),
    razor_namespace_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "namespace"), "at_namespace"),
        // `@namespace Demo` is as legal as `@namespace Demo.App`.
        $._name,
        optional(";"),
      ),
    razor_rendermode_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "rendermode"), "at_rendermode"),
        $.razor_rendermode,
      ),
    // The three built-ins, but a render mode is just an expression — a
    // component may name its own (`@rendermode PageRenderMode`, a static field).
    razor_rendermode: ($) =>
      choice(
        "InteractiveServer",
        "InteractiveWebAssembly",
        "InteractiveAuto",
        $._name,
        // `@rendermode @(new InteractiveServerRenderMode(prerender: false))`
        $.razor_explicit_expression,
      ),

    _taghelper_target: ($) =>
      seq(
        choice($.identifier, alias("*", $.taghelper_wildcard)),
        ",",
        field("assembly", $._name),
      ),
    razor_addtaghelper_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "addTagHelper"), "at_addtaghelper"),
        choice($.string_literal, $._taghelper_target),
      ),
    razor_removetaghelper_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "removeTagHelper"), "at_removetaghelper"),
        choice($.string_literal, $._taghelper_target),
      ),
    razor_taghelperprefix_directive: ($) =>
      seq(
        alias(seq($._razor_marker, "tagHelperPrefix"), "at_taghelperprefix"),
        choice($.string_literal, $.identifier),
      ),

    razor_block: ($) =>
      prec.left(
        seq(
          alias(
            seq($._razor_marker, optional(choice("code", "functions"))),
            "at_block",
          ),
          "{",
          repeat(
            choice(
              $.declaration,
              seq($.statement),
              $._node,
              $._preproc_line,
              // Razor accepts `@model` inside `@{ }` (eShopOnWeb's pages do).
              $.razor_model_directive,
            ),
          ),
          "}",
        ),
      ),

    // `@(…)` carries its own parentheses rather than reusing C#'s shared
    // `parenthesized_expression`. Reusing it meant the state after the closing
    // `)` was shared with every C# context where an operator may follow, so
    // `style="width:@(Clamped)%"` re-read the `%` as a modulus and left a
    // MISSING node where its right operand should have been. Spelling the
    // parentheses out here gives the expression a state nothing can extend.
    razor_explicit_expression: ($) =>
      seq(alias($._razor_marker, "at_explicit"), "(", $.expression, ")"),

    // A Razor implicit expression is a restricted, whitespace-free chain —
    // that is the language's actual rule ("implicit expressions cannot contain
    // spaces"; anything richer needs `@(…)`), and `await` is the one documented
    // exception. Handing this to C#'s full `expression` let the parser run off
    // the end of the expression and into the prose behind it: `@Label. Dangerous
    // gates …` parsed the sentence as a member access, and `@repo.Files.Count
    // file(s)` swallowed `file`. The chain simply cannot do that.
    razor_implicit_expression: ($) =>
      seq(
        alias($._razor_marker, "at_implicit"),
        choice($.await_expression, $._implicit_chain),
      ),

    _implicit_chain: ($) =>
      prec.left(
        seq(
          // Identifier only — no generics. Per the Razor docs an implicit
          // expression "cannot contain generics, as the characters inside the
          // brackets (<>) are interpreted as an HTML tag": `@Method<int>()` is
          // `@Method` followed by an `<int>` element, and a generic call has to
          // be written `@(Method<int>())`.
          $.identifier,
          repeat(
            choice(
              $.razor_member_access,
              $._implicit_invocation,
              $._implicit_index,
            ),
          ),
        ),
      ),

    // `.Name` is deliberately ONE token, not `.` followed by an identifier. The
    // lexer does not backtrack across tokens, so a standalone immediate `.`
    // would be consumed before the parser could discover that no identifier
    // follows — and `title="… @Label. Dangerous gates …"` would still fail.
    // Matching the dot and the name together means `. Dangerous` simply is not
    // a member access, and the text run takes it.
    razor_member_access: (_) =>
      token.immediate(
        // @ts-ignore
        /\??\.(\p{XID_Start}|_)(\p{XID_Continue})*/,
      ),
    _implicit_invocation: ($) =>
      seq(token.immediate("("), commaSep($.argument), ")"),
    _implicit_index: ($) =>
      seq(token.immediate("["), commaSep1($.argument), optional(","), "]"),

    // `@<text>…</text>` — a RenderFragment template. It stands where a C#
    // expression does (`RenderFragment f = @<text>hi</text>;`, or as a lambda
    // body), so it extends the inherited `expression` rule below rather than
    // living in `_node`.
    razor_template: ($) =>
      seq(alias($._razor_marker, "at_template"), $.element),

    expression: ($, original) => choice(original, $.razor_template),

    razor_lock: ($) =>
      seq(
        alias(seq($._razor_marker, "lock"), "at_lock"),
        "(",
        $.expression,
        ")",
        "{",
        $._blended_content,
        "}",
      ),

    razor_compound_using: ($) =>
      seq(
        alias(seq($._razor_marker, "using"), "at_using"),
        "(",
        choice(
          alias($.using_variable_declaration, $.variable_declaration),
          $.expression,
        ),
        ")",
        "{",
        $._blended_content,
        "}",
      ),

    razor_if: ($) =>
      seq(
        alias(seq($._razor_marker, "if"), "at_if"),
        $.razor_condition,
        seq("{", $._blended_content, "}"),
        repeat(choice($.razor_else_if, $.razor_else)),
      ),

    razor_try: ($) =>
      prec.right(
        seq(
          alias(seq($._razor_marker, "try"), "at_try"),
          "{",
          $._blended_content,
          "}",
          repeat(choice($.razor_catch, $.razor_finally)),
        ),
      ),

    razor_catch: ($) =>
      seq(
        token(prec(10, "catch")),
        repeat(choice($.catch_declaration, $.catch_filter_clause)),
        "{",
        $._blended_content,
        "}",
      ),

    razor_finally: ($) =>
      seq(token(prec(10, "finally")), "{", $._blended_content, "}"),

    razor_else_if: ($) =>
      seq(
        token(prec(10, "else")),
        "if",
        $.razor_condition,
        "{",
        $._blended_content,
        "}",
      ),

    razor_else: ($) =>
      seq(token(prec(10, "else")), "{", $._blended_content, "}"),

    razor_switch: ($) =>
      seq(
        alias(seq($._razor_marker, "switch"), "at_switch"),
        $.razor_condition,
        "{",
        repeat(choice($.razor_switch_case, $.razor_switch_default)),
        "}",
      ),

    razor_condition: ($) => prec(10, seq("(", $.expression, ")")),

    razor_switch_case: ($) =>
      prec.left(
        seq(
          "case",
          $.razor_case_condition,
          ":",
          $._blended_content,
          optional("break;"),
        ),
      ),

    razor_switch_default: ($) =>
      prec.right(seq("default", ":", $._blended_content, optional("break;"))),

    razor_case_condition: (_) => /[^:]+/,

    _razor_for_initializer: ($) =>
      seq(
        alias(seq($._razor_marker, "for"), "at_for"),
        "(",
        field(
          "initializer",
          optional(choice($.variable_declaration, $.expression)),
        ),
        ";",
        field("condition", optional($.expression)),
        ";",
        field("update", optional($.expression)),
        ")",
      ),

    razor_for: ($) =>
      seq(
        $._razor_for_initializer,
        "{",
        field("body", $._blended_content),
        "}",
      ),

    _blended_content: ($) =>
      repeat1(
        prec(
          10,
          choice($._node, $.explicit_line_transition, $.statement, $.comment),
        ),
      ),

    _razor_foreach_initializer: ($) =>
      seq(
        alias(seq($._razor_marker, "foreach"), "at_foreach"),
        "(",
        choice(
          seq(
            field("type", $.type),
            field("left", choice($.identifier, $.tuple_pattern)),
          ),
          field("left", $.expression),
        ),
        "in",
        field("right", $.expression),
        ")",
      ),

    razor_foreach: ($) =>
      seq(
        $._razor_foreach_initializer,
        "{",
        field("body", $._blended_content),
        "}",
      ),

    razor_while: ($) =>
      seq(
        alias(seq($._razor_marker, "while"), "at_while"),
        $.razor_condition,
        "{",
        $._blended_content,
        "}",
      ),

    _razor_while_condition: ($) => seq("while", $.razor_condition),

    razor_do_while: ($) =>
      seq(
        alias(seq($._razor_marker, "do"), "at_do"),
        "{",
        $._blended_content,
        "}",
        $._razor_while_condition,
        ";",
      ),

    razor_section: ($) =>
      seq(
        alias(seq($._razor_marker, "section"), "at_section"),
        $.identifier,
        "{",
        $._blended_content,
        "}",
      ),

    explicit_line_transition: ($) =>
      prec.left(
        seq(
          alias("@:", "at_colon_transition"),
          alias(token(prec(1, /[^\n\r]+/)), $.element),
        ),
      ),

    // One token, not a parsed seq. As `seq("@*", repeat1(/./), "*@")` the
    // terminator had to win an LR race against every other token valid at that
    // point, which it lost inside an element or an `@if` body — the comment is
    // legal anywhere, so it must lex as a unit.
    razor_comment: (_) => token(seq("@*", /[^*]*\*+([^@*][^*]*\*+)*/, "@")),
    razor_attribute_name: ($) =>
      seq(
        $._razor_marker,
        seq(
          choice(
            "attributes",
            // `@bind` and the two-way `@bind-Value` / `@bind-Checked` form.
            token(prec(10, /bind(-[A-Za-z_][A-Za-z0-9_]*)?/)),
            "formname",
            token(prec(10, /on[a-z]+/i)),
            "key",
            "ref",
            // `<HeadOutlet @rendermode="PageRenderMode" />` — a render mode is
            // a directive attribute too, not only a top-level directive.
            "rendermode",
          ),
          optional($.razor_attribute_modifier),
        ),
      ),

    razor_attribute_modifier: (_) =>
      choice(
        ":culture",
        ":preventDefault",
        ":stopPropagation",
        ":event",
        ":format",
        ":after",
        ":get",
        ":set",
      ),

    html_comment: (_) => token(seq("<!--", /[^-]*-+([^->][^-]*-+)*/, ">")),

    doctype: (_) =>
      token(seq("<!", /[dD][oO][cC][tT][yY][pP][eE]/, /[^>]*/, ">")),

    // HTML Base Definitions
    // Lowercase only, and defined before `_tag_name` (same precedence) so it
    // wins the tie on an exact match. Blazor components are PascalCase, so
    // `<Input>` stays a normal element that expects `</Input>`.
    _void_tag_name: (_) =>
      token(
        prec(
          1,
          /(area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)/,
        ),
      ),
    _end_tag: ($) => seq("</", alias($._tag_name, $.tag_name), ">"),
    // `<script>` and `<style>` are their own elements, as in tree-sitter-html:
    // their bodies are one `raw_text` token, never markup and never C#, so the
    // editor can inject JavaScript and CSS there. The name is lexed at the same
    // precedence as `_tag_name`, so a longer name that merely starts with it
    // (`<StyleSheet>`) is still an ordinary tag by the longest-match rule.
    _script_tag_name: (_) => token(prec(1, /[sS][cC][rR][iI][pP][tT]/)),
    _style_tag_name: (_) => token(prec(1, /[sS][tT][yY][lL][eE]/)),
    _script_raw_text: (_) => token(prec(1, rawTextUntil("script"))),
    _style_raw_text: (_) => token(prec(1, rawTextUntil("style"))),
    // At the same precedence as the void, script and style names and defined
    // after them: a longer name wins by length (`<inputs>`, `<StyleSheet>`), and
    // on an exact match the earlier-defined special name wins the tie.
    _tag_name: (_) => token(prec(1, /[a-zA-Z0-9-:]+/)),
    _html_attribute_name: (_) => /[a-zA-Z0-9-:]+/,
    // The text run is lexed *below* the default precedence so that a C#
    // expression already in progress keeps going: in
    // `class="@A.Merge(x, "c")"` the `.` must extend the member access rather
    // than start a longer (and, to the lexer, more attractive) text run.
    _html_attribute_text: (_) => token(prec(-1, /[^"@]+/)),
    _html_attribute_text_single: (_) => token(prec(-1, /[^'@]+/)),
    // Single-quoted values are as valid as double-quoted ones, and are how you
    // embed a double quote: `placeholder='{ "a": 1 }'`.
    _html_attribute_value: ($) =>
      choice(
        seq(
          '"',
          repeat(
            choice(
              $.razor_explicit_expression,
              $.razor_implicit_expression,
              alias($._html_attribute_text, $.attribute_value),
            ),
          ),
          '"',
        ),
        seq(
          "'",
          repeat(
            choice(
              $.razor_explicit_expression,
              $.razor_implicit_expression,
              alias($._html_attribute_text_single, $.attribute_value),
            ),
          ),
          "'",
        ),
      ),
    // `.` and `(` were excluded from the leading character class to stop a text
    // run from stealing the `.` in `@Foo.Bar` — but that also made ordinary
    // prose unlexable, so `</code>.` (a sentence ending in a tag) failed. The
    // real requirement is a precedence one: when a C# expression is in progress
    // its `.`/`(` must win; otherwise the text run should.
    //
    // `>` is text, too. Only `<` opens a tag, so `<pre>cat a >> b</pre>` is
    // ordinary content — excluding `>` made every shell snippet unparseable.
    _html_text: (_) => token(prec(-1, /[^<&@\s]([^<&@]*[^<&@\s])?/)),
    _html_entity: (_) =>
      token(/&(#[0-9]+|#[xX][0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]*);/),

    // A directive attribute's value is C#, but the `@` prefix is idiomatic and
    // legal: `@key="@($"sub-{id}")"` is as valid as `@key="expr"`.
    razor_attribute_value: ($) =>
      choice(
        seq(
          '"',
          optional($.modifier),
          choice($.razor_explicit_expression, $.expression),
          '"',
        ),
        seq(
          "'",
          optional($.modifier),
          choice($.razor_explicit_expression, $.expression),
          "'",
        ),
      ),

    _html_attribute: ($) =>
      seq(
        alias($._html_attribute_name, $.attribute_name),
        "=",
        $._html_attribute_value,
      ),

    razor_html_attribute: ($) =>
      seq($.razor_attribute_name, optional(seq("=", $.razor_attribute_value))),

    element: ($) =>
      choice(
        // Void elements have no end tag, and the closing slash is optional:
        // `<img src="a.png">` and `<br />` are both well-formed.
        seq(
          "<",
          alias($._void_tag_name, $.tag_name),
          attributes($),
          optional("/"),
          ">",
        ),
        seq("<", alias($._tag_name, $.tag_name), attributes($), "/>"),
        seq(
          "<",
          alias($._tag_name, $.tag_name),
          attributes($),
          ">",
          repeat(
            choice(
              $._node,
              alias($._html_text, $.text),
              alias($._html_entity, $.entity),
            ),
          ),
          $._end_tag,
        ),
      ),

    script_element: ($) =>
      rawTextElement($, $._script_tag_name, $._script_raw_text),
    style_element: ($) =>
      rawTextElement($, $._style_tag_name, $._style_raw_text),
  },
});

// An element whose body is one raw text token: `<script>` and `<style>`.
function rawTextElement($, name, body) {
  return choice(
    seq("<", alias(name, $.tag_name), attributes($), "/>"),
    seq(
      "<",
      alias(name, $.tag_name),
      attributes($),
      ">",
      optional(alias(body, $.raw_text)),
      "</",
      alias(name, $.tag_name),
      ">",
    ),
  );
}

// Everything up to (not including) the first `</tag`, matched case-blind, as a
// single regular expression: a `<` that does not begin `</tag` is body text.
// Built rather than written out so the two bodies cannot drift apart.
function rawTextUntil(tag) {
  const either = (c) => `[${c.toLowerCase()}${c.toUpperCase()}]`;
  const neither = (c) => `[^${c.toLowerCase()}${c.toUpperCase()}]`;
  const alternatives = ["[^<]", "<[^/]"];
  for (let i = 0; i < tag.length; i++) {
    alternatives.push(
      "</" + [...tag.slice(0, i)].map(either).join("") + neither(tag[i]),
    );
  }
  return new RegExp(`(${alternatives.join("|")})+`);
}

function commaSep(rule) {
  return optional(commaSep1(rule));
}

function commaSep1(rule) {
  return seq(rule, repeat(seq(",", rule)));
}

// A plain helper, not a rule: a rule that can match the empty string is
// rejected by tree-sitter, and the attribute list is optional on every branch.
function attributes($) {
  return repeat(
    prec.left(
      seq(
        choice(
          $._html_attribute,
          alias($._html_attribute_name, $.attribute_name),
          $.razor_html_attribute,
        ),
        optional(" "),
      ),
    ),
  );
}
