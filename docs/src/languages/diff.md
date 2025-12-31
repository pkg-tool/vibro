# Diff

Diff support is available natively in Vector.

- Tree-sitter: [the-mikedavis/tree-sitter-diff](https://github.com/the-mikedavis/tree-sitter-diff)

## Configuration

Vector will not attempt to format diff files and has [`remove_trailing_whitespace_on_save`](https://github.com/pkg-tool/vector/blob/master/docs/src/configuring-vector.md#remove-trailing-whitespace-on-save) and [`ensure-final-newline-on-save`](https://github.com/pkg-tool/vector/blob/master/docs/src/configuring-vector.md#ensure-final-newline-on-save) set to false.

Vector will automatically recognize files with `patch` and `diff` extensions as Diff files. To recognize other extensions, add them to `file_types` in your Vector settings.json:

```json
  "file_types": {
    "Diff": ["dif"]
  },
```
