# Repository Guidelines

## Project Structure & Module Organization

- `crates/`: Rust workspace crates.
  - `crates/vector/`: main editor application.
  - `crates/cli/`: command-line interface (`vector` integration commands).
- `assets/`: bundled resources (keymaps, fonts, generated `assets/licenses.md`).
- `docs/`: documentation source (`docs/src/` Markdown).
- `extensions/`: bundled extensions and extension tooling.
- `script/`: developer/CI utilities (linting, bundling, checks).
- `tooling/xtask/`: CI/workflow helpers.

## Build, Test, and Development Commands

- `cargo run`: run the editor (debug).
- `cargo run --release`: run an optimized build.
- `cargo test --workspace`: run the full test suite.
- `script/clippy`: runs `cargo clippy --release --all-targets --all-features -- -D warnings` and (locally) `cargo machete` + `typos`.
- `script/check-keymaps`: keymap sanity checks (platform key naming rules).
- `script/bundle-mac`: build a stable macOS `.app` + `.dmg` (output: `target/<arch>-apple-darwin/release/Vector.dmg`). Optional arch: `script/bundle-mac x86_64-apple-darwin`.
- `script/prettier --write`: format `assets/settings/default.json` and `docs/` (requires `pnpm`).

## Coding Style & Naming Conventions

- Rust is formatted with `rustfmt` (`cargo fmt`) and linted with strict Clippy; do not suppress lints—fix root causes.
- Use standard Rust naming: modules/functions `snake_case`, types/traits `CamelCase`, constants `SCREAMING_SNAKE_CASE`.
- Keybinding/action names should reference existing actions (namespace `vector::…`).

## Testing Guidelines

- Prefer adding/adjusting unit tests next to the code you change.
- When touching keymaps, run `cargo test -p vector test_base_keymap --all-features`.

## Commit & Pull Request Guidelines

- Follow existing commit style: `area: short summary (#PR)` (e.g., `ui: Fix NumberField focus (#45871)`).
- PRs should explain **what** and **why**, note any user-visible behavior changes, and include screenshots for UI changes.

## Offline-First Policy (Important)

Vector is designed to run fully offline: the app must not perform network requests and must not include social features (collaboration, chat, calls, subscriptions). Extensions and toolchains are installed manually (no built-in “download/install” flows).

The only allowed network capability is **auto-update**, and only when an explicit `auto_update_url` is configured (default: off, network blocked).

## Porting From `main` → `master` (Strict Offline Fork)

Upstream `main` moves fast; cherry-picking individual commits is usually impractical. Preferred workflow: rebase/merge `master` onto `main`, then re-apply/update the “strict offline” patch-set and re-audit for residual networking.

**Offline guardrails that must remain enabled:**
- Block runtime HTTP: `crates/vector/src/main.rs` (`BlockedHttpClient`) and prevent UI URL-opening: `crates/gpui/src/app.rs` (`gpui::set_allow_http_urls(false)`).
- Disable auto-downloads: Node/toolchains (`crates/vector/src/main.rs`), LSP servers (`crates/project/src/lsp_store.rs`), DAP adapters (`crates/dap_adapters/src/`), Prettier/plugins (`crates/project/src/prettier_store.rs`), extension marketplace/registry.
- Gate auto-update strictly behind `auto_update_url` (default off).
- Keep `vector` branding/paths (avoid drifting back to `zed` / `.zed` naming).

**Safe to port (offline editor features):**
- editor core; UI components; search/replace; project tree; quick open/fuzzy; command palette; settings/keymaps; themes/rendering; terminal; local previews (Markdown/SVG/images); snippets; task runner; local LSP (stdio); local DAP; local Git (diff/commit); platform fixes; perf/memory improvements.

**Never port (online/social/network/AI):**
- `collab*`, `channel*`, `call*`, `livekit*`, `remote*`, `cloud_*`, login/subscriptions/upsell.
- network stack/crates (`net`, `rpc`, `http_client*`, `reqwest_client`, “cloud client”), telemetry/crash uploads.
- extension store/auto-download; “download/install” toolchain flows.
- AI/LLM/agent/assistant providers (`agent*`, `assistant_*`, `open_ai`, `anthropic`, `mistral`, `google_ai`, `x_ai`, `open_router`, web search providers, edit prediction).

**Review required (port only after redesign for offline):**
- extension system (manual install only), JSON schema store (local-only), git hosting UI, devcontainers/remote tooling, crash handling (local logs only), “system specs” collection, CI/release workflows (non-runtime). Default: skip until explicitly designed for offline.

**Maintain this list:** the “Safe to port / Never port / Review required” groups are not fixed. Whenever you sync or rebase from upstream `main`, re-audit new crates/features and update these groups based on the current code and offline policy.
