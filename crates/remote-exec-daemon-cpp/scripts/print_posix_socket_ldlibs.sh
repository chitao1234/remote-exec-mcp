#!/bin/sh
set -eu

: "${HOST_CXX:=c++}"
: "${PROBE_CPPFLAGS:=}"
: "${PROBE_CXXFLAGS:=-std=c++11 -O2 -Wall -Wextra -pthread}"
: "${PROBE_LDFLAGS:=}"
: "${PROBE_LDLIBS:=-pthread}"

tmp_dir=${TMPDIR:-/tmp}/remote-exec-cpp-socket-libs.$$
source=$tmp_dir/probe.cpp
binary=$tmp_dir/probe
mkdir -p "$tmp_dir"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

cat > "$source" <<'EOF'
#include <netdb.h>
#include <sys/socket.h>
#include <sys/types.h>

int main() {
    struct addrinfo hints = {};
    struct addrinfo* result = 0;
    (void)getaddrinfo("127.0.0.1", "0", &hints, &result);
    freeaddrinfo(result);
    return socket(AF_INET, SOCK_STREAM, 0);
}
EOF

for libs in "" "-lsocket" "-lsocket -lnsl" "-lnsl -lsocket" "-lnetwork"; do
    if $HOST_CXX $PROBE_CPPFLAGS $PROBE_CXXFLAGS \
        -o "$binary" "$source" \
        $PROBE_LDFLAGS $PROBE_LDLIBS $libs >/dev/null 2>&1; then
        printf '%s\n' "$libs"
        exit 0
    fi
done

# Leave the final link to produce the platform-specific diagnostic if no probe
# candidate worked.
printf '\n'
