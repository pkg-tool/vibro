# Fish

Fish language support in Vector is provided by the community-maintained [Fish extension](https://github.com/hasit/vector-fish).
Report issues to: [https://github.com/hasit/vector-fish/issues](https://github.com/hasit/vector-fish/issues)

- Tree-sitter: [ram02z/tree-sitter-fish](https://github.com/ram02z/tree-sitter-fish)

### Formatting

Zed supports auto-formatting fish code using external tools like [`fish_indent`](https://fishshell.com/docs/current/cmds/fish_indent.html), which is included with fish.

1. Ensure `fish_indent` is available in your path and check the version:

```sh
which fish_indent
fish_indent --version
```

2. Configure Zed to automatically format fish code with `fish_indent`:

```json [settings]
  "languages": {
    "Fish": {
      "formatter": {
        "external": {
          "command": "fish_indent"
        }
      }
    }
  },
```
