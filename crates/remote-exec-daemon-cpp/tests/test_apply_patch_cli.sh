#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 APPLY_PATCH_BINARY" >&2
    exit 2
fi

apply_patch_binary=$1
test_root=$(mktemp -d "${TMPDIR:-/tmp}/remote-exec-apply-patch-test.XXXXXX")
help_file="$test_root/help.txt"

cleanup() {
    rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM

printf '%s\n' 'Custom help text' > "$help_file"
"$apply_patch_binary" --help | grep -F 'Apply a Codex-style patch read from standard input.'
"$apply_patch_binary" --help-file "$help_file" | cmp - "$help_file"

printf '%s' '*** Begin Patch
*** Add File: created.txt
+hello from cpp
*** End Patch
' | (
    cd "$test_root"
    "$apply_patch_binary" >/dev/null
)

test "$(cat "$test_root/created.txt")" = 'hello from cpp'
