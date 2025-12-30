# Vector on Linux

## Standard Installation

For most people we recommend using the script on the [download](https://vector.dev/download) page to install Vector:

```sh
curl -f https://vector.dev/install.sh | sh
```

We also offer a preview build of Vector which receives updates about a week ahead of stable. You can install it with:

```sh
curl -f https://vector.dev/install.sh | VECTOR_CHANNEL=preview sh
```

The Vector installed by the script works best on systems that:

- have a Vulkan compatible GPU available (for example Linux on an M-series macBook)
- have a system-wide glibc (NixOS and Alpine do not by default)
  - x86_64 (Intel/AMD): glibc version >= 2.31 (Ubuntu 20 and newer)
  - aarch64 (ARM): glibc version >= 2.35 (Ubuntu 22 and newer)

Both Nix and Alpine have third-party Vector packages available (though they are currently a few weeks out of date). If you'd like to use our builds they do work if you install a glibc compatibility layer. On NixOS you can try [nix-ld](https://github.com/Mic92/nix-ld), and on Alpine [gcompat](https://wiki.alpinelinux.org/wiki/Running_glibc_programs).

You will need to build from source for:

- architectures other than 64-bit Intel or 64-bit ARM (for example a 32-bit or RISC-V machine)
- Redhat Enterprise Linux 8.x, Rocky Linux 8, AlmaLinux 8, Amazon Linux 2 on all architectures
- Redhat Enterprise Linux 9.x, Rocky Linux 9.3, AlmaLinux 8, Amazon Linux 2023 on aarch64 (x86_x64 OK)

## Other ways to install Vector on Linux

Vector is open source, and [you can install from source](./development/linux.md).

### Installing via a package manager

There are third-party Vector packages for various Linux distributions and package managers. Names may vary; search for `vector` or `vector-editor` in your package manager.

When installing a third-party package please be aware that it may not be completely up to date and may be slightly different from the Vector we package.

We'd love your help making Vector available for everyone. If Vector is not yet available for your package manager, and you would like to fix that, we have some notes on [how to do it](./development/linux.md#notes-for-packaging-vector).

### Downloading manually

If you'd prefer, you can install Vector by downloading our pre-built .tar.gz. This is the same artifact that our install script uses, but you can customize the location of your installation by modifying the instructions below:

Download the `.tar.gz` file:

- [vector-linux-x86_64.tar.gz](https://vector.dev/api/releases/stable/latest/vector-linux-x86_64.tar.gz) ([preview](https://vector.dev/api/releases/preview/latest/vector-linux-x86_64.tar.gz))
- [vector-linux-aarch64.tar.gz](https://vector.dev/api/releases/stable/latest/vector-linux-aarch64.tar.gz)
  ([preview](https://vector.dev/api/releases/preview/latest/vector-linux-aarch64.tar.gz))

Then ensure that the `vector` binary in the tarball is on your path. The easiest way is to unpack the tarball and create a symlink:

```sh
mkdir -p ~/.local
# extract vector to ~/.local/vector.app/
tar -xvf <path/to/download>.tar.gz -C ~/.local
# link the vector binary to ~/.local/bin (or another directory in your $PATH)
ln -sf ~/.local/vector.app/bin/vector ~/.local/bin/vector
```

If you'd like integration with an XDG-compatible desktop environment, you will also need to install the `.desktop` file:

```sh
cp ~/.local/vector.app/share/applications/vector.desktop ~/.local/share/applications/dev.vector.Vector.desktop
sed -i "s|Icon=vector|Icon=$HOME/.local/vector.app/share/icons/hicolor/512x512/apps/vector.png|g" ~/.local/share/applications/dev.vector.Vector.desktop
sed -i "s|Exec=vector|Exec=$HOME/.local/vector.app/libexec/vector-editor|g" ~/.local/share/applications/dev.vector.Vector.desktop
```

## Uninstalling Vector

### Standard Uninstall

If Vector was installed using the default installation script, it can be uninstalled by supplying the `--uninstall` flag to the `vector` command

```sh
vector --uninstall
```

If there are no errors, the shell will then prompt you whether you'd like to keep your preferences or delete them. After making a choice, you should see a message that Vector was successfully uninstalled.

In the case that the `vector` command was not found in your PATH, you can try one of the following commands

```sh
$HOME/.local/bin/vector --uninstall
```

The first case might fail if a symlink was not properly established between `$HOME/.local/bin/vector` and `$HOME/.local/vector.app/bin/vector`. But it should work as long as Vector was installed to its default location.

If Vector was installed to a different location, you must invoke the `vector` binary stored in that installation directory and pass the `--uninstall` flag to it in the same format as the previous commands.

### Package Manager

If Vector was installed using a package manager, please consult the documentation for that package manager on how to uninstall a package.

## Troubleshooting

Linux works on a large variety of systems configured in many different ways. We primarily test Vector on a vanilla Ubuntu setup, as it is the most common distribution our users use, that said we do expect it to work on a wide variety of machines.

### Vector fails to start

If you see an error like "/lib64/libc.so.6: version 'GLIBC_2.29' not found" it means that your distribution's version of glibc is too old. You can either upgrade your system, or [install Vector from source](./development/linux.md).

### Graphics issues

### Vector fails to open windows

Vector requires a GPU to run effectively. Under the hood, we use [Vulkan](https://www.vulkan.org/) to communicate with your GPU. If you are seeing problems with performance, or Vector fails to load, it is possible that Vulkan is the culprit.

If you see a notification saying `Vector failed to open a window: NoSupportedDeviceFound` this means that Vulkan cannot find a compatible GPU. You can begin troubleshooting Vulkan by installing the `vulkan-tools` package and running:

```sh
vkcube
```

This should output a line describing your current graphics setup and show a rotating cube. If this does not work, you should be able to fix it by installing Vulkan compatible GPU drivers, however in some cases (for example running Linux on an Arm-based MacBook) there is no Vulkan support yet.

You can find out which graphics card Vector is using by looking in the Vector log (`~/.local/share/vector/logs/Vector.log`) for `Using GPU: ...`.

If you see errors like `ERROR_INITIALIZATION_FAILED` or `GPU Crashed` or `ERROR_SURFACE_LOST_KHR` then you may be able to work around this by installing different drivers for your GPU, or by selecting a different GPU to run on.

On some systems the file `/etc/prime-discrete` can be used to enforce the use of a discrete GPU using [PRIME](https://wiki.archlinux.org/title/PRIME). Depending on the details of your setup, you may need to change the contents of this file to "on" (to force discrete graphics) or "off" (to force integrated graphics).

On others, you may be able to the environment variable `DRI_PRIME=1` when running Vector to force the use of the discrete GPU.

If you're using an AMD GPU and Vector crashes when selecting long lines, try setting the `VECTOR_PATH_SAMPLE_COUNT=0` environment variable.
If you're using an AMD GPU, you might get a 'Broken Pipe' error. Try using the RADV or Mesa drivers.

If you are using Mesa, and want more control over which GPU is selected you can run `MESA_VK_DEVICE_SELECT=list vector --foreground` to get a list of available GPUs and then export `MESA_VK_DEVICE_SELECT=xxxx:yyyy` to choose a specific device.

If you are using `amdvlk` you may find that Vector only opens when run with `sudo $(which vector)`. To fix this, remove the `amdvlk` and `lib32-amdvlk` packages and install mesa/vulkan instead.

For more information, the [Arch guide to Vulkan](https://wiki.archlinux.org/title/Vulkan) has some good steps that translate well to most distributions.

If Vulkan is configured correctly, and Vector is still not working for you, please file an issue with as much information as possible.

### I can't open any files

### Clicking links isn't working

These features are provided by XDG desktop portals, specifically:

- `org.freedesktop.portal.FileChooser`
- `org.freedesktop.portal.OpenURI`

Some window managers, such as `Hyprland`, don't provide a file picker by default. See [this list](https://wiki.archlinux.org/title/XDG_Desktop_Portal#List_of_backends_and_interfaces) as a starting point for alternatives.

### Vector isn't remembering my API keys

These feature also requires XDG desktop portals, specifically:

- `org.freedesktop.portal.Secret` or
- `org.freedesktop.Secrets`

Vector needs a place to securely store secrets such as API keys, and uses a system-provided keychain to do this. Examples of packages that provide this are `gnome-keyring`, `KWallet` and `keepassxc` among others.

### Could not start inotify

Vector relies on inotify to watch your filesystem for changes. If you cannot start inotify then Vector will not work reliably.

If you are seeing "too many open files" then first try `sysctl fs.inotify`.

- You should see that max_user_instances is 128 or higher (you can change the limit with `sudo sysctl fs.inotify.max_user_instances=1024`). Vector needs only 1 inotify instance.
- You should see that `max_user_watches` is 8000 or higher (you can change the limit with `sudo sysctl fs.inotify.max_user_watches=64000`). Vector needs one watch per directory in all your open projects + one per git repository + a handful more for settings, themes, keymaps, extensions.

It is also possible that you are running out of file descriptors. You can check the limits with `ulimit` and update them by editing `/etc/security/limits.conf`.

### No sound or wrong output device

If you're not hearing any sound in Vector or the audio is routed to the wrong device, it could be due to a mismatch between audio systems. Vector relies on ALSA, while your system may be using PipeWire or PulseAudio. To resolve this, you need to configure ALSA to route audio through PipeWire/PulseAudio.

If your system uses PipeWire:

1. **Install the PipeWire ALSA plugin**

   On Debian-based systems, run:

   ```bash
   sudo apt install pipewire-alsa
   ```

2. **Configure ALSA to use PipeWire**

   Add the following configuration to your ALSA settings file. You can use either `~/.asoundrc` (user-level) or `/etc/asound.conf` (system-wide):

   ```bash
   pcm.!default {
       type pipewire
   }

   ctl.!default {
       type pipewire
   }
   ```

3. **Restart your system**
