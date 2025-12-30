# Vector Extensions

This directory contains extensions for Vector that are maintained alongside the editor for ease of development.

## Structure

Vector includes support for a number of languages without requiring installing an extension. Those languages can be found under `crates/languages/src`.

Support for all other languages is done via extensions. These extensions provide language servers, tree-sitter grammars, and related configuration.

## Dev Extensions

See the docs for developing extensions locally for how to work with one of these extensions.

## Updating

> [!NOTE]
> Community contributors should submit a PR with the changes, and the maintainers will handle releases.

The process for updating an extension in this directory has two parts.

1. Create a PR with your changes. (Merge it)
2. Bump the extension version in:

   - extensions/{language_name}/extension.toml
   - extensions/{language_name}/Cargo.toml
   - Cargo.lock

   You can do this manually, or with a script:

   ```sh
   # Output the current version for a given language
   ./script/language-extension-version <langname>

   # Update the version in `extension.toml` and `Cargo.toml` and trigger a `cargo check`
   ./script/language-extension-version <langname> <new_version>
   ```

   Commit your changes to a branch, push a PR and merge it.

3. Publish updated extensions via your chosen registry/distribution mechanism.
