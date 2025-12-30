# Contributing to Vector

Thanks for your interest in contributing to Vector!

All activity in project spaces is subject to our `CODE_OF_CONDUCT.md`.

## Contribution ideas

If you're looking for ideas about what to work on, check out:

- The project issue tracker and discussions (if enabled).

For adding themes or support for a new language, check out our docs on developing extensions under `docs/src/extensions/developing-extensions.md`.

## Proposing changes

The best way to propose a change is to start with an issue or discussion in this repository.

First, write a short **problem statement**, which _clearly_ and _briefly_ describes the problem you want to solve independently from any specific solution. It doesn't need to be long or formal, but it's difficult to consider a solution in absence of a clear understanding of the problem.

Next, write a short **solution proposal**. How can the problem (or set of problems) you have stated above be addressed? What are the pros and cons of your approach? Again, keep it brief and informal. This isn't a specification, but rather a starting point for a conversation.

By effectively engaging early in your process, we're better positioned to give you feedback and understand your pull request once you open it. If the first thing we see from you is a big changeset, we're much less likely to respond to it in a timely manner.

## Tips to improve the chances of your PR getting reviewed and merged

- Discuss your plans ahead of time with the team
- Small, focused, incremental pull requests are much easier to review
- Spend time explaining your changes in the pull request body
- Add test coverage and documentation
- Choose tasks that align with our roadmap
- Pair with us and watch us code to learn the codebase
- Low effort PRs, such as those that just re-arrange syntax, won't be merged without a compelling justification

## File icons

Vector's default icon theme consists of icons that are designed to fit together in a cohesive manner.

We do not accept PRs for file icons that are just an off-the-shelf SVG taken from somewhere else.

### Adding new icons to the Vector icon theme

If you would like to add a new icon to the Vector icon theme, open a discussion and we can work with you on getting an icon designed and added.

## Bird's-eye view of Vector

Vector is made up of several smaller crates - let's go over those you're most likely to interact with:

- [`gpui`](/crates/gpui) is a GPU-accelerated UI framework which provides all of the building blocks for Vector. **We recommend familiarizing yourself with the root level GPUI documentation.**
- [`editor`](/crates/editor) contains the core `Editor` type that drives both the code editor and various input fields within Vector. It also handles a display layer for LSP features such as Inlay Hints or code completions.
- [`project`](/crates/project) manages files and navigation within the filetree. It is also Vector's side of communication with LSP.
- [`workspace`](/crates/workspace) handles local state serialization and groups projects together.
- [`vim`](/crates/vim) is a thin implementation of Vim workflow over `editor`.
- [`lsp`](/crates/lsp) handles communication with external LSP servers.
- [`language`](/crates/language) drives `editor`'s understanding of language - from providing a list of symbols to the syntax map.
- [`rpc`](/crates/rpc) defines messages exchanged with remote components (e.g. remote development).
- [`theme`](/crates/theme) defines the theme system and provides a default theme.
- [`ui`](/crates/ui) is a collection of UI components and common patterns used throughout Vector.
- [`cli`](/crates/cli) is the CLI crate which invokes the Vector binary.
- [`vector`](/crates/vector) is where all things come together, and the `main` entry point for Vector.

## Packaging Vector

Check our notes under `docs/src/development/linux.md`.
