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

; cell ids are generated noise — dim them; execution counts stay bright
; (they are the In [n] signal)

(code_cell
  (pair key: (id_key) value: (string (string_content) @comment)))

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

; --- dim nbformat scaffolding: cell content (injected python/markdown)
; --- should dominate the view, the JSON plumbing should recede

(cell_type_key) @comment
(execution_count_key) @comment
(id_key) @comment
(metadata_key) @comment
(outputs_key) @comment
(source_key) @comment
(output_type_key) @comment
(data_key) @comment
(name_key) @comment
(text_key) @comment
(ename_key) @comment
(evalue_key) @comment
(traceback_key) @comment

; whole metadata objects (cell and notebook level) are generated noise

(pair
  key: (metadata_key)
  value: (object)) @comment

; top-level nbformat keys without dedicated grammar nodes

(pair
  key: (string
    (string_content) @variant
    (#match? @variant "^(nbformat|nbformat_minor|cells)$")) @comment)
