#include "test_file_routes_shared.h"

#include <cstdint>
#include <string>

#include "http/http_helpers.h"
#include "rpc/server_contract.h"
#include "rpc/server_routes.h"
#include "test_assert.h"
#include "test_text_file.h"

namespace fs = test_fs;

namespace {

// Little-endian UTF-16 with a byte-order mark, matching the Rust daemon's
// utf16le_bom_bytes test helper.
std::string utf16le_bom_bytes(const std::string& text) {
    std::string bytes;
    bytes.push_back(static_cast<char>(0xFF));
    bytes.push_back(static_cast<char>(0xFE));
    for (std::size_t i = 0; i < text.size(); ++i) {
        bytes.push_back(text[i]);
        bytes.push_back('\0');
    }
    return bytes;
}

Json file_read_body(
    const std::string& path,
    std::uint64_t limit,
    std::uint64_t max_bytes,
    bool include_offset,
    std::uint64_t offset
) {
    Json body{
        {"path", path},
        {"limit", limit},
        {"max_bytes", max_bytes},
    };
    if (include_offset) {
        body["offset"] = offset;
    }
    return body;
}

HttpResponse post_file(TestRouteHarness& harness, const std::string& route, const Json& body) {
    return route_request(harness, make_json_http_request(route, body));
}

std::string error_code(const HttpResponse& response) {
    return Json::parse(response.body).at("code").get<std::string>();
}

std::string error_message(const HttpResponse& response) {
    return Json::parse(response.body).at("message").get<std::string>();
}

void assert_bad_request_file_tool(TestRouteHarness& harness, const fs::path& root) {
    // limit and max_bytes are required and must be positive, matching the Rust
    // daemon's validation order.
    const HttpResponse zero_limit =
        post_file(harness, "/v1/file/read", file_read_body("sample.txt", 0U, 1024U, false, 0U));
    TEST_ASSERT(zero_limit.status == 400);
    TEST_ASSERT(error_code(zero_limit) == "bad_request");
    TEST_ASSERT(error_message(zero_limit).find("read.limit") != std::string::npos);

    // A missing required field is a bad request, not a crash.
    const HttpResponse missing_limit =
        post_file(harness, "/v1/file/read", Json{{"path", "sample.txt"}, {"max_bytes", 1024}});
    TEST_ASSERT(missing_limit.status == 400);
    TEST_ASSERT(error_code(missing_limit) == "bad_request");

    const HttpResponse zero_write_bytes = post_file(
        harness,
        "/v1/file/write",
        Json{{"path", "sample.txt"}, {"content", "x"}, {"max_bytes", 0}}
    );
    TEST_ASSERT(zero_write_bytes.status == 400);
    TEST_ASSERT(error_message(zero_write_bytes).find("write.max_bytes") != std::string::npos);

    const HttpResponse empty_old_string = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "sample.txt"},
            {"old_string", ""},
            {"new_string", "x"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(empty_old_string.status == 400);
    TEST_ASSERT(error_message(empty_old_string).find("old_string") != std::string::npos);

    // 64-bit limits must survive the wire even on 32-bit long platforms.
    const fs::path large_path = root / "size-limit.txt";
    write_text_file(large_path, "0123456789\n");
    const HttpResponse too_large =
        post_file(harness, "/v1/file/read", file_read_body("size-limit.txt", 10U, 4U, false, 0U));
    TEST_ASSERT(too_large.status == 400);
    TEST_ASSERT(error_code(too_large) == "file_too_large");

    // A limit far beyond any 32-bit value must be accepted, not truncated.
    const HttpResponse wide_limit = post_file(
        harness,
        "/v1/file/read",
        file_read_body("size-limit.txt", 4294967296ULL, 4294967296ULL, false, 0U)
    );
    TEST_ASSERT(wide_limit.status == 200);
}

void assert_read_route(TestRouteHarness& harness, const fs::path& root) {
    write_text_file(root / "lines.txt", "one\ntwo\nthree\n");

    const HttpResponse windowed =
        post_file(harness, "/v1/file/read", file_read_body("lines.txt", 2U, 1024U, true, 0U));
    TEST_ASSERT(windowed.status == 200);
    const Json windowed_body = Json::parse(windowed.body);
    TEST_ASSERT(
        windowed_body.at("output").get<std::string>()
        == "1: one\n2: two\n\n(showing lines 1-2 of 3 lines)"
    );
    TEST_ASSERT(windowed_body.at("lines_returned").get<unsigned int>() == 2U);
    TEST_ASSERT(windowed_body.at("total_lines").get<unsigned int>() == 3U);
    TEST_ASSERT(!windowed_body.at("eof").get<bool>());

    const HttpResponse tail =
        post_file(harness, "/v1/file/read", file_read_body("lines.txt", 10U, 1024U, true, 2U));
    TEST_ASSERT(tail.status == 200);
    const Json tail_body = Json::parse(tail.body);
    TEST_ASSERT(
        tail_body.at("output").get<std::string>()
        == "2: two\n3: three\n\n(EOF reached, file has 3 lines)"
    );
    TEST_ASSERT(tail_body.at("eof").get<bool>());

    const fs::path empty_path = root / "empty.txt";
    write_text_file(empty_path, "");
    const HttpResponse empty =
        post_file(harness, "/v1/file/read", file_read_body("empty.txt", 10U, 1024U, false, 0U));
    TEST_ASSERT(empty.status == 200);
    const Json empty_body = Json::parse(empty.body);
    TEST_ASSERT(empty_body.at("output").get<std::string>() == "(file is empty)");
    TEST_ASSERT(empty_body.at("total_lines").get<unsigned int>() == 0U);
    TEST_ASSERT(empty_body.at("eof").get<bool>());

    const HttpResponse out_of_range =
        post_file(harness, "/v1/file/read", file_read_body("lines.txt", 3U, 1024U, true, 99U));
    TEST_ASSERT(out_of_range.status == 200);
    TEST_ASSERT(
        Json::parse(out_of_range.body).at("output").get<std::string>()
        == "(offset out of range, file only has 3 lines)"
    );

    const HttpResponse missing =
        post_file(harness, "/v1/file/read", file_read_body("absent.txt", 10U, 1024U, false, 0U));
    TEST_ASSERT(missing.status == 400);
    TEST_ASSERT(error_code(missing) == "file_missing");

    const fs::path directory_path = root / "a-directory";
    fs::create_directories(directory_path);
    const HttpResponse not_a_file =
        post_file(harness, "/v1/file/read", file_read_body("a-directory", 10U, 1024U, false, 0U));
    TEST_ASSERT(not_a_file.status == 400);
    TEST_ASSERT(error_code(not_a_file) == "file_not_file");

    // Invalid UTF-8 without autodetection stays a decode error.
    const fs::path invalid_path = root / "invalid-utf8.txt";
    fs::write_file_bytes(invalid_path, std::string("\xFF\xFE\xFD"));
    const HttpResponse invalid = post_file(
        harness,
        "/v1/file/read",
        file_read_body("invalid-utf8.txt", 10U, 1024U, false, 0U)
    );
    TEST_ASSERT(invalid.status == 400);
    TEST_ASSERT(error_code(invalid) == "file_decode_failed");
}

void assert_write_and_edit_routes(TestRouteHarness& harness, const fs::path& root) {
    const HttpResponse created = post_file(
        harness,
        "/v1/file/write",
        Json{{"path", "demo.txt"}, {"content", "red blue red\n"}, {"max_bytes", 1024}}
    );
    TEST_ASSERT(created.status == 200);
    const Json created_body = Json::parse(created.body);
    TEST_ASSERT(created_body.at("created").get<bool>());
    TEST_ASSERT(created_body.at("line_count").get<unsigned int>() == 1U);

    // Overwriting an existing file reports created = false.
    const HttpResponse updated = post_file(
        harness,
        "/v1/file/write",
        Json{{"path", "demo.txt"}, {"content", "green\n"}, {"max_bytes", 1024}}
    );
    TEST_ASSERT(updated.status == 200);
    TEST_ASSERT(!Json::parse(updated.body).at("created").get<bool>());

    const HttpResponse single_match = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "demo.txt"},
            {"old_string", "green"},
            {"new_string", "blue"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(single_match.status == 200);
    TEST_ASSERT(Json::parse(single_match.body).at("replacements").get<unsigned int>() == 1U);
    TEST_ASSERT(fs::read_file_bytes(root / "demo.txt") == "blue\n");

    // A search string that is absent is reported separately from an
    // ambiguous one.
    const HttpResponse not_found = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "demo.txt"},
            {"old_string", "absent"},
            {"new_string", "blue"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(not_found.status == 400);
    TEST_ASSERT(error_code(not_found) == "file_old_string_not_found");

    const fs::path repeated_path = root / "repeated.txt";
    write_text_file(repeated_path, "red blue red\n");
    const HttpResponse needs_replace_all = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "repeated.txt"},
            {"old_string", "red"},
            {"new_string", "green"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(needs_replace_all.status == 400);
    TEST_ASSERT(error_code(needs_replace_all) == "file_old_string_ambiguous");

    const HttpResponse replaced_all = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "repeated.txt"},
            {"old_string", "red"},
            {"new_string", "green"},
            {"replace_all", true},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(replaced_all.status == 200);
    const Json replaced_body = Json::parse(replaced_all.body);
    TEST_ASSERT(replaced_body.at("replacements").get<unsigned int>() == 2U);
    TEST_ASSERT(fs::read_file_bytes(repeated_path) == "green blue green\n");

    const HttpResponse single_replacement = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "repeated.txt"},
            {"old_string", "blue"},
            {"new_string", "cyan"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(single_replacement.status == 200);
    TEST_ASSERT(Json::parse(single_replacement.body).at("replacements").get<unsigned int>() == 1U);
    TEST_ASSERT(fs::read_file_bytes(repeated_path) == "green cyan green\n");

    // Editing a missing file reports file_missing rather than creating it.
    const HttpResponse edit_missing = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "never-there.txt"},
            {"old_string", "a"},
            {"new_string", "b"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(edit_missing.status == 400);
    TEST_ASSERT(error_code(edit_missing) == "file_missing");
}

void assert_encoding_autodetect_routes(TestRouteHarness& harness, const fs::path& root) {
    // Autodetection is off by default, so a UTF-16 file is rejected until the
    // daemon config enables it.
    const fs::path utf16_path = root / "utf16.txt";
    fs::write_file_bytes(utf16_path, utf16le_bom_bytes("hello\nworld\n"));

    const HttpResponse disabled =
        post_file(harness, "/v1/file/read", file_read_body("utf16.txt", 10U, 1024U, false, 0U));
    TEST_ASSERT(disabled.status == 400);
    TEST_ASSERT(error_code(disabled) == "file_decode_failed");

    harness.state.config.experimental_apply_patch_target_encoding_autodetect = true;
    harness.refresh_context();

    const HttpResponse read =
        post_file(harness, "/v1/file/read", file_read_body("utf16.txt", 10U, 1024U, false, 0U));
    TEST_ASSERT(read.status == 200);
    TEST_ASSERT(
        Json::parse(read.body).at("output").get<std::string>()
        == "1: hello\n2: world\n\n(EOF reached, file has 2 lines)"
    );

    // An edit re-encodes into the detected encoding, preserving the BOM.
    const HttpResponse edit = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "utf16.txt"},
            {"old_string", "world"},
            {"new_string", "there"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(edit.status == 200);
    TEST_ASSERT(fs::read_file_bytes(utf16_path) == utf16le_bom_bytes("hello\nthere\n"));

    // A write to an existing detected file keeps its encoding too.
    const HttpResponse write = post_file(
        harness,
        "/v1/file/write",
        Json{{"path", "utf16.txt"}, {"content", "done\n"}, {"max_bytes", 1024}}
    );
    TEST_ASSERT(write.status == 200);
    TEST_ASSERT(fs::read_file_bytes(utf16_path) == utf16le_bom_bytes("done\n"));

    // Binary content is refused rather than silently decoded.
    const fs::path binary_path = root / "binary.bin";
    fs::write_file_bytes(binary_path, std::string("\xFF\x00\xFE\x01\xFD\x02"));
    const HttpResponse binary =
        post_file(harness, "/v1/file/read", file_read_body("binary.bin", 10U, 1024U, false, 0U));
    TEST_ASSERT(binary.status == 400);
    TEST_ASSERT(error_code(binary) == "file_decode_failed");

    harness.state.config.experimental_apply_patch_target_encoding_autodetect = false;
    harness.refresh_context();
}

void assert_file_tool_sandbox(TestRouteHarness& harness, const fs::path& root) {
    const fs::path readable = root / "file-sandbox-readable";
    const fs::path writable = root / "file-sandbox-writable";
    fs::create_directories(readable);
    fs::create_directories(writable);

    harness.state.config.sandbox_configured = true;
    harness.state.config.sandbox.read.allow.push_back(readable.string());
    harness.state.config.sandbox.write.allow.push_back(writable.string());
    enable_test_daemon_sandbox(harness.state);
    harness.refresh_context();

    write_text_file(root / "blocked-read.txt", "blocked\n");
    write_text_file(root / "blocked-write.txt", "blocked\n");

    const HttpResponse read = post_file(
        harness,
        "/v1/file/read",
        file_read_body("blocked-read.txt", 10U, 1024U, false, 0U)
    );
    TEST_ASSERT(read.status == 400);
    TEST_ASSERT(error_code(read) == "sandbox_denied");

    const HttpResponse write = post_file(
        harness,
        "/v1/file/write",
        Json{{"path", "blocked-write.txt"}, {"content", "x\n"}, {"max_bytes", 1024}}
    );
    TEST_ASSERT(write.status == 400);
    TEST_ASSERT(error_code(write) == "sandbox_denied");

    const HttpResponse edit = post_file(
        harness,
        "/v1/file/edit",
        Json{
            {"path", "blocked-write.txt"},
            {"old_string", "blocked"},
            {"new_string", "updated"},
            {"replace_all", false},
            {"max_bytes", 1024},
        }
    );
    TEST_ASSERT(edit.status == 400);
    TEST_ASSERT(error_code(edit) == "sandbox_denied");

    // A read-only path stays writable-denied even though it is readable.
    const fs::path readable_file = readable / "readable.txt";
    write_text_file(readable_file, "readable\n");
    const HttpResponse readable_read = post_file(
        harness,
        "/v1/file/read",
        file_read_body(readable_file.string(), 10U, 1024U, false, 0U)
    );
    TEST_ASSERT(readable_read.status == 200);
    const HttpResponse readable_write = post_file(
        harness,
        "/v1/file/write",
        Json{{"path", readable_file.string()}, {"content", "x\n"}, {"max_bytes", 1024}}
    );
    TEST_ASSERT(readable_write.status == 400);
    TEST_ASSERT(error_code(readable_write) == "sandbox_denied");

    harness.state.config.sandbox_configured = false;
    harness.state.config.sandbox.read.allow.clear();
    harness.state.config.sandbox.write.allow.clear();
    enable_test_daemon_sandbox(harness.state);
    harness.refresh_context();
}

} // namespace

void run_file_tool_route_tests(TestRouteHarness& harness, const fs::path& root) {
    // The capability must be advertised or the broker will refuse to route
    // remote_read, remote_write, and remote_edit to this daemon at all.
    HttpRequest target_info;
    target_info.method = "POST";
    target_info.path = "/v1/target-info";
    const HttpResponse info_response = route_request(harness, target_info);
    TEST_ASSERT(info_response.status == 200);
    TEST_ASSERT(
        Json::parse(info_response.body).at("file_tool_protocol_version").get<unsigned int>()
        == server_contract::FILE_TOOL_PROTOCOL_VERSION
    );

    assert_read_route(harness, root);
    assert_write_and_edit_routes(harness, root);
    assert_encoding_autodetect_routes(harness, root);
    assert_bad_request_file_tool(harness, root);
    assert_file_tool_sandbox(harness, root);
}
