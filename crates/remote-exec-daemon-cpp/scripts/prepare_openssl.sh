#!/bin/sh
set -eu

version=${OPENSSL_VERSION:-3.5.7}
sha256=${OPENSSL_SHA256:-a8c0d28a529ca480f9f36cf5792e2cd21984552a3c8e4aa11a24aa31aeac98e8}
url=${OPENSSL_URL:-https://github.com/openssl/openssl/releases/download/openssl-${version}/openssl-${version}.tar.gz}
deps_dir=${OPENSSL_DEPS_DIR:-build/deps}
source_cache_dir=${OPENSSL_SOURCE_CACHE_DIR:-}
if [ -n "$source_cache_dir" ]; then
    default_archive=${source_cache_dir}/openssl-${version}.tar.gz
else
    default_archive=${deps_dir}/downloads/openssl-${version}.tar.gz
fi
archive=${OPENSSL_ARCHIVE:-$default_archive}
source_dir=${OPENSSL_SOURCE_DIR:-${deps_dir}/src/openssl-${version}}
install_dir=${OPENSSL_INSTALL_DIR:-${deps_dir}/openssl-${version}}
configure_target=${OPENSSL_CONFIGURE_TARGET:-}
configure_options=${OPENSSL_CONFIGURE_OPTIONS:-}
if [ -n "${OPENSSL_BASE_CONFIGURE_OPTIONS:-}" ]; then
    base_configure_options=$OPENSSL_BASE_CONFIGURE_OPTIONS
else
    case "$version" in
        1.1.*) base_configure_options="no-shared no-tests" ;;
        *) base_configure_options="no-shared no-module no-tests" ;;
    esac
fi
jobs=${OPENSSL_JOBS:-1}
make_command=${OPENSSL_BUILD_MAKE:-make}
build_mode=${OPENSSL_BUILD_MODE:-default}

case "$install_dir" in
    /*) ;;
    *) install_dir=$(pwd)/$install_dir ;;
esac

mkdir -p "$(dirname "$archive")" "$deps_dir/src"

if [ ! -f "$archive" ]; then
    if command -v curl >/dev/null 2>&1; then
        curl -L --fail --show-error --output "$archive.tmp" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "$archive.tmp" "$url"
    else
        echo "prepare-openssl requires curl or wget, or OPENSSL_ARCHIVE" >&2
        exit 1
    fi
    mv "$archive.tmp" "$archive"
fi

actual_sha256=
if command -v sha256sum >/dev/null 2>&1; then
    actual_sha256=$(sha256sum "$archive" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    actual_sha256=$(shasum -a 256 "$archive" | awk '{print $1}')
elif command -v openssl >/dev/null 2>&1; then
    actual_sha256=$(openssl dgst -sha256 "$archive" | awk '{print $NF}')
else
    echo "prepare-openssl requires sha256sum, shasum, or openssl for verification" >&2
    exit 1
fi

if [ "$actual_sha256" != "$sha256" ]; then
    echo "OpenSSL archive checksum mismatch" >&2
    echo "expected: $sha256" >&2
    echo "actual:   $actual_sha256" >&2
    exit 1
fi

if [ ! -f "$source_dir/Configure" ]; then
    rm -rf "$source_dir.tmp"
    mkdir -p "$source_dir.tmp"
    tar -xzf "$archive" -C "$source_dir.tmp" --strip-components=1
    rm -rf "$source_dir"
    mv "$source_dir.tmp" "$source_dir"
fi

if [ ! -f "$install_dir/lib/libssl.a" ] && [ ! -f "$install_dir/lib/libssl.lib" ]; then
    cd "$source_dir"
    if [ -n "$configure_target" ]; then
        perl ./Configure "$configure_target" \
            $base_configure_options \
            $configure_options \
            --prefix="$install_dir" --openssldir="$install_dir/ssl" --libdir=lib
    else
        ./config $base_configure_options \
            $configure_options \
            --prefix="$install_dir" --openssldir="$install_dir/ssl" --libdir=lib
    fi
    if [ "$build_mode" = "static-libraries-only" ]; then
        case "$version" in
            1.0.*)
                "$make_command" -j "$jobs" build_crypto build_ssl
                ;;
            *)
                "$make_command" -j "$jobs" build_libs
                ;;
        esac
        mkdir -p "$install_dir/include" "$install_dir/lib"
        rm -rf "$install_dir/include/openssl"
        cp -R -L include/openssl "$install_dir/include/"
        cp libssl.a libcrypto.a "$install_dir/lib/"
    elif [ "$build_mode" = "default" ]; then
        "$make_command" -j "$jobs"
        "$make_command" install_sw
    else
        echo "unsupported OPENSSL_BUILD_MODE '$build_mode'" >&2
        exit 1
    fi
fi

printf '%s\n' "$install_dir"
