#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# heptix - the missing haptic feedback for Linux!
# Copyright (C) 2026 M Mustakim Ali
#
# This program is free software; you can redistribute it and/or modify
# it under the terms of the GNU General Public License version 2 as
# published by the Free Software Foundation.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program; if not, see <https://www.gnu.org/licenses/>.

# Runs inside the container started by run.sh, as root, with /stage holding the
# static heptix binary, packaging/ and install.sh.
set -u
PASS=0
FAIL=0

check() {
  desc=$1
  shift
  if "$@" >/tmp/check.out 2>&1; then
    echo "ok   - $desc"
    PASS=$((PASS + 1))
  else
    echo "FAIL - $desc"
    sed 's/^/         /' /tmp/check.out | head -20
    FAIL=$((FAIL + 1))
  fi
}

# Runs a command as the login user, with that user's systemd manager reachable.
as_tester() {
  runuser -u tester -- env XDG_RUNTIME_DIR=/run/user/1000 \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus HOME=/home/tester "$@"
}

# wait_for <seconds> <command...>: retries until the command succeeds.
wait_for() {
  n=$1
  shift
  while [ "$n" -gt 0 ]; do
    "$@" >/dev/null 2>&1 && return 0
    sleep 1
    n=$((n - 1))
  done
  return 1
}

reader_log() { journalctl -u heptix-reader.service --no-pager -o cat 2>/dev/null; }
client_log() { journalctl --no-pager -o cat _UID=1000 _SYSTEMD_USER_UNIT=heptix.service 2>/dev/null; }
reader_pid() { systemctl show -p MainPID --value heptix-reader.service; }
client_pid() { as_tester systemctl --user show -p MainPID --value heptix.service; }

booted() { s=$(systemctl is-system-running 2>/dev/null); [ "$s" = running ] || [ "$s" = degraded ]; }
echo "== booting systemd"
wait_for 90 booted || { echo "systemd did not boot"; exit 2; }
loginctl enable-linger tester
wait_for 30 test -S /run/user/1000/bus || { echo "no user manager for tester"; exit 2; }
useradd -m -u 1001 other 2>/dev/null
# A container has no seat; serve the test user the way a headless machine would.
mkdir -p /etc/heptix
echo 1000 >/etc/heptix/allow-uids

# --- service mode -----------------------------------------------------------
echo "== service mode install"
as_tester env HEPTIX_LOCAL_DIR=/stage sh /stage/install.sh --service --yes >/tmp/install-service.log 2>&1
check "installer exits cleanly" test $? -eq 0
check "binary is installed root-owned in /usr/lib/heptix" sh -c '[ "$(stat -c "%U %a" /usr/lib/heptix/heptix)" = "root 755" ]'
check "unit files are installed" sh -c 'test -f /etc/systemd/system/heptix-reader.service && test -f /etc/systemd/user/heptix.service'
check "the heptix system user exists" getent passwd heptix
check "the login user is not in the input group" sh -c '! id -nG tester | tr " " "\n" | grep -qx input'
check "heptix is on PATH" test -x /usr/local/bin/heptix
check "reader service is active" wait_for 20 systemctl is-active --quiet heptix-reader.service
check "socket is world-connectable and owned by heptix" sh -c '[ "$(stat -c "%U %a" /run/heptix/events.sock)" = "heptix 666" ]'
check "client service is active" wait_for 20 as_tester systemctl --user is-active --quiet heptix.service
check "client connected to the reader (verification passed)" wait_for 20 sh -c "journalctl --no-pager -o cat _UID=1000 _SYSTEMD_USER_UNIT=heptix.service | grep -q 'reader: connected'"
check "reader has refused nobody so far" sh -c '! journalctl -u heptix-reader.service --no-pager -o cat | grep -q refused'

echo "== reader process"
RP=$(reader_pid)
check "reader runs as the heptix user, not root" sh -c "[ \"\$(stat -c %U /proc/$RP)\" = heptix ]"
check "reader has no_new_privs and a seccomp filter" sh -c "grep -q '^NoNewPrivs:.1' /proc/$RP/status && grep -q '^Seccomp:.2' /proc/$RP/status"
check "reader's only capability is CAP_SYS_PTRACE" sh -c "grep -q '^CapEff:.0000000000080000' /proc/$RP/status"
check "reader is in the input group" sh -c "grep '^Groups:' /proc/$RP/status | tr ' \t' '\n' | grep -qx $(getent group input | cut -d: -f3)"
check "reader is in its own network namespace with only a loopback" sh -c "[ \"\$(readlink /proc/$RP/ns/net)\" != \"\$(readlink /proc/1/ns/net)\" ] && [ \"\$(nsenter -t $RP -n ip -o link | wc -l)\" = 1 ]"
check "reader holds no input device (nobody is subscribed or none exist)" sh -c "! ls -l /proc/$RP/fd | grep -q /dev/input"

echo "== client process"
CP=$(client_pid)
check "client is non-dumpable (its /proc entries are root-owned)" sh -c "[ \"\$(stat -c %U /proc/$CP/environ)\" = root ]"
check "client has no_new_privs" sh -c "grep -q '^NoNewPrivs:.1' /proc/$CP/status"

echo "== refusals"
cat >/tmp/probe.py <<'PYEND'
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(5)
s.connect("/run/heptix/events.sock")
# A well-formed subscription: type 0x80, flags, max_ms, scroll_ticks, no device path.
s.sendall(bytes([11, 0x80, 3, 200, 0, 0, 0, 24, 0, 0, 0, 0]))
try:
    data = s.recv(64)
except Exception as e:
    data = b""
sys.exit(0 if data == b"" else 1)
PYEND
check "another program of the same user gets no data" as_tester python3 /tmp/probe.py
check "another user gets no data" runuser -u other -- python3 /tmp/probe.py
check "the reader logged the refusals" wait_for 10 sh -c "journalctl -u heptix-reader.service --no-pager -o cat | grep -c refused | grep -qv '^0$'"
as_tester timeout 4 /usr/local/bin/heptix --client >/tmp/shell-client.log 2>&1
check "heptix started from a shell is refused by the reader" sh -c "grep -q 'refused' /tmp/shell-client.log"
check "the genuine client is still connected" as_tester systemctl --user is-active --quiet heptix.service

echo "== reader restart"
BEFORE=$(client_log | grep -c 'reader: connected')
systemctl restart heptix-reader.service
check "the client reconnects after the reader restarts" wait_for 30 sh -c "[ \$(journalctl --no-pager -o cat _UID=1000 _SYSTEMD_USER_UNIT=heptix.service | grep -c 'reader: connected') -gt $BEFORE ]"

echo "== uninstall"
as_tester env HEPTIX_LOCAL_DIR=/stage sh /stage/install.sh --uninstall >/tmp/uninstall.log 2>&1
check "uninstall exits cleanly" test $? -eq 0
check "system files are gone" sh -c '! test -e /usr/lib/heptix/heptix && ! test -e /etc/systemd/system/heptix-reader.service && ! test -e /etc/systemd/user/heptix.service && ! test -e /usr/local/bin/heptix'
check "reader service is stopped" sh -c '! systemctl is-active --quiet heptix-reader.service'

# --- direct mode ------------------------------------------------------------
echo "== direct mode install"
as_tester env HEPTIX_LOCAL_DIR=/stage sh /stage/install.sh --direct --yes >/tmp/install-direct.log 2>&1
check "installer exits cleanly" test $? -eq 0
check "binary is in ~/.local/bin" test -x /home/tester/.local/bin/heptix
check "user unit runs in explicit direct mode" grep -q -- '--direct' /home/tester/.config/systemd/user/heptix.service
check "the installer warned that direct mode is not recommended" grep -q 'not recommended' /tmp/install-direct.log
check "the user was added to the input group" sh -c 'id -nG tester | tr " " "\n" | grep -qx input'
check "no reader service was installed" sh -c '! test -e /etc/systemd/system/heptix-reader.service'

echo "== switching from direct to service mode"
as_tester env HEPTIX_LOCAL_DIR=/stage sh /stage/install.sh --service --yes >/tmp/install-switch.log 2>&1
check "installer exits cleanly" test $? -eq 0
check "the per-user direct install was removed" sh -c '! test -e /home/tester/.local/bin/heptix && ! test -e /home/tester/.config/systemd/user/heptix.service'
check "reader service is active again" wait_for 20 systemctl is-active --quiet heptix-reader.service
check "client reconnected through the system unit" wait_for 20 sh -c "journalctl --no-pager -o cat _UID=1000 _SYSTEMD_USER_UNIT=heptix.service | grep -c 'reader: connected' | grep -qv '^0$'"
check "the installer offered to remove the input group" grep -q "input' group" /tmp/install-switch.log

as_tester env HEPTIX_LOCAL_DIR=/stage sh /stage/install.sh --uninstall >/dev/null 2>&1
check "final uninstall leaves nothing behind" sh -c '! test -e /usr/lib/heptix && ! test -e /home/tester/.local/bin/heptix'

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
