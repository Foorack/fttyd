#!/usr/bin/env bash
# Runs inside the container started by scripts/docker-tests.sh. Root is available, so
# this covers what the host test suites cannot: privilege dropping, socket ownership,
# permission errors and special files.
set -uo pipefail

apt-get update -qq >/dev/null 2>&1 || true
for package in curl unzip procps; do
  if ! command -v "$package" >/dev/null 2>&1; then
    apt-get install -y -qq "$package" >/dev/null 2>&1 || { echo "error: could not install $package"; exit 1; }
  fi
done

failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }
pass() { echo "ok: $*"; }

wait_for_port() {
  local port="$1"
  for _ in $(seq 1 50); do
    if curl -sf -o /dev/null "http://127.0.0.1:${port}/api/info"; then return 0; fi
    sleep 0.1
  done
  return 1
}

# ---------------------------------------------------------------------------
echo "--- unix socket with --socket-owner ---"
socket=/tmp/fttyd.sock
rm -f "$socket"
fttyd -W -i "$socket" -U nobody:nogroup bash -c 'sleep 30' >/tmp/socket.log 2>&1 &
server_pid=$!
for _ in $(seq 1 50); do [[ -S "$socket" ]] && break; sleep 0.1; done
if [[ ! -S "$socket" ]]; then
  fail "socket $socket was not created"
else
  owner="$(stat -c '%U:%G' "$socket")"
  [[ "$owner" == "nobody:nogroup" ]] && pass "socket owned by $owner" || fail "socket owned by $owner"
  code="$(curl -s --unix-socket "$socket" -o /dev/null -w '%{http_code}' http://localhost/api/info)"
  [[ "$code" == "200" ]] && pass "http over unix socket" || fail "http over unix socket returned $code"
fi
kill "$server_pid" 2>/dev/null
wait "$server_pid" 2>/dev/null
rm -f "$socket"

# ---------------------------------------------------------------------------
echo "--- --uid/--gid drop privileges after binding ---"
id nobody >/dev/null 2>&1 || fail "no nobody user"
fttyd -W -p 17681 -u 65534 -g 65534 bash -c 'id -u' >/tmp/uid.log 2>&1 &
server_pid=$!
if wait_for_port 17681; then
  pass "server bound before dropping privileges"
else
  fail "server did not come up as uid 65534"
fi

# The server process itself must have dropped to the requested ids; every terminal
# child inherits them (the per-connection child is covered by the host tests).
uid_line="$(awk '/^Uid:/ {print $2}' "/proc/${server_pid}/status" 2>/dev/null)"
gid_line="$(awk '/^Gid:/ {print $2}' "/proc/${server_pid}/status" 2>/dev/null)"
[[ "$uid_line" == "65534" ]] && pass "server runs as uid 65534" || fail "server uid is ${uid_line:-unknown}"
[[ "$gid_line" == "65534" ]] && pass "server runs as gid 65534" || fail "server gid is ${gid_line:-unknown}"
kill "$server_pid" 2>/dev/null
wait "$server_pid" 2>/dev/null

# ---------------------------------------------------------------------------
echo "--- permission errors are per entry, not per listing ---"
mkdir -p /srv/readable /srv/locked
echo hello > /srv/readable/visible.txt
echo secret > /srv/locked/secret.txt
chmod 700 /srv/locked

fttyd -W -p 17683 -u 65534 -g 65534 bash -c 'sleep 30' >/tmp/perm.log 2>&1 &
server_pid=$!
wait_for_port 17683
readable="$(curl -s 'http://127.0.0.1:17683/api/fs?path=/srv/readable')"
[[ "$readable" == *"visible.txt"* ]] && pass "readable directory listed" || fail "readable listing: $readable"

locked_status="$(curl -s -o /dev/null -w '%{http_code}' 'http://127.0.0.1:17683/api/fs?path=/srv/locked')"
[[ "$locked_status" == "403" ]] && pass "locked directory returns 403" || fail "locked directory returned $locked_status"

root_listing="$(curl -s 'http://127.0.0.1:17683/api/fs?path=/srv')"
[[ "$root_listing" == *"locked"* ]] && pass "locked entry still listed from its parent" || fail "parent listing: $root_listing"
kill "$server_pid" 2>/dev/null
wait "$server_pid" 2>/dev/null

# ---------------------------------------------------------------------------
echo "--- /proc and /sys listings do not hang ---"
fttyd -W -p 17684 bash -c 'sleep 30' >/tmp/proc.log 2>&1 &
server_pid=$!
wait_for_port 17684
for path in /proc /sys /dev; do
  status="$(timeout 10 curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:17684/api/fs?path=${path}")"
  [[ "$status" == "200" ]] && pass "listed $path" || fail "listing $path returned $status"
done
# Downloading a special file must be refused, not hung on.
status="$(timeout 10 curl -s -o /dev/null -w '%{http_code}' 'http://127.0.0.1:17684/api/download?path=/dev/zero')"
[[ "$status" == "400" ]] && pass "/dev/zero download refused" || fail "/dev/zero download returned $status"

# Archiving a directory containing special files completes and reports them.
timeout 30 curl -s -o /tmp/dev.zip 'http://127.0.0.1:17684/api/archive?path=/dev&format=zip' || fail "archive of /dev failed"
if [[ -s /tmp/dev.zip ]] && unzip -t /tmp/dev.zip >/dev/null 2>&1; then
  pass "archive of /dev is a valid zip"
  unzip -l /tmp/dev.zip | grep -q "fttyd-archive-report.txt" \
    && pass "special files reported in the archive" \
    || fail "special file report missing"
else
  fail "archive of /dev is not a valid zip"
fi
kill "$server_pid" 2>/dev/null
wait "$server_pid" 2>/dev/null

# ---------------------------------------------------------------------------
echo "--- disk-full upload is refused with 507 ---"
mkdir -p /mnt/tiny
mount -t tmpfs -o size=64k tmpfs /mnt/tiny 2>/dev/null || echo "note: cannot mount tmpfs, skipping disk-full check"
if mountpoint -q /mnt/tiny; then
  fttyd -W -p 17685 bash -c 'sleep 30' >/tmp/full.log 2>&1 &
  server_pid=$!
  wait_for_port 17685
  head -c 200000 /dev/zero > /tmp/big.bin
  status="$(curl -s -o /dev/null -w '%{http_code}' -X PUT \
    -H 'x-fttyd-token:' -H 'content-range: bytes 0-199999/200000' \
    -H 'content-type: application/octet-stream' --data-binary @/tmp/big.bin \
    'http://127.0.0.1:17685/api/upload?path=/mnt/tiny/big.bin')"
  [[ "$status" == "507" ]] && pass "upload rejected with 507" || fail "upload to a full filesystem returned $status"
  umount /mnt/tiny
  kill "$server_pid" 2>/dev/null
  wait "$server_pid" 2>/dev/null
fi

echo
if [[ "$failures" -ne 0 ]]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all container checks passed"
