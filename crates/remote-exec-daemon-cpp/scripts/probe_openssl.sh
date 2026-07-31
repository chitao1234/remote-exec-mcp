#!/bin/sh
set -eu

: "${HOST_CXX:=c++}"
: "${PROBE_CPPFLAGS:=}"
: "${PROBE_CXXFLAGS:=-std=c++11 -O2 -Wall -Wextra -pthread}"
: "${PROBE_LDFLAGS:=}"
: "${PROBE_LDLIBS:=-lssl -lcrypto}"

tmp_dir=${TMPDIR:-/tmp}/remote-exec-cpp-openssl.$$
source=$tmp_dir/probe.cpp
binary=$tmp_dir/probe
mkdir -p "$tmp_dir"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

cat > "$source" <<'EOF'
#include <openssl/opensslv.h>
#include <openssl/ssl.h>

#if OPENSSL_VERSION_NUMBER < 0x10002000L
#error OpenSSL 1.0.2 or newer is required
#endif

#ifdef OPENSSL_NO_EC
#error OpenSSL EC support is required for TLS 1.2 interoperability
#endif

int main() {
    SSL_CTX* context = SSL_CTX_new(SSLv23_method());
    SSL_CTX_free(context);
    return context == 0;
}
EOF

$HOST_CXX $PROBE_CPPFLAGS $PROBE_CXXFLAGS \
    -o "$binary" "$source" \
    $PROBE_LDFLAGS $PROBE_LDLIBS >/dev/null 2>&1
