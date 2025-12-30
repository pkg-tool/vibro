# Workspace Persistence

Vector creates local SQLite databases to persist data relating to its workspace and your projects. These databases store, for instance, the tabs and panes you have open in a project, the scroll position of each open file, the list of all projects you've opened (for the recent projects modal picker), etc. You can find and explore these databases in the following locations:

- macOS: `~/Library/Application Support/Vector`
- Linux: `~/.local/share/Vector`
- Windows: `%LOCALAPPDATA%\Vector`

The naming convention of these databases takes on the form of `0-<vector_channel>`:

- Stable: `0-stable`
- Preview: `0-preview`

**If you encounter workspace persistence issues in Vector, deleting the database and restarting Vector often resolves the problem, as the database may have been corrupted at some point.** If your issue continues after restarting Vector and regenerating a new database, please [file an issue](https://github.com/vector-editor/vector/issues/new).
