#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 APPLY_PATCH_BINARY" >&2
    exit 2
fi

apply_patch_binary=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
test_root=$(mktemp -d "${TMPDIR:-/tmp}/remote-exec-apply-patch-test.XXXXXX")
help_file="$test_root/help.txt"

cleanup() {
    rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM

printf '%s\n' 'Custom help text' > "$help_file"
"$apply_patch_binary" --help | grep -F 'Apply a Codex-style patch read from standard input.'
"$apply_patch_binary" --help --help-file "$help_file" | cmp - "$help_file"

printf '%s' '*** Begin Patch
*** Add File: created.txt
+hello from cpp
*** End Patch
' | (
    cd "$test_root"
    "$apply_patch_binary" >/dev/null
)

test "$(cat "$test_root/created.txt")" = 'hello from cpp'

printf '%s' '*** Begin Patch
*** Add File: ignored-help-file.txt
+help-file is ignored without --help
*** End Patch
' | (
    cd "$test_root"
    "$apply_patch_binary" --help-file "$test_root/missing-help.txt" >/dev/null
)

test "$(cat "$test_root/ignored-help-file.txt")" = 'help-file is ignored without --help'

set +e
printf '%s' '*** Begin Patch
*** Update File: missing.txt
@@
-old
+new
*** End Patch
' | (
    cd "$test_root"
    "$apply_patch_binary" > "$test_root/missing.stdout" 2> "$test_root/missing.stderr"
)
missing_status=$?
set -e
test "$missing_status" -eq 1
test ! -s "$test_root/missing.stdout"
grep -F 'failed to update `missing.txt`: unable to read ./missing.txt' "$test_root/missing.stderr"

mkdir "$test_root/locked"
chmod 555 "$test_root/locked"
set +e
printf '%s' '*** Begin Patch
*** Add File: created-before-error.txt
+created
*** Add File: locked/blocked-after-error.txt
+blocked
*** End Patch
' | (
    cd "$test_root"
    "$apply_patch_binary" > "$test_root/partial.stdout" 2> "$test_root/partial.stderr"
)
partial_status=$?
set -e
chmod 755 "$test_root/locked"
test "$partial_status" -eq 1
grep -F 'Partial success. Updated the following files:' "$test_root/partial.stdout"
grep -F 'A created-before-error.txt' "$test_root/partial.stdout"
grep -F 'failed to add `locked/blocked-after-error.txt`: unable to write ' "$test_root/partial.stderr"
test "$(cat "$test_root/created-before-error.txt")" = 'created'
