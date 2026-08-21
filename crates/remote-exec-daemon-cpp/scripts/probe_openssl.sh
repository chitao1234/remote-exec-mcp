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
#define BORINGSSL_NO_CXX 1
#include <openssl/opensslv.h>
#include <openssl/ssl.h>

#if defined(LIBRESSL_VERSION_NUMBER) && LIBRESSL_VERSION_NUMBER < 0x2070100fL
#error LibreSSL 2.7.1 or newer is required
#endif

#if !defined(OPENSSL_IS_BORINGSSL) && !defined(LIBRESSL_VERSION_NUMBER) \
    && OPENSSL_VERSION_NUMBER < 0x10002000L
#error OpenSSL 1.0.2 or newer is required
#endif

#if !defined(OPENSSL_IS_BORINGSSL) && defined(OPENSSL_NO_EC)
#error OpenSSL EC support is required for TLS 1.2 interoperability
#endif

int main() {
#if defined(OPENSSL_IS_BORINGSSL) || defined(LIBRESSL_VERSION_NUMBER) \
    || OPENSSL_VERSION_NUMBER >= 0x10100000L
    SSL_CTX* context = SSL_CTX_new(TLS_server_method());
    if (context != 0 && SSL_CTX_set_min_proto_version(context, TLS1_2_VERSION) != 1) {
        SSL_CTX_free(context);
        return 1;
    }
#else
    SSL_CTX* context = SSL_CTX_new(SSLv23_method());
#endif
    SSL_CTX_free(context);
    return context == 0;
}
EOF

$HOST_CXX $PROBE_CPPFLAGS $PROBE_CXXFLAGS \
    -o "$binary" "$source" \
    $PROBE_LDFLAGS $PROBE_LDLIBS >/dev/null 2>&1
