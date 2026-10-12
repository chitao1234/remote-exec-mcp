#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/remote-exec-cpp-build-version.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

mkdir -p "$test_root/repo/scripts" "$test_root/archive/scripts"
cp "$script_dir/../scripts/write_build_version_header.sh" "$test_root/repo/scripts/"
cp "$script_dir/../scripts/write_build_version_header.sh" "$test_root/archive/scripts/"
header=$test_root/generated/version.h

assert_metadata() {
    printf '#pragma once\n\n#define REMOTE_EXEC_CPP_BUILD_METADATA "%s"\n' "$1" > "$test_root/expected.h"
    cmp "$header" "$test_root/expected.h"
}

# Source archives must remain buildable without a Git repository.
sh "$test_root/archive/scripts/write_build_version_header.sh" "$header"
build_date=$(date -u +%Y%m%d)
assert_metadata "+$build_date"

if ! command -v git >/dev/null 2>&1; then
    exit 0
fi

git -C "$test_root/repo" init -q
git -C "$test_root/repo" -c user.name=Test -c user.email=test@example.invalid commit -q --allow-empty -m first
sh "$test_root/repo/scripts/write_build_version_header.sh" "$header"
revision=$(git -C "$test_root/repo" rev-parse --short=12 HEAD)
assert_metadata "+g$revision.$build_date"

# Repeated builds preserve the header timestamp when metadata is unchanged.
cp -p "$header" "$test_root/previous.h"
sh "$test_root/repo/scripts/write_build_version_header.sh" "$header"
cmp "$header" "$test_root/previous.h"
test ! "$header" -nt "$test_root/previous.h"

# A new HEAD must refresh an existing header, without cleaning the build.
git -C "$test_root/repo" -c user.name=Test -c user.email=test@example.invalid commit -q --allow-empty -m second
sh "$test_root/repo/scripts/write_build_version_header.sh" "$header"
revision=$(git -C "$test_root/repo" rev-parse --short=12 HEAD)
assert_metadata "+g$revision.$build_date"
if cmp -s "$header" "$test_root/previous.h"; then
    echo 'build metadata did not change with HEAD' >&2
    exit 1
fi

# Changing the UTC build date must also refresh an existing header. The mock
# checks that the generator requests UTC rather than the machine's local date.
mkdir "$test_root/bin"
cat > "$test_root/bin/date" <<'EOF'
#!/bin/sh
set -eu
test "$1" = -u
test "$2" = +%Y%m%d
printf '%s\n' 20000101
EOF
chmod +x "$test_root/bin/date"
PATH="$test_root/bin:$PATH" sh "$test_root/repo/scripts/write_build_version_header.sh" "$header"
assert_metadata "+g$revision.20000101"
