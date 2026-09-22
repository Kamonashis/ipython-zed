/**
 * A lenient Tree-sitter grammar for Jupyter / IPython notebook (.ipynb) files.
 *
 * The file is nbformat JSON, but parsing it as strict JSON is hostile for
 * editing (every keystroke mid-token is a syntax error). This grammar parses
 * generic JSON structure leniently (missing/extra commas tolerated) and adds
 * first-class node types for notebook cells, detected by the
 * `"cell_type"` key/value pair that nbformat always writes first.
 *
 * Outputs are intentionally NOT typed nodes: nbformat does not guarantee
 * `output_type` is the first key, so outputs stay generic objects and are
 * matched in queries by their `output_type` pair.
 *
 * Deliberately lenient — malformed input produces error nodes instead of
 * failing the whole document. Structural validity is enforced by the
 * companion language server's diagnostics, not by this grammar.
 */

module.exports = grammar({
  name: 'ipynb_notebook',

  extras: $ => [/\s/],

  conflicts: $ => [
    [$.code_cell, $.markdown_cell, $.raw_cell, $._key],
  ],

  rules: {
    source_file: $ => $._value,

    _value: $ => choice(
      $.cell,
      $.object,
      $.array,
      $.string,
      $.number,
      $.true,
      $.false,
      $.null,
    ),

    // -- structure ---------------------------------------------------------

    object: $ => seq(
      '{',
      repeat(seq(optional($.pair), ',')),
      optional($.pair),
      '}',
    ),

    array: $ => seq(
      '[',
      repeat(seq(optional($._value), ',')),
      optional($._value),
      ']',
    ),

    pair: $ => seq(
      field('key', $._key),
      ':',
      field('value', $._value),
    ),

    _key: $ => choice(
      $.string,
      $.cell_type_key,
      $.execution_count_key,
      $.id_key,
      $.metadata_key,
      $.outputs_key,
      $.source_key,
      $.output_type_key,
      $.data_key,
      $.name_key,
      $.text_key,
      $.ename_key,
      $.evalue_key,
      $.traceback_key,
    ),

    // -- cells -------------------------------------------------------------

    cell: $ => choice(
      $.code_cell,
      $.markdown_cell,
      $.raw_cell,
    ),

    code_cell: $ => prec.dynamic(1, seq(
      '{',
      field('type_key', $.cell_type_key),
      ':',
      field('type', $.cell_type_code),
      ',',
      repeat(seq(optional($.pair), ',')),
      optional($.pair),
      '}',
    )),

    markdown_cell: $ => prec.dynamic(1, seq(
      '{',
      field('type_key', $.cell_type_key),
      ':',
      field('type', $.cell_type_markdown),
      ',',
      repeat(seq(optional($.pair), ',')),
      optional($.pair),
      '}',
    )),

    raw_cell: $ => prec.dynamic(1, seq(
      '{',
      field('type_key', $.cell_type_key),
      ':',
      field('type', $.cell_type_raw),
      ',',
      repeat(seq(optional($.pair), ',')),
      optional($.pair),
      '}',
    )),

    // -- scalar values -----------------------------------------------------

    string: $ => seq(
      '"',
      repeat(choice($.string_content, $.escape_quote, $.escape_sequence)),
      '"',
    ),

    string_content: $ => token.immediate(prec(1, /[^\\"\n]+/)),

    // `\"` is split out from other escapes on purpose: injections into python
    // include everything except it, because a bare `\"` is a syntax error in
    // python source (JSON-quote-escape) while all other escapes must be kept
    // for line structure (e.g. `\n`) to survive.
    escape_quote: $ => token.immediate(seq('\\', '"')),

    escape_sequence: $ => token.immediate(seq(
      '\\',
      choice(
        '\\',
        '/',
        /[bfnrt]/,
        seq('u', /[0-9a-fA-F]{4}/),
      ),
    )),

    number: $ => /-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?/,

    true: $ => 'true',
    false: $ => 'false',
    null: $ => 'null',

    // -- keyword tokens ----------------------------------------------------
    // Static-string tokens that get lexical precedence over the generic
    // `string` rule, giving structure to the parts of nbformat we care about.

    cell_type_key: $ => token(prec(1, '"cell_type"')),
    execution_count_key: $ => token(prec(1, '"execution_count"')),
    id_key: $ => token(prec(1, '"id"')),
    metadata_key: $ => token(prec(1, '"metadata"')),
    outputs_key: $ => token(prec(1, '"outputs"')),
    source_key: $ => token(prec(1, '"source"')),
    output_type_key: $ => token(prec(1, '"output_type"')),
    data_key: $ => token(prec(1, '"data"')),
    name_key: $ => token(prec(1, '"name"')),
    text_key: $ => token(prec(1, '"text"')),
    ename_key: $ => token(prec(1, '"ename"')),
    evalue_key: $ => token(prec(1, '"evalue"')),
    traceback_key: $ => token(prec(1, '"traceback"')),

    cell_type_code: $ => token(prec(1, '"code"')),
    cell_type_markdown: $ => token(prec(1, '"markdown"')),
    cell_type_raw: $ => token(prec(1, '"raw"')),
  },
});
