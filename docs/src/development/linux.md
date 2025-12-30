# Building Vector for Linux

## Repository

Clone the Vector repository.

## Dependencies

- Install [rustup](https://www.rust-lang.org/tools/install)

- Install the necessary system libraries:

  ```sh
  script/linux
  ```

  If you prefer to install the system libraries manually, you can find the list of required packages in the `script/linux` file.

### Linkers {#linker}

On Linux, Rust's default linker is [LLVM's `lld`](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/). Alternative linkers, especially [Wild](https://github.com/davidlattimore/wild) and [Mold](https://github.com/rui314/mold) can significantly improve clean and incremental build time.

At present Zed uses Mold in CI because it's more mature. For local development Wild is recommended because it's 5-20% faster than Mold.

These linkers can be installed with `script/install-mold` and `script/install-wild`.

To use Wild as your default, add these lines to your `~/.cargo/config.toml`:

```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=--ld-path=wild"]

[target.aarch64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=--ld-path=wild"]
```

To use Mold as your default:

```toml
[target.'cfg(target_os = "linux")']
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

## Building from source

Once the dependencies are installed, you can build Vector using [Cargo](https://doc.rust-lang.org/cargo/).

For a debug build of the editor:

```sh
cargo run
```

And to run the tests:

```sh
cargo test --workspace
```

In release mode, the primary user interface is the `cli` crate. You can run it in development with:

```sh
cargo run -p cli
```

## Installing a development build

You can install a local build on your machine with:

```sh
./script/install-linux
```

This will build Vector and the CLI in release mode and make them available at `~/.local/bin/vector`, installing .desktop files to `~/.local/share`.

> **Note**: If you encounter linker errors involving `aws-lc-rs` on GCC >= 14, see upstream issues like:
> - [FIPS fails to build with GCC >= 14](https://github.com/aws/aws-lc-rs/issues/569)
> - [GCC-14 - build failure for FIPS module](https://github.com/aws/aws-lc/issues/2010)

## Wayland & X11

Vector supports both X11 and Wayland. By default, we pick whichever we can find at runtime. If you're on Wayland and want to run in X11 mode, use the environment variable `WAYLAND_DISPLAY=''`.

## Notes for packaging Vector

Thank you for taking on the task of packaging Vector!

### Technical requirements

Vector has two main binaries:

- You will need to build `crates/cli` and make its binary available in `$PATH` with the name `zed`.
- You will need to build `crates/zed` and put it at `$PATH/to/cli/../../libexec/zed-editor`. For example, if you are going to put the cli at `~/.local/bin/zed` put zed at `~/.local/libexec/zed-editor`. As some linux distributions (notably Arch) discourage the use of `libexec`, you can also put this binary at `$PATH/to/cli/../../lib/zed/zed-editor` (e.g. `~/.local/lib/zed/zed-editor`) instead.
- If you are going to provide a `.desktop` file you can find a template in `crates/zed/resources/zed.desktop.in`, and use `envsubst` to populate it with the values required. This file should also be renamed to `$APP_ID.desktop` so that the file [follows the FreeDesktop standards](https://github.com/zed-industries/zed/issues/12707#issuecomment-2168742761). You should also make this desktop file executable (`chmod 755`).
- You will need to ensure that the necessary libraries are installed. You can get the current list by [inspecting the built binary](https://github.com/zed-industries/zed/blob/935cf542aebf55122ce6ed1c91d0fe8711970c82/script/bundle-linux#L65-L67) on your system.
- For an example of a complete build script, see [script/bundle-linux](https://github.com/zed-industries/zed/blob/935cf542aebf55122ce6ed1c91d0fe8711970c82/script/bundle-linux).
- You can disable Zed's auto updates and provide instructions for users who try to update Zed manually by building (or running) Zed with the environment variable `ZED_UPDATE_EXPLANATION`. For example: `ZED_UPDATE_EXPLANATION="Please use flatpak to update zed."`.
- Make sure to update the contents of the `crates/zed/RELEASE_CHANNEL` file to 'nightly', 'preview', or 'stable', with no newline. This will cause Zed to use the credentials manager to remember a user's login.

### Other things to note

At Vector, our priority has been to move fast and bring the latest technology to our users. We've long been frustrated at having software that is slow, out of date, or hard to configure, and so we've built our editor to those tastes.

However, we realize that many distros have other priorities. We want to work with everyone to bring Vector to their favorite platforms. But there is a long way to go:

- Vector is a fast-moving early-phase project.
- Vector automatically installs the correct version of common developer tools in the same way as rustup/rbenv/pyenv, etc.
- Users can install extensions locally. These extensions may install further tooling as needed, such as language servers.
- Some features connect to online services by default (e.g. AI providers). Vector itself does not include collaboration/calls or built-in usage analytics in this fork.
- As a result of the above issues, Vector currently does not play nice with sandboxes.

## Flatpak

> Vector's current Flatpak integration exits the sandbox on startup. Workflows that rely on Flatpak's sandboxing may not work as expected.

To build & install the Flatpak package locally follow the steps below:

1. Install Flatpak for your distribution as outlined [here](https://flathub.org/setup).
2. Run the `script/flatpak/deps` script to install the required dependencies.
3. Run `script/flatpak/bundle-flatpak`.
4. Now the package has been installed and has a bundle available at `target/release/{app-id}.flatpak`.

## Memory profiling

[`heaptrack`](https://github.com/KDE/heaptrack) is quite useful for diagnosing memory leaks. To install it:

```sh
$ sudo apt install heaptrack heaptrack-gui
$ cargo install cargo-heaptrack
```

Then, to build and run Vector with the profiler attached:

```sh
$ cargo heaptrack -b vector
```

When this Vector instance is exited, terminal output will include a command to run `heaptrack_interpret` to convert the `*.raw.zst` profile to a `*.zst` file which can be passed to `heaptrack_gui` for viewing.

## Perf recording

How to get a flamegraph with resolved symbols from a running zed instance. Use
when zed is using a lot of CPU. Not useful for hangs.

### During the incident

- Find the PID (process ID) using:
  `ps -eo size,pid,comm | grep zed | sort | head -n 1 | cut -d ' ' -f 2`
  Or find the pid of the command zed-editor with the most ram usage in something
  like htop/btop/top.

- Install perf:
  On Ubuntu (derivatives) run `sudo apt install linux-tools`.

- Perf Record:
  run `sudo perf record -p <pid you just found>`, wait a few seconds to gather data then press Ctrl+C. You should now have a perf.data file

- Make the output file user owned:
  run `sudo chown $USER:$USER perf.data`

- Get build info:
  Run zed again and type `zed: about` in the command pallet to get the exact commit.

The `data.perf` file can be send to zed together with the exact commit.

### Later

This can be done by Zed staff.

- Build Zed with symbols:
  Check out the commit found previously and modify `Cargo.toml`.
  Apply the following diff then make a release build.

```diff
[profile.release]
-debug = "limited"
+debug = "full"
```

- Add the symbols to perf database:
  `pref buildid-cache -v -a <path to release zed binary>`

- Resolve the symbols from the db:
  `perf inject -i perf.data -o perf_with_symbols.data`

- Install flamegraph:
  `cargo install cargo-flamegraph`

- Render the flamegraph:
  `flamegraph --perfdata perf_with_symbols.data`

## Troubleshooting

### Cargo errors claiming that a dependency is using unstable features

Try `cargo clean` and `cargo build`.
