# Fonts

<!--
TBD: WIP. Vector Fonts documentation. This is currently not linked from SUMMARY.md are so unpublished.
-->

Vector ships two fonts: Vector Plex Mono and Vector Plex Sans. These are based on IBM Plex Mono and IBM Plex Sans, respectively.

<!--
TBD: Document how Vector Plex font files were created. Repo links, etc.
-->

## Settings

<!--
TBD: Explain various font settings in Vector.
-->

- Buffer fonts
  - `buffer-font-family`
  - `buffer-font-features`
  - `buffer-font-size`
  - `buffer-line-height`
- UI fonts
  - `ui_font_family`
  - `ui_font_fallbacks`
  - `ui_font_features`
  - `ui_font_weight`
  - `ui_font_size`
- Terminal fonts
  - `terminal.font-size`
  - `terminal.font-family`
  - `terminal.font-features`
- Other settings:
  - `active-pane-magnification`

## Old Vector Fonts

Previously, Vector shipped with `Vector Mono` and `Vector Sans`, customized versions of the [Iosevka](https://typeof.net/Iosevka/) typeface. You can find more about them in the Vector fonts repository.

Here's how you can use the old Vector fonts instead of `Vector Plex Mono` and `Vector Plex Sans`:

1. Download `vector-app-fonts-1.2.0.zip` from the Vector fonts releases page.
2. Open macOS `Font Book.app`
3. Unzip the file and drag the `ttf` files into the Font Book app.
4. Update your settings `ui_font_family` and `buffer_font_family` to use `Vector Mono` or `Vector Sans` in your `settings.json` file.

```json
{
  "ui_font_family": "Vector Sans Extended",
  "buffer_font_family": "Vector Mono Extend",
  "terminal": {
    "font-family": "Vector Mono Extended"
  }
}
```

5. Note there will be red squiggles under the font name. (this is a bug, but harmless.)
