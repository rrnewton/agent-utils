#!/usr/bin/env bash
# The privileged filesystem tests need an actual Linux host, not capability skips.
# Only call this on an ephemeral GitHub-hosted runner, never on a developer host.
set -euo pipefail

if [[ ${GITHUB_ACTIONS:-} != true || ${AGENT_UTILS_RUNNER_ENVIRONMENT:-} != github-hosted ]]; then
    echo 'REFUSED: host preparation is only for an ephemeral GitHub-hosted runner' >&2
    exit 2
fi
: "${RUNNER_TEMP:?runner temporary directory is required}"
: "${GITHUB_ENV:?GitHub environment export file is required}"
umask 077

# The production trusted-executable proof requires canonical /usr/bin executables.
# Ubuntu 26.04 replaced sudo with a symlinked Rust implementation. Pin this test
# host to 24.04 instead of weakening the ownership proof for a new platform.
source /etc/os-release
if [[ ${ID:-} != ubuntu || ${VERSION_ID:-} != 24.04 ]]; then
    echo 'REFUSED: this test-host contract requires Ubuntu 24.04' >&2
    exit 2
fi
sudo -n apt-get update -qq
sudo -n apt-get install -y -qq btrfs-progs fuse3 fuse2fs lsof zstd
test "$(readlink -f /usr/bin/sudo)" = /usr/bin/sudo
test "$(stat -c %u /usr/bin/sudo)" = 0
PYTHONPATH=py python3 - <<'PY'
from pathlib import Path
from wrkslots.cli import _root_owned_executable
print(_root_owned_executable(Path('/usr/bin/sudo'), 'sudo'))
PY
sudo -n true

# Image-removal safety proves every bound loop filesystem's identity. Hosted
# images may leave udev inactive; without its UUID database, the btrfs test
# backing device looks unidentified and the production guard rightly refuses.
# Start it only on this disposable VM, before creating any test loop devices.
sudo -n systemctl start systemd-udevd.service
sudo -n udevadm trigger --action=change --subsystem-match=block
sudo -n udevadm settle --timeout=30

# Replay/commit-tree fixtures create fresh bare repositories, so repository-local
# identity from the checkout cannot supply their committer. This VM is disposable.
git config --global user.name 'agent-utils CI'
git config --global user.email 'agent-utils-ci@example.invalid'

# Only this disposable VM is changed. The tests must really create nested user
# and mount namespaces; a capability skip must not count as passing coverage.
if [[ -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]]; then
    sudo -n sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
fi
unshare --user --map-root-user --mount --fork true
# Lock delegation proves the coordinator's open file description with
# pidfd_getfd. Permit same-user non-descendant ptrace on this disposable VM;
# production retains its fail-closed proof on restricted hosts.
if [[ -e /proc/sys/kernel/yama/ptrace_scope ]]; then
    sudo -n sysctl -w kernel.yama.ptrace_scope=0
fi

# The image migration tests require a NOCOW-capable backing filesystem. An ext4
# /tmp cannot satisfy that contract even if mkfs.btrfs and sudo are installed.
# Keep their temporary files on a bounded, disposable btrfs loop filesystem.
test_host_root="$RUNNER_TEMP/agent-utils-test-host"
mkdir -m 700 "$test_host_root"
truncate -s 16G "$test_host_root/filesystem.img"
mkfs.btrfs -q -f "$test_host_root/filesystem.img"
# Tests explicitly prove exclusions against /tmp, and Unix sockets have a
# 108-byte pathname bound. Preserve the disposable VM's existing /tmp entries
# before exposing the btrfs mount there. No developer host can reach this code.
test_tmp="$test_host_root/mount"
mkdir -m 700 "$test_tmp"
sudo -n mount -o loop,noatime "$test_host_root/filesystem.img" "$test_tmp"
sudo -n cp -a /tmp/. "$test_tmp/"
sudo -n chown root:root "$test_tmp"
sudo -n chmod 1777 "$test_tmp"
sudo -n mount --bind "$test_tmp" /tmp
test "$(findmnt -n -o FSTYPE --target /tmp)" = btrfs
test_loop_device=$(findmnt -n -o SOURCE --target "$test_tmp")
sudo -n udevadm settle --timeout=30
test_loop_uuid=$(sudo -n blkid -s UUID -o value "$test_loop_device")
test -n "$test_loop_uuid"
# Read the same world-readable udev evidence the unprivileged tests consume.
udevadm info --query=property --name="$test_loop_device" | \
    grep -Fx "ID_FS_UUID=$test_loop_uuid"
probe=$(mktemp /tmp/nocow.XXXXXX)
chattr +C "$probe"
lsattr "$probe"
rm "$probe"
# Do not override the test process umask: permission tests exercise its default.
printf 'TMPDIR=/tmp\n' >> "$GITHUB_ENV"
printf 'Test host ready: Ubuntu %s, canonical sudo, user namespaces, btrfs /tmp\n' \
    "$VERSION_ID"
