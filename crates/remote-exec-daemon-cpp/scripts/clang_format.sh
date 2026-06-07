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

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_root=$(CDPATH= cd -- "$script_dir/.." && pwd)

find "$project_root/include" "$project_root/src" "$project_root/tests" \
    -type f \
    \( -name '*.cpp' -o -name '*.h' -o -name '*.hpp' \) \
    -print0 |
    xargs -0 "$CLANG_FORMAT" $clang_format_args
