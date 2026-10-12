#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <output-header>" >&2
    exit 2
fi

output=$1
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
revision=$(git -C "$project_dir" rev-parse --short=12 HEAD 2>/dev/null || :)
build_date=$(date -u +%Y%m%d)
case "$revision" in
    ''|*[!0-9a-fA-F]*) metadata="+$build_date" ;;
    *) metadata="+g$revision.$build_date" ;;
esac

mkdir -p "$(dirname -- "$output")"
temporary=$output.tmp.$$
trap 'rm -f "$temporary"' EXIT HUP INT TERM
printf '#pragma once\n\n#define REMOTE_EXEC_CPP_BUILD_METADATA "%s"\n' "$metadata" > "$temporary"
# Preserve the timestamp when metadata is unchanged so incremental builds only
# recompile consumers when HEAD or the UTC build date actually changes.
if ! cmp -s "$temporary" "$output"; then
    mv -f "$temporary" "$output"
fi
