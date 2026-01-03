(comment) @comment
(generated_comment) @comment

(subject) @title
(subject (overflow) @comment)

(prefix (type) @keyword)
(prefix (scope) @attribute)
(prefix ["(" ")" ":"] @punctuation.delimiter)
(prefix "!" @punctuation.special)

(title) @title
(text) @comment

(branch) @string.special.symbol
(number) @number

(change kind: (new) @string)
(change kind: (modified) @keyword)
(change kind: (deleted) @variable.special)
(change kind: (renamed) @keyword)

(filepath) @link_uri
(arrow) @punctuation.delimiter
(annotation) @comment

(trailer (token) @attribute)
(breaking_change (token) @variable.special)

(scissor) @comment
