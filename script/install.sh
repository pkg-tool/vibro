#!/usr/bin/env sh
set -eu

# Downloads the latest tarball from https://vector.dev/releases and unpacks it
# into ~/.local/. If you'd prefer to do this manually, instructions are at
# https://vector.dev/docs/linux.

main() {
    platform="$(uname -s)"
    arch="$(uname -m)"
    channel="${ZED_CHANNEL:-stable}"
    # Use TMPDIR if available (for environments with non-standard temp directories)
    if [ -n "${TMPDIR:-}" ] && [ -d "${TMPDIR}" ]; then
        temp="$(mktemp -d "$TMPDIR/zed-XXXXXX")"
    else
        temp="$(mktemp -d "/tmp/zed-XXXXXX")"
    fi

    if [ "$platform" = "Darwin" ]; then
        platform="macos"
    elif [ "$platform" = "Linux" ]; then
        platform="linux"
    else
        echo "Unsupported platform $platform"
        exit 1
    fi

    case "$platform-$arch" in
        macos-arm64* | linux-arm64* | linux-armhf | linux-aarch64)
            arch="aarch64"
            ;;
        macos-x86* | linux-x86* | linux-i686*)
            arch="x86_64"
            ;;
        *)
            echo "Unsupported platform or architecture"
            exit 1
            ;;
    esac

    if command -v curl >/dev/null 2>&1; then
        curl () {
            command curl -fL "$@"
        }
    elif command -v wget >/dev/null 2>&1; then
        curl () {
            wget -O- "$@"
        }
    else
        echo "Could not find 'curl' or 'wget' in your path"
        exit 1
    fi

    "$platform" "$@"

    if [ "$(command -v vector)" = "$HOME/.local/bin/vector" ]; then
        echo "Vector has been installed. Run with 'vector'"
    else
        echo "To run Vector from your terminal, you must add ~/.local/bin to your PATH"
        echo "Run:"

        case "$SHELL" in
            *zsh)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.zshrc"
                echo "   source ~/.zshrc"
                ;;
            *fish)
                echo "   fish_add_path -U $HOME/.local/bin"
                ;;
            *)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.bashrc"
                echo "   source ~/.bashrc"
                ;;
        esac

        echo "To run Vector now, '~/.local/bin/vector'"
    fi
}

linux() {
    if [ -n "${VECTOR_BUNDLE_PATH:-}" ]; then
        cp "$VECTOR_BUNDLE_PATH" "$temp/vector-linux-$arch.tar.gz"
    else
        echo "Downloading Zed"
        curl "https://cloud.zed.dev/releases/$channel/latest/download?asset=zed&arch=$arch&os=linux&source=install.sh" > "$temp/zed-linux-$arch.tar.gz"
    fi

    suffix=""
    if [ "$channel" != "stable" ]; then
        suffix="-$channel"
    fi

    appid=""
    case "$channel" in
      stable)
        appid="dev.vector.Vector"
        ;;
      nightly)
        appid="dev.vector.Vector-Nightly"
        ;;
      preview)
        appid="dev.vector.Vector-Preview"
        ;;
      dev)
        appid="dev.vector.Vector-Dev"
        ;;
      *)
        echo "Unknown release channel: ${channel}. Using stable app ID."
        appid="dev.vector.Vector"
        ;;
    esac

    # Unpack
    rm -rf "$HOME/.local/vector$suffix.app"
    mkdir -p "$HOME/.local/vector$suffix.app"
    tar -xzf "$temp/vector-linux-$arch.tar.gz" -C "$HOME/.local/"

    # Setup ~/.local directories
    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"

    # Link the binary
    if [ -f "$HOME/.local/vector$suffix.app/bin/vector" ]; then
        ln -sf "$HOME/.local/vector$suffix.app/bin/vector" "$HOME/.local/bin/vector"
    else
        # support for versions before 0.139.x.
        ln -sf "$HOME/.local/vector$suffix.app/bin/cli" "$HOME/.local/bin/vector"
    fi

    # Copy .desktop file
    desktop_file_path="$HOME/.local/share/applications/${appid}.desktop"
    cp "$HOME/.local/vector$suffix.app/share/applications/vector$suffix.desktop" "${desktop_file_path}"
    sed -i "s|Icon=vector|Icon=$HOME/.local/vector$suffix.app/share/icons/hicolor/512x512/apps/vector.png|g" "${desktop_file_path}"
    sed -i "s|Exec=vector|Exec=$HOME/.local/vector$suffix.app/bin/vector|g" "${desktop_file_path}"
}

macos() {
    echo "Downloading Zed"
    curl "https://cloud.zed.dev/releases/$channel/latest/download?asset=zed&os=macos&arch=$arch&source=install.sh" > "$temp/Zed-$arch.dmg"
    hdiutil attach -quiet "$temp/Zed-$arch.dmg" -mountpoint "$temp/mount"
    app="$(cd "$temp/mount/"; echo *.app)"
    echo "Installing $app"
    if [ -d "/Applications/$app" ]; then
        echo "Removing existing $app"
        rm -rf "/Applications/$app"
    fi
    ditto "$temp/mount/$app" "/Applications/$app"
    hdiutil detach -quiet "$temp/mount"

    mkdir -p "$HOME/.local/bin"
    # Link the binary
    ln -sf "/Applications/$app/Contents/MacOS/cli" "$HOME/.local/bin/vector"
}

main "$@"
