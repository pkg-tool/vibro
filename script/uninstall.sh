#!/usr/bin/env sh
set -eu

# Uninstalls Vector that was installed using the install.sh script

check_remaining_installations() {
    platform="$(uname -s)"
    if [ "$platform" = "Darwin" ]; then
        # Check for any Vector variants in /Applications
        remaining=$(ls -d /Applications/Vector*.app 2>/dev/null | wc -l)
        [ "$remaining" -eq 0 ]
    else
        # Check for any Vector variants in ~/.local
        remaining=$(ls -d "$HOME/.local/vector"*.app 2>/dev/null | wc -l)
        [ "$remaining" -eq 0 ]
    fi
}

prompt_remove_preferences() {
    printf "Do you want to keep your Vector preferences? [Y/n] "
    read -r response
    case "$response" in
        [nN]|[nN][oO])
            rm -rf "$HOME/.config/vector"
            echo "Preferences removed."
            ;;
        *)
            echo "Preferences kept."
            ;;
    esac
}

main() {
    platform="$(uname -s)"
    channel="${VECTOR_CHANNEL:-stable}"

    if [ "$platform" = "Darwin" ]; then
        platform="macos"
    elif [ "$platform" = "Linux" ]; then
        platform="linux"
    else
        echo "Unsupported platform $platform"
        exit 1
    fi

    "$platform"

    echo "Vector has been uninstalled"
}

linux() {
    suffix=""
    if [ "$channel" != "stable" ]; then
        suffix="-$channel"
    fi

    appid=""
    db_suffix="stable"
    case "$channel" in
      stable)
        appid="dev.vector.Vector"
        db_suffix="stable"
        ;;
      nightly)
        appid="dev.vector.Vector-Nightly"
        db_suffix="nightly"
        ;;
      preview)
        appid="dev.vector.Vector-Preview"
        db_suffix="preview"
        ;;
      dev)
        appid="dev.vector.Vector-Dev"
        db_suffix="dev"
        ;;
      *)
        echo "Unknown release channel: ${channel}. Using stable app ID."
        appid="dev.vector.Vector"
        db_suffix="stable"
        ;;
    esac

    # Remove the app directory
    rm -rf "$HOME/.local/vector$suffix.app"

    # Remove the binary symlink
    rm -f "$HOME/.local/bin/vector"

    # Remove the .desktop file
    rm -f "$HOME/.local/share/applications/${appid}.desktop"

    # Remove the database directory for this channel
    rm -rf "$HOME/.local/share/vector/db/0-$db_suffix"

    # Remove socket file
    rm -f "$HOME/.local/share/vector/vector-$db_suffix.sock"

    # Remove the entire Vector directory if no installations remain
    if check_remaining_installations; then
        rm -rf "$HOME/.local/share/vector"
        prompt_remove_preferences
    fi

    rm -rf "$HOME/.vector_server"
}

macos() {
    app="Vector.app"
    db_suffix="stable"
    app_id="dev.vector.Vector"
    case "$channel" in
      nightly)
        app="Vector Nightly.app"
        db_suffix="nightly"
        app_id="dev.vector.Vector-Nightly"
        ;;
      preview)
        app="Vector Preview.app"
        db_suffix="preview"
        app_id="dev.vector.Vector-Preview"
        ;;
      dev)
        app="Vector Dev.app"
        db_suffix="dev"
        app_id="dev.vector.Vector-Dev"
        ;;
    esac

    # Remove the app bundle
    if [ -d "/Applications/$app" ]; then
        rm -rf "/Applications/$app"
    fi

    # Remove the binary symlink
    rm -f "$HOME/.local/bin/vector"

    # Remove the database directory for this channel
    rm -rf "$HOME/Library/Application Support/Vector/db/0-$db_suffix"

    # Remove app-specific files and directories
    rm -rf "$HOME/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments/$app_id.sfl"*
    rm -rf "$HOME/Library/Caches/$app_id"
    rm -rf "$HOME/Library/HTTPStorages/$app_id"
    rm -rf "$HOME/Library/Preferences/$app_id.plist"
    rm -rf "$HOME/Library/Saved Application State/$app_id.savedState"

    # Remove the entire Vector directory if no installations remain
    if check_remaining_installations; then
        rm -rf "$HOME/Library/Application Support/Vector"
        rm -rf "$HOME/Library/Logs/Vector"

        prompt_remove_preferences
    fi

    rm -rf "$HOME/.vector_server"
}

main "$@"
