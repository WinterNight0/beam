#!/bin/sh
# Installs beam for the current Linux user: the programs, their menu entries
# and their icons. No root needed; nothing outside your home is touched.
#
#   sh scripts/linux/install.sh               install, or update to the latest build
#   sh scripts/linux/install.sh --no-gui      only the command line and full-screen view
#   sh scripts/linux/install.sh --gui         with the desktop window, without asking
#   sh scripts/linux/install.sh --uninstall   remove it again
#
# Without --gui or --no-gui it asks whether to install the desktop window
# (beam-gui), and the answer starts on yes. With no terminal to ask on, it
# installs the window.
#
# What it does:
#   1. takes `beam` (and `beam-gui`) from next to this script if they are
#      there (a copy handed over without the source), otherwise builds them:
#      cargo build --release -p beam [-p beam-gui];
#   2. copies them to ~/.local/bin;
#   3. copies the menu entries (packaging/linux/*.desktop) to
#      ~/.local/share/applications and the icons to ~/.local/share/icons.
# Without the window, an earlier install of it is removed. It never touches
# ~/.beam, where the device key and paired peers live.
#
# The window finds its icon on Wayland through beam-gui.desktop: the window's
# application id is `beam-gui`, the same name.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin
apps=$data/applications
icons=$data/icons/hicolor
sizes="48x48 128x128 256x256"

usage() {
    sed -n '5,8p' "$0" | sed 's/^# //'
}

refresh() {
    # Tell the desktop the menu and the icons changed. Both tools are
    # optional: without them the desktop notices on its own, a little later.
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database -q "$apps" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
        gtk-update-icon-cache -q -t "$icons" || true
}

# Removes one program with its menu entry and icons.
remove() {
    rm -f "$bin/$1" "$apps/$1.desktop"
    for s in $sizes; do rm -f "$icons/$s/apps/$1.png"; done
}

gui=ask
uninstall=no
for arg in "$@"; do
    case $arg in
        --gui) gui=yes ;;
        --no-gui) gui=no ;;
        --uninstall) uninstall=yes ;;
        -h | --help) usage; exit 0 ;;
        *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
    esac
done

if [ "$uninstall" = yes ]; then
    remove beam
    remove beam-gui
    refresh
    echo "beam is uninstalled. Your keys and paired peers in ~/.beam were not touched."
    exit 0
fi

if [ "$gui" = ask ]; then
    if [ -t 0 ]; then
        printf 'Install the desktop window (beam-gui) too? [Y/n] '
        read -r answer || answer=
        case $answer in
            [nN]*) gui=no ;;
            *) gui=yes ;;
        esac
    else
        gui=yes
    fi
fi
programs=beam
[ "$gui" = yes ] && programs="beam beam-gui"

# 1. Find or build the programs.
handed_over=yes
for p in $programs; do
    [ -x "$here/$p" ] || handed_over=no
done
if [ "$handed_over" = yes ]; then
    from=$here
    share=$here
else
    if ! command -v cargo >/dev/null 2>&1; then
        [ -x "$HOME/.cargo/bin/cargo" ] && PATH="$HOME/.cargo/bin:$PATH" || {
            echo "No beam next to this script, and cargo was not found to build it." >&2
            echo "Install Rust from https://rustup.rs/" >&2
            exit 1
        }
    fi
    packages=
    for p in $programs; do packages="$packages -p $p"; done
    echo "==> cargo build --release$packages"
    # shellcheck disable=SC2086 # one -p per program
    (cd "$repo" && cargo build --release $packages)
    from=$repo/target/release
    share=$repo/packaging/linux
fi
# A handed-over copy carries its menu entries and icons beside the programs.
[ -d "$share/icons" ] || share=$repo/packaging/linux

# 2. The programs.
mkdir -p "$bin"
for p in $programs; do
    install -m 755 "$from/$p" "$bin/$p"
done

# 3. Menu entries and icons.
mkdir -p "$apps"
for p in $programs; do
    install -m 644 "$share/$p.desktop" "$apps/$p.desktop"
    for s in $sizes; do
        mkdir -p "$icons/$s/apps"
        install -m 644 "$share/icons/hicolor/$s/apps/$p.png" "$icons/$s/apps/$p.png"
    done
done
[ "$gui" = yes ] || remove beam-gui
refresh

if [ "$gui" = yes ]; then
    echo "Installed beam and beam-gui to $bin, with menu entries and icons."
else
    echo "Installed beam to $bin, with its menu entry and icon (no desktop window)."
fi
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Note: $bin is not on your PATH. Add it to run \`beam\` from a terminal." ;;
esac
