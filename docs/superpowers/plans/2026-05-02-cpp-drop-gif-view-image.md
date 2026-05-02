# C++ Drop GIF `view_image` Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Remove GIF input support from the C++ daemon `view_image` path while preserving the existing unsupported-format error surface.

**Architecture:** Keep the current C++ image path as a pure passthrough-only route, but shrink the accepted input set by deleting GIF magic-byte support from `image_mime_type`. Reuse the existing unsupported-format path so route behavior, error mapping, and broker expectations stay stable.

**Tech Stack:** C++17, custom HTTP route tests, `make` host tests, XP cross-build checks

---

## File Map

- Modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
  - Remove GIF magic-byte detection from the C++ `view_image` passthrough allowlist.
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
  - Add a targeted route regression that GIF input now fails with the existing `image_decode_failed` surface.
- Modify: `crates/remote-exec-daemon-cpp/README.md`
  - Remove GIF from the documented C++ `view_image` passthrough list.
- Modify: `README.md`
  - Remove GIF from the top-level C++ daemon capability summary.

### Task 1: Drop GIF From The C++ `view_image` Allowlist

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `README.md`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** `TDD`
Reason: The behavior seam is the route test binary already covering `view_image`, so a failing route regression cleanly proves the current GIF acceptance before the implementation change.

- [ ] **Step 1: Add a failing route regression for GIF input rejection**

```cpp
// crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
    const fs::path gif_file = root / "tiny.gif";
    write_binary_file(
        gif_file,
        base64_decode_bytes("R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==")
    );

    const HttpResponse gif_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "tiny.gif"}, {"workdir", root.string()}}
        )
    );
    assert(gif_response.status == 400);
    const Json gif_error = Json::parse(gif_response.body);
    assert(gif_error.at("code").get<std::string>() == "image_decode_failed");
```

- [ ] **Step 2: Run the focused verification for this step**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-server-routes
```

Expected:

- FAIL because the current C++ daemon still recognizes GIF and returns `200` with a passthrough response whose `image_url` starts with `data:image/gif;base64,`.

- [ ] **Step 3: Remove GIF support and update the docs**

```cpp
// crates/remote-exec-daemon-cpp/src/server_routes.cpp
std::string image_mime_type(const std::string& path, const std::string& bytes) {
    if (bytes.size() >= 8 && std::memcmp(bytes.data(), "\x89PNG\r\n\x1A\n", 8) == 0) {
        return "image/png";
    }
    if (bytes.size() >= 3 &&
        static_cast<unsigned char>(bytes[0]) == 0xFF &&
        static_cast<unsigned char>(bytes[1]) == 0xD8 &&
        static_cast<unsigned char>(bytes[2]) == 0xFF) {
        return "image/jpeg";
    }
    if (bytes.size() >= 12 &&
        std::memcmp(bytes.data(), "RIFF", 4) == 0 &&
        std::memcmp(bytes.data() + 8, "WEBP", 4) == 0) {
        return "image/webp";
    }
    throw std::runtime_error(
        "unable to process image at `" + path + "`: unsupported image format"
    );
}
```

```md
<!-- crates/remote-exec-daemon-cpp/README.md -->
`view_image` supports passthrough reads for PNG, JPEG, and WebP only. The
daemon does not resize or re-encode images, so omitted `detail` defaults to
`original`.
```

```md
<!-- README.md -->
- `remote-exec-daemon-cpp` is intentionally narrower than the main daemon: POSIX builds support `tty=true` when PTY allocation is available, Windows XP-compatible builds reject `tty=true`, `view_image` supports passthrough reads for PNG, JPEG, and WebP only and defaults omitted `detail` to `original`, TLS is unavailable, static path sandboxing is available for exec cwd, transfer read/write endpoints, patch write targets, and `view_image` reads, regular-file transfers, directory trees, and broker-built multi-source transfer bundles are supported, transfer import/export bodies stream through the daemon without staging a full tar archive in memory, and transfer staging always falls back to uncompressed payloads. POSIX C++ builds can preserve, follow, or skip source symlinks. Windows XP-compatible C++ builds skip symlink entries inside directory transfers and import archives when preservation is unavailable, while `follow` copies regular-file and directory targets when exposed by the platform. On POSIX it follows the Rust daemon's default shell policy and forces `LC_ALL=C.UTF-8` plus `LANG=C.UTF-8`; on Windows XP-compatible builds it supports `cmd.exe`. Hard links, sparse entries, malformed archive paths, and non-passthrough image formats remain unsupported there; special files are skipped during export.
```

- [ ] **Step 4: Run the post-change verification**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-server-routes
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Expected:

- `test-host-server-routes` PASS with GIF input now rejected as `image_decode_failed`
- `check-posix` PASS
- `check-windows-xp` PASS

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/src/server_routes.cpp \
        crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp \
        crates/remote-exec-daemon-cpp/README.md \
        README.md
git commit -m "fix: drop GIF image support from C++ daemon"
```
