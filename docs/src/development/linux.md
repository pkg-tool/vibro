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

## Backend dependencies

Vector does not include collaboration or call features in this fork, so there are no additional backend services required for development.

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

- You will need to build `crates/cli` and make its binary available in `$PATH` with the name `vector`.
- You will need to build the application (package `vector` in `crates/vector`) and put it at `$PATH/to/cli/../../libexec/vector-editor`. For example, if you are going to put the CLI at `~/.local/bin/vector` put the editor at `~/.local/libexec/vector-editor`. As some linux distributions (notably Arch) discourage the use of `libexec`, you can also put this binary at `$PATH/to/cli/../../lib/vector/vector-editor` (e.g. `~/.local/lib/vector/vector-editor`) instead.
- If you are going to provide a `.desktop` file you can find a template in `crates/vector/resources/vector.desktop.in`, and use `envsubst` to populate it with the values required. This file should also be renamed to `$APP_ID.desktop` so that the file follows the FreeDesktop standards.
- You will need to ensure that the necessary libraries are installed. You can get the current list by inspecting the built binary on your system.
- For an example of a complete build script, see `script/bundle-linux`.
- You can disable Vector's auto updates and provide instructions for users who try to update Vector manually by building (or running) Vector with the environment variable `VECTOR_UPDATE_EXPLANATION`. For example: `VECTOR_UPDATE_EXPLANATION="Please use flatpak to update Vector."`.
- Make sure to update the contents of the `crates/release_channel/RELEASE_CHANNEL` file to `nightly`, `preview`, or `stable`, with no newline. This will cause Vector to use the credentials manager to remember a user's login.

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

## Troubleshooting

### Cargo errors claiming that a dependency is using unstable features

Try `cargo clean` and `cargo build`.

### Vulkan/GPU issues

If Vector crashes at runtime due to GPU or vulkan issues, you can try running [vkcube](https://github.com/krh/vkcube) (usually available as part of the `vulkaninfo` package on various distributions) to try to troubleshoot where the issue is coming from. Try running in both X11 and wayland modes by running `vkcube -m [x11|wayland]`. Some versions of `vkcube` use `vkcube` to run in X11 and `vkcube-wayland` to run in wayland.

If you have multiple GPUs, you can also try running Vector on a different one (for example, with [vkdevicechooser](https://github.com/jiriks74/vkdevicechooser)) to figure out where the issue comes from.
