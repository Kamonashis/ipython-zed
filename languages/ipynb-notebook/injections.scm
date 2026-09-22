; Python source of code cells — array-of-strings and single-string forms.
;
; Capture string content runs and every escape except `\"`: the JSON
; quote-escape would appear as a bare backslash-quote in python source and
; corrupt the injected document, while all other escapes (\n, \\, \t, unicode)
; must be kept for line structure to survive.
;
; `injection.combined` merges all fragments into one python document so
; multi-line statements and block structure parse correctly.

(code_cell
  (pair
    key: (source_key)
    value: (array
      (string
        [
          (string_content)
          (escape_sequence)
        ] @injection.content)))
  (#set! injection.language "python")
  (#set! injection.combined))

(code_cell
  (pair
    key: (source_key)
    value: (string
      [
        (string_content)
        (escape_sequence)
      ] @injection.content))
  (#set! injection.language "python")
  (#set! injection.combined))

; Markdown source of markdown cells.

(markdown_cell
  (pair
    key: (source_key)
    value: (array
      (string
        [
          (string_content)
          (escape_quote)
          (escape_sequence)
        ] @injection.content)))
  (#set! injection.language "markdown")
  (#set! injection.combined))

(markdown_cell
  (pair
    key: (source_key)
    value: (string
      [
        (string_content)
        (escape_quote)
        (escape_sequence)
      ] @injection.content))
  (#set! injection.language "markdown")
  (#set! injection.combined))
