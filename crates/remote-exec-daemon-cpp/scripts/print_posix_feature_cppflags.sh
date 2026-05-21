#!/bin/sh
set -eu

: "${HOST_CXX:=c++}"
: "${PROBE_CPPFLAGS:=}"
: "${PROBE_CXXFLAGS:=-std=c++11 -O2 -Wall -Wextra -pthread}"

tmp_dir=${TMPDIR:-/tmp}/remote-exec-cpp-feature-cppflags.$$
source=$tmp_dir/probe.cpp
object=$tmp_dir/probe.o
mkdir -p "$tmp_dir"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

cat > "$source" <<'EOF'
#include <netdb.h>

int main() {
    char host[NI_MAXHOST];
    char service[NI_MAXSERV];
    host[0] = '\0';
    service[0] = '\0';
    return host[0] + service[0];
}
EOF

for flags in "" "-D_DEFAULT_SOURCE"; do
    if $HOST_CXX $PROBE_CPPFLAGS $flags $PROBE_CXXFLAGS -c -o "$object" "$source" >/dev/null 2>&1; then
        printf '%s\n' "$flags"
        exit 0
    fi
done

# Leave feature visibility failures to the real source compile; this probe only
# adds known portability flags when they are required by the platform headers.
printf '\n'
