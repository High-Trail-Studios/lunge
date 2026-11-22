#!/usr/bin/env bash
# Installs a Lunge .deb or .rpm on a clean distro container, confirms the
# window opens under Xvfb, then uninstalls it and checks nothing is left.
#
#   packaging/linux/smoke-test.sh IMAGE PACKAGE
#   packaging/linux/smoke-test.sh ubuntu:22.04 target/debian/lunge_0.5.0-1_amd64.deb
#
# The package is installed on its own first, before Xvfb, so its declared
# dependencies alone have to be enough to resolve. That's what catches
# runtime (dlopen) libraries missing from the package metadata, and a glibc
# requirement newer than the distro has. Needs Docker.
set -euo pipefail

if [ $# -ne 2 ]; then
  echo "usage: $0 IMAGE PACKAGE" >&2
  exit 2
fi
image=$1
pkg=$(realpath "$2")
name=$(basename "$pkg")

echo "::group::$name on $image"
docker run --rm -v "$pkg:/pkgs/$name:ro" "$image" bash -euc '
  pkg=$(echo /pkgs/*)
  if command -v apt-get >/dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends "$pkg" >/dev/null
    apt-get install -y -qq --no-install-recommends xvfb >/dev/null
    uninstall() { apt-get remove -y -qq lunge >/dev/null; }
  elif command -v dnf >/dev/null; then
    dnf install -y -q "$pkg"
    dnf install -y -q xorg-x11-server-Xvfb
    uninstall() { dnf remove -y -q lunge; }
  elif command -v zypper >/dev/null; then
    zypper --non-interactive --quiet install --allow-unsigned-rpm "$pkg"
    zypper --non-interactive --quiet install xorg-x11-server-Xvfb
    uninstall() { zypper --non-interactive --quiet remove lunge; }
  else
    echo "no supported package manager in this image" >&2
    exit 1
  fi
  . /etc/os-release
  echo "installed on $PRETTY_NAME ($(ldd --version | head -1))"

  Xvfb :99 -nolisten tcp >/dev/null 2>&1 &
  sleep 2
  # 124 = timeout fired while lunge was still running, which is the pass.
  rc=0; DISPLAY=:99 timeout 5 lunge || rc=$?
  [ "$rc" -eq 124 ] || { echo "lunge exited early ($rc)" >&2; exit 1; }
  echo "lunge started and stayed running"

  uninstall
  [ ! -e /usr/bin/lunge ] || { echo "/usr/bin/lunge left behind" >&2; exit 1; }
  echo "uninstalled cleanly"
'
echo "::endgroup::"
