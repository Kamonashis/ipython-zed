; nbformat structure: keys, scalars, strings

(pair key: (_) @property)

(string) @string
(string_content) @string
(escape_quote) @string.escape
(escape_sequence) @string.escape

(number) @number

(true) @boolean
(false) @boolean
(null) @constant.builtin

; cell types — visually distinguish the three cell kinds

(code_cell type: (cell_type_code) @type)
(markdown_cell type: (cell_type_markdown) @emphasis)
(raw_cell type: (cell_type_raw) @comment)

; cell ids and execution counts stand out slightly

(code_cell
  (pair key: (id_key) value: (string (string_content) @label)))

(code_cell
  (pair key: (execution_count_key) value: (number) @number))

; error outputs (output_type: "error") — flag the traceback keys

(object
  (pair
    key: (output_type_key)
    value: (string
      (string_content) @variant
      (#eq? @variant "error")))
  (pair key: (ename_key) @property)
  (pair key: (evalue_key) @property)
  (pair key: (traceback_key) @property))
