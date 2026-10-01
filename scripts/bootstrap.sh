#!/usr/bin/env bash
# Install/check the native prerequisites required by GPUI Kit 0.7.
set -euo pipefail

case "$(uname -s)" in
  Darwin)
    if xcode-select -p >/dev/null 2>&1; then
      echo "Xcode Command Line Tools: $(xcode-select -p)"
    else
      echo 'Xcode Command Line Tools are required. Run: xcode-select --install' >&2
      exit 1
    fi
    ;;
  Linux)
    if ! command -v apt-get >/dev/null 2>&1; then
      echo 'This bootstrap script supports apt-based Linux distributions.' >&2
      echo 'Install the equivalent GPUI Kit Vulkan, Wayland, X11, fontconfig, OpenSSL, and zstd development packages.' >&2
      exit 1
    fi
    sudo apt-get update
    sudo apt-get install -y \
      gcc g++ clang \
      libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev \
      libssl-dev libzstd-dev libvulkan1 mesa-vulkan-drivers
    echo 'Linux build prerequisites installed.'
    ;;
  *)
    echo 'Unsupported host. On Windows run scripts/bootstrap.ps1 from PowerShell.' >&2
    exit 1
    ;;
esac
