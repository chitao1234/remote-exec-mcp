#!/bin/sh
set -eu

usage() {
    echo "usage: $0 check|format" >&2
    exit 2
}

if [ "$#" -ne 1 ]; then
    usage
fi

mode=$1
case "$mode" in
    check)
        clang_format_args="--dry-run --Werror"
        ;;
    format)
        clang_format_args="-i"
        ;;
    *)
        usage
        ;;
esac

: "${CLANG_FORMAT:=clang-format}"

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

git ls-files -z \
    'crates/remote-exec-daemon-cpp/include/**/*.cpp' \
    'crates/remote-exec-daemon-cpp/include/**/*.h' \
    'crates/remote-exec-daemon-cpp/include/**/*.hpp' \
    'crates/remote-exec-daemon-cpp/src/**/*.cpp' \
    'crates/remote-exec-daemon-cpp/src/**/*.h' \
    'crates/remote-exec-daemon-cpp/src/**/*.hpp' \
    'crates/remote-exec-daemon-cpp/tests/**/*.cpp' \
    'crates/remote-exec-daemon-cpp/tests/**/*.h' \
    'crates/remote-exec-daemon-cpp/tests/**/*.hpp' |
    xargs -0 "$CLANG_FORMAT" $clang_format_args
