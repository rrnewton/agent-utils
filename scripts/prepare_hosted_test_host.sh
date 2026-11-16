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

# Only this disposable VM is changed. The tests must really create nested user
# and mount namespaces; a capability skip must not count as passing coverage.
if [[ -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]]; then
    sudo -n sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
fi
unshare --user --map-root-user --mount --fork true

# The image migration tests require a NOCOW-capable backing filesystem. An ext4
# /tmp cannot satisfy that contract even if mkfs.btrfs and sudo are installed.
# Keep their temporary files on a bounded, disposable btrfs loop filesystem.
test_host_root="$RUNNER_TEMP/agent-utils-test-host"
mkdir -m 700 "$test_host_root"
truncate -s 16G "$test_host_root/filesystem.img"
mkfs.btrfs -q -f "$test_host_root/filesystem.img"
mkdir -m 700 "$test_host_root/mount"
sudo -n mount -o loop,noatime "$test_host_root/filesystem.img" "$test_host_root/mount"
sudo -n chown "$(id -u):$(id -g)" "$test_host_root/mount"
chmod 700 "$test_host_root/mount"
mkdir -m 700 "$test_host_root/mount/tmp"
test "$(findmnt -n -o FSTYPE --target "$test_host_root/mount/tmp")" = btrfs
touch "$test_host_root/mount/tmp/nocow-probe"
chattr +C "$test_host_root/mount/tmp/nocow-probe"
lsattr "$test_host_root/mount/tmp/nocow-probe"
rm "$test_host_root/mount/tmp/nocow-probe"

# Each later Bash step inherits the private umask, including Rust tests which
# create their own temporary directories. Python inherits the chosen TMPDIR.
printf 'umask 077\n' > "$test_host_root/bash-env"
printf 'TMPDIR=%s\nBASH_ENV=%s\n' \
    "$test_host_root/mount/tmp" "$test_host_root/bash-env" >> "$GITHUB_ENV"
printf 'Test host ready: Ubuntu %s, canonical sudo, user namespaces, btrfs TMPDIR=%s\n' \
    "$VERSION_ID" "$test_host_root/mount/tmp"
