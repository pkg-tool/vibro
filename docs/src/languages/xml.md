# XML

XML support is available through an extension (open the Extensions panel and search for `xml`).

- Tree-sitter: [tree-sitter-grammars/tree-sitter-xml](https://github.com/tree-sitter-grammars/tree-sitter-xml)

## Configuration

If you have additional file extensions that are not being automatically recognized as XML just add them to [file_types](../configuring-vector.md#file-types) in your Vector settings:

```json [settings]
  "file_types": {
    "XML": ["rdf", "gpx", "kml"]
  }
```
