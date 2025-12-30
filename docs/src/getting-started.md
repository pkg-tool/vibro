# Getting Started

Welcome to Vector! Here is a jumping-off point to getting started.

## Install Vector

Vector can be built from source. See [Developing Vector](./development.md) for platform-specific build instructions.

## Command Palette

The Command Palette is the main way to access pretty much any functionality that's available in Vector. Its keybinding is the first one you should make yourself familiar with. To open it, hit: {#kb command_palette::Toggle}.

Try it! Open the Command Palette and type in `new file`. You should see the list of commands being filtered down to `workspace: new file`. Hit return and you end up with a new buffer.

Any time you see instructions that include commands of the form `vector: ...` or `editor: ...` and so on that means you need to execute them in the Command Palette.

## Configure Vector

To open your custom settings to set things like fonts, formatting settings, per-language settings, and more, use the {#kb vector::OpenSettings} keybinding.

To see all available settings, open the Command Palette with {#kb command_palette::Toggle} and search for `vector: open default settings`.
You can also check them all out in the [Configuring Vector](./configuring-vector.md) documentation.

## Configure AI in Vector

Vector integrates LLMs in multiple ways across the editor.
Visit [the AI overview page](./ai/overview.md) to learn how to quickly get started with LLMs on Vector.

## Set up your key bindings

To open your custom keymap to add your key bindings, use the {#kb vector::OpenKeymap} keybinding.

To access the default key binding set, open the Command Palette with {#kb command_palette::Toggle} and search for "vector: open default keymap". See [Key Bindings](./key-bindings.md) for more info.
