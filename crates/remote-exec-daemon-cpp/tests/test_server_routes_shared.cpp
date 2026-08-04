#include "test_assert.h"
#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <iterator>
#include <string>

#include "codec/base64_codec.h"
#include "core/config.h"
#include "http/http_helpers.h"
#include "platform/platform.h"
#include "policy/path_policy.h"
#include "port_forward/port_forward_endpoint.h"
#include "port_forward/port_tunnel.h"
#include "rpc/server_contract.h"
#include "rpc/server_routes.h"
#include "rpc/transfer_wire.h"
#include "test_contract_fixtures.h"
#include "test_filesystem.h"
#include "test_server_routes_shared.h"
#include "test_tar_helpers.h"
#include "test_text_file.h"
#include "transfer/transfer_ops.h"

namespace fs = test_fs;

namespace tar = test_tar;

namespace {

#ifndef _WIN32
bool path_can_be_opened_for_read(const fs::path& path) {
    FILE* probe = std::fopen(path.string().c_str(), "rb");
    if (probe == NULL) {
        return false;
    }
    std::fclose(probe);
    return true;
}
#endif

} // namespace

static std::string decode_data_url_bytes(const std::string& image_url) {
    const std::size_t comma = image_url.find(',');
    TEST_ASSERT(comma != std::string::npos);
    return base64_decode_bytes(image_url.substr(comma + 1));
}

static HttpRequest transfer_export_request(const Json& body) {
    HttpRequest request = make_json_http_request("/v1/transfer/export", body);
    request.headers[server_contract::TRANSFER_STREAM_VERSION_HEADER] =
        server_contract::TRANSFER_STREAM_VERSION_VALUE;
    return request;
}

static HttpRequest transfer_import_request(
    const fs::path& destination,
    const std::string& archive,
    const char* source_type = "file",
    const char* overwrite = "replace"
) {
    HttpRequest request;
    request.method = "POST";
    request.path = "/v1/transfer/import";
    request.headers["content-type"] = server_contract::TRANSFER_STREAM_CONTENT_TYPE;
    request.headers[server_contract::TRANSFER_STREAM_VERSION_HEADER] =
        server_contract::TRANSFER_STREAM_VERSION_VALUE;
    request.headers[server_contract::TRANSFER_SOURCE_TYPE_HEADER] = source_type;
    request.headers[server_contract::TRANSFER_DESTINATION_PATH_HEADER] =
        tar::encoded_destination_path_header(destination);
    request.headers[server_contract::TRANSFER_OVERWRITE_HEADER] = overwrite;
    request.headers[server_contract::TRANSFER_CREATE_PARENT_HEADER] = "true";
    request.headers[server_contract::TRANSFER_SYMLINK_MODE_HEADER] = "preserve";
    request.headers[server_contract::TRANSFER_COMPRESSION_HEADER] = "none";
    request.body = tar::framed_transfer_body(archive);
    return request;
}

static void assert_bad_request_for_transfer_import(
    TestRouteHarness& harness,
    const HttpRequest& request,
    const fs::path& destination,
    const std::string& message_fragment
) {
    const HttpResponse response = route_request(harness, request);
    TEST_ASSERT(response.status == 400);
    const Json body = Json::parse(response.body);
    TEST_ASSERT(body.at("code").get<std::string>() == "bad_request");
    TEST_ASSERT(body.at("message").get<std::string>().find(message_fragment) != std::string::npos);
    TEST_ASSERT(!fs::exists(destination));
}

static void assert_target_info_and_basic_helpers(TestRouteHarness& harness) {
    HttpRequest info_request;
    info_request.method = "POST";
    info_request.path = "/v1/target-info";
    info_request.headers[request_id_header_name()] = "client-req-123";
    const HttpResponse info_response = route_request(harness, info_request);
    TEST_ASSERT(info_response.status == 200);
    TEST_ASSERT(info_response.headers.at(request_id_header_name()) == "client-req-123");
    const Json info = Json::parse(info_response.body);
    TEST_ASSERT(info.at("target").get<std::string>() == "cpp-test");
    TEST_ASSERT(
        info.at("supports_pty").get<bool>() == harness.state.metadata.capabilities.supports_pty
    );
    TEST_ASSERT(info.at("supports_image_read").get<bool>());
    TEST_ASSERT(!info.at("supports_transfer_compression").get<bool>());
    TEST_ASSERT(
        info.at("transfer_stream_protocol_version").get<unsigned int>()
        == server_contract::TRANSFER_STREAM_PROTOCOL_VERSION
    );
    TEST_ASSERT(info.at("supports_port_forward").get<bool>());
    TEST_ASSERT(
        info.at("port_forward_protocol_version").get<unsigned int>()
        == server_contract::PORT_TUNNEL_PROTOCOL_VERSION
    );

    HttpRequest generated_request;
    generated_request.method = "POST";
    generated_request.path = "/v1/health";
    const HttpResponse generated_response = route_request(harness, generated_request);
    TEST_ASSERT(generated_response.status == 200);
    TEST_ASSERT(generated_response.headers.at(request_id_header_name()).find("req_cpp_") == 0);

    TEST_ASSERT(normalize_port_forward_endpoint("8080") == "127.0.0.1:8080");
    TEST_ASSERT(
        base64_decode_bytes(base64_encode_bytes(std::string("hello\0world", 11))).size() == 11
    );
}

static void assert_shared_server_contract() {
    const Json& port_tunnel_contract = test_contract::port_tunnel_contract();
    const Json& transfer_headers = test_contract::transfer_headers_contract().at("headers");

    TEST_ASSERT(
        port_tunnel_contract.at("protocol_version_header").get<std::string>()
        == server_contract::PORT_TUNNEL_VERSION_HEADER
    );
    TEST_ASSERT(
        port_tunnel_contract.at("protocol_version_value").get<std::string>()
        == server_contract::PORT_TUNNEL_VERSION_VALUE
    );
    TEST_ASSERT(
        port_tunnel_contract.at("protocol_version_number").get<unsigned int>()
        == server_contract::PORT_TUNNEL_PROTOCOL_VERSION
    );
    TEST_ASSERT(
        port_tunnel_contract.at("upgrade_token").get<std::string>()
        == server_contract::PORT_TUNNEL_UPGRADE_TOKEN
    );

    TEST_ASSERT(
        transfer_headers.at("destination_path").get<std::string>()
        == server_contract::TRANSFER_DESTINATION_PATH_HEADER
    );
    TEST_ASSERT(
        transfer_headers.at("overwrite").get<std::string>()
        == server_contract::TRANSFER_OVERWRITE_HEADER
    );
    TEST_ASSERT(
        transfer_headers.at("create_parent").get<std::string>()
        == server_contract::TRANSFER_CREATE_PARENT_HEADER
    );
    TEST_ASSERT(
        transfer_headers.at("source_type").get<std::string>()
        == server_contract::TRANSFER_SOURCE_TYPE_HEADER
    );
    TEST_ASSERT(
        transfer_headers.at("compression").get<std::string>()
        == server_contract::TRANSFER_COMPRESSION_HEADER
    );
    TEST_ASSERT(
        transfer_headers.at("symlink_mode").get<std::string>()
        == server_contract::TRANSFER_SYMLINK_MODE_HEADER
    );
}

static void assert_transfer_export_errors(TestRouteHarness& harness, const fs::path& root) {
    const HttpResponse compression_response = route_request(
        harness,
        transfer_export_request(
            Json{{"path", (root / "missing.txt").string()}, {"compression", "zstd"}}
        )
    );
    TEST_ASSERT(compression_response.status == 400);
    TEST_ASSERT(
        Json::parse(compression_response.body).at("code").get<std::string>()
        == "transfer_compression_unsupported"
    );

    const HttpResponse missing_source_response = route_request(
        harness,
        transfer_export_request(Json{{"path", (root / "missing.txt").string()}})
    );
    TEST_ASSERT(missing_source_response.status == 400);
    TEST_ASSERT(
        Json::parse(missing_source_response.body).at("code").get<std::string>()
        == "transfer_source_missing"
    );
}

static void assert_image_routes(TestRouteHarness& harness, const fs::path& root) {
    const fs::path image_file = root / "tiny.png";
    fs::write_file_bytes(
        image_file,
        base64_decode_bytes("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/"
                            "x8AAwMCAO+aL9sAAAAASUVORK5CYII=")
    );
    const std::string original_image = fs::read_file_bytes(image_file);

    const HttpResponse image_response = route_request(
        harness,
        make_json_http_request(
            "/v1/image/read",
            Json{{"path", "tiny.png"}, {"workdir", root.string()}}
        )
    );
    TEST_ASSERT(image_response.status == 200);
    const Json image = Json::parse(image_response.body);
    TEST_ASSERT(image.at("detail").is_null());
    TEST_ASSERT(image.at("image_url").get<std::string>().find("data:image/png;base64,") == 0);
    TEST_ASSERT(decode_data_url_bytes(image.at("image_url").get<std::string>()) == original_image);

    const HttpResponse noop_detail_response = route_request(
        harness,
        make_json_http_request(
            "/v1/image/read",
            Json{{"path", "tiny.png"}, {"workdir", root.string()}, {"detail", "low"}}
        )
    );
    TEST_ASSERT(noop_detail_response.status == 200);
    const Json noop_detail = Json::parse(noop_detail_response.body);
    TEST_ASSERT(noop_detail.at("detail").get<std::string>() == "low");
    TEST_ASSERT(
        decode_data_url_bytes(noop_detail.at("image_url").get<std::string>()) == original_image
    );

    const HttpResponse missing_image_response = route_request(
        harness,
        make_json_http_request(
            "/v1/image/read",
            Json{{"path", "missing.png"}, {"workdir", root.string()}}
        )
    );
    TEST_ASSERT(missing_image_response.status == 400);
    const Json missing_image = Json::parse(missing_image_response.body);
    TEST_ASSERT(missing_image.at("code").get<std::string>() == "image_missing");

    const fs::path gif_file = root / "tiny.gif";
    fs::write_file_bytes(
        gif_file,
        base64_decode_bytes("R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==")
    );

    const HttpResponse gif_response = route_request(
        harness,
        make_json_http_request(
            "/v1/image/read",
            Json{{"path", "tiny.gif"}, {"workdir", root.string()}}
        )
    );
    TEST_ASSERT(gif_response.status == 400);
    const Json gif_error = Json::parse(gif_response.body);
    TEST_ASSERT(gif_error.at("code").get<std::string>() == "image_decode_failed");

#ifndef _WIN32
    const fs::path blocked_image_dir = root / "blocked-image";
    fs::create_directories(blocked_image_dir);
    fs::write_file_bytes(
        blocked_image_dir / "blocked.png",
        base64_decode_bytes("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4////"
                            "fwAJ+wP9KobjigAAAABJRU5ErkJggg==")
    );
    fs::permissions(blocked_image_dir, fs::perms::none, fs::perm_options::replace);
    const fs::path blocked_image_path = blocked_image_dir / "blocked.png";
    const bool platform_enforces_blocked_directory =
        !path_can_be_opened_for_read(blocked_image_path);
    HttpResponse blocked_image_response;
    // Haiku can run with root-like single-user filesystem access, so first
    // verify that chmod actually denies this read on the current target.
    if (platform_enforces_blocked_directory) {
        blocked_image_response = route_request(
            harness,
            make_json_http_request(
                "/v1/image/read",
                Json{{"path", "blocked-image/blocked.png"}, {"workdir", root.string()}}
            )
        );
    }
    fs::permissions(blocked_image_dir, fs::perms::owner_all, fs::perm_options::replace);
    if (platform_enforces_blocked_directory) {
        TEST_ASSERT(blocked_image_response.status == 500);
        const Json blocked_image_error = Json::parse(blocked_image_response.body);
        TEST_ASSERT(blocked_image_error.at("code").get<std::string>() == "internal_error");
    }
#endif
}

static void assert_patch_route_audit_fields(TestRouteHarness& harness, const fs::path& root) {
    const fs::path patch_file = root / "patch-audit.txt";
    const std::string patch_text = "*** Begin Patch\n"
                                   "*** Add File: patch-audit.txt\n"
                                   "+audit\n"
                                   "*** End Patch\n";

    const HttpResponse response = route_request(
        harness,
        make_json_http_request(
            "/v1/patch/apply",
            Json{{"workdir", root.string()}, {"patch", patch_text}}
        )
    );
    TEST_ASSERT(response.status == 200);
    const Json body = Json::parse(response.body);
    TEST_ASSERT(
        body.at("output").get<std::string>().find("A patch-audit.txt") != std::string::npos
    );
    TEST_ASSERT(
        body.at("daemon_instance_id").get<std::string>()
        == harness.state.metadata.daemon_instance_id
    );
    TEST_ASSERT(body.at("updated_paths").size() == 1);
    TEST_ASSERT(body.at("updated_paths")[0].get<std::string>() == "A patch-audit.txt");
    TEST_ASSERT(fs::read_file_bytes(patch_file) == "audit\n");
}

static void assert_transfer_path_info_routes(TestRouteHarness& harness, const fs::path& root) {
    const fs::path source_file = root / "transfer-source.txt";
    write_text_file(source_file, "route transfer payload");

    const HttpResponse source_info_response = route_request(
        harness,
        make_json_http_request("/v1/transfer/path-info", Json{{"path", source_file.string()}})
    );
    TEST_ASSERT(source_info_response.status == 200);
    const Json source_info = Json::parse(source_info_response.body);
    TEST_ASSERT(source_info.at("exists").get<bool>());
    TEST_ASSERT(!source_info.at("is_directory").get<bool>());

    const HttpResponse root_info_response = route_request(
        harness,
        make_json_http_request("/v1/transfer/path-info", Json{{"path", root.string()}})
    );
    TEST_ASSERT(root_info_response.status == 200);
    const Json root_info = Json::parse(root_info_response.body);
    TEST_ASSERT(root_info.at("exists").get<bool>());
    TEST_ASSERT(root_info.at("is_directory").get<bool>());

    const HttpResponse relative_info_response = route_request(
        harness,
        make_json_http_request("/v1/transfer/path-info", Json{{"path", "relative/path.txt"}})
    );
    TEST_ASSERT(relative_info_response.status == 400);
    const Json relative_info_error = Json::parse(relative_info_response.body);
    TEST_ASSERT(relative_info_error.at("code").get<std::string>() == "transfer_path_not_absolute");

#ifndef _WIN32
    const fs::path blocked_transfer_dir = root / "blocked-transfer";
    fs::create_directories(blocked_transfer_dir);
    write_text_file(blocked_transfer_dir / "inside.txt", "secret");
    fs::permissions(blocked_transfer_dir, fs::perms::none, fs::perm_options::replace);
    const fs::path blocked_transfer_file = blocked_transfer_dir / "inside.txt";
    const bool platform_enforces_blocked_transfer_directory =
        !path_can_be_opened_for_read(blocked_transfer_file);
    HttpResponse blocked_transfer_info_response;
    if (platform_enforces_blocked_transfer_directory) {
        blocked_transfer_info_response = route_request(
            harness,
            make_json_http_request(
                "/v1/transfer/path-info",
                Json{{"path", blocked_transfer_file.string()}}
            )
        );
    }
    fs::permissions(blocked_transfer_dir, fs::perms::owner_all, fs::perm_options::replace);
    if (platform_enforces_blocked_transfer_directory) {
        TEST_ASSERT(blocked_transfer_info_response.status == 500);
        const Json blocked_transfer_info_error = Json::parse(blocked_transfer_info_response.body);
        TEST_ASSERT(blocked_transfer_info_error.at("code").get<std::string>() == "internal_error");
    }
#endif
}

static std::string assert_transfer_export_and_exclude_routes(
    TestRouteHarness& harness,
    const fs::path& root
) {
    const fs::path source_file = root / "transfer-source.txt";
    write_text_file(source_file, "route transfer payload");

    const HttpResponse export_response =
        route_request(harness, transfer_export_request(Json{{"path", source_file.string()}}));
    TEST_ASSERT(export_response.status == 200);
    TEST_ASSERT(
        export_response.headers.at("Content-Type") == server_contract::TRANSFER_EXPORT_CONTENT_TYPE
    );
    TEST_ASSERT(
        export_response.headers.at(server_contract::TRANSFER_STREAM_VERSION_HEADER)
        == server_contract::TRANSFER_STREAM_VERSION_VALUE
    );
    TEST_ASSERT(export_response.headers.at(server_contract::TRANSFER_SOURCE_TYPE_HEADER) == "file");
    TEST_ASSERT(export_response.headers.at(server_contract::TRANSFER_COMPRESSION_HEADER) == "none");
    TEST_ASSERT(!export_response.body.empty());
    const std::string export_archive = tar::decode_framed_transfer_archive(export_response.body);

    const fs::path exclude_source = root / "transfer-exclude-source";
    fs::create_directories(exclude_source / ".git");
    fs::create_directories(exclude_source / "logs");
    write_text_file(exclude_source / "keep.txt", "keep");
    write_text_file(exclude_source / "top.log", "drop");
    write_text_file(exclude_source / ".git" / "config", "secret");
    write_text_file(exclude_source / "logs" / "readme.txt", "keep");
    write_text_file(exclude_source / "logs" / "app.log", "drop");
    Json exclude_patterns = Json::array();
    exclude_patterns.push_back("**/*.log");
    exclude_patterns.push_back(".git/**");
    const HttpResponse export_excluded_response = route_request(
        harness,
        transfer_export_request(
            Json{{"path", exclude_source.string()}, {"exclude", exclude_patterns}}
        )
    );
    TEST_ASSERT(export_excluded_response.status == 200);
    const ImportSummary excluded_import = import_path(
        tar::decode_framed_transfer_archive(export_excluded_response.body),
        TransferSourceType::Directory,
        (root / "transfer-exclude-dest").string(),
        TransferOverwrite::Replace,
        true
    );
    TEST_ASSERT(excluded_import.warnings.empty());
    TEST_ASSERT(fs::read_file_bytes(root / "transfer-exclude-dest" / "keep.txt") == "keep");
    TEST_ASSERT(
        fs::read_file_bytes(root / "transfer-exclude-dest" / "logs" / "readme.txt") == "keep"
    );
    TEST_ASSERT(!fs::exists(root / "transfer-exclude-dest" / "top.log"));
    TEST_ASSERT(!fs::exists(root / "transfer-exclude-dest" / ".git"));
    TEST_ASSERT(!fs::exists(root / "transfer-exclude-dest" / "logs" / "app.log"));

    Json malformed_exclude = Json::array();
    malformed_exclude.push_back("tmp/[abc");
    const HttpResponse invalid_exclude_response = route_request(
        harness,
        transfer_export_request(
            Json{{"path", exclude_source.string()}, {"exclude", malformed_exclude}}
        )
    );
    TEST_ASSERT(invalid_exclude_response.status == 400);
    const Json invalid_exclude = Json::parse(invalid_exclude_response.body);
    TEST_ASSERT(invalid_exclude.at("code").get<std::string>() == "transfer_failed");
    TEST_ASSERT(
        invalid_exclude.at("message").get<std::string>().find("invalid exclude pattern")
        != std::string::npos
    );

    return export_archive;
}

static void assert_transfer_import_success(
    TestRouteHarness& harness,
    const fs::path& root,
    const std::string& export_body
) {
    HttpRequest import_request = transfer_import_request(root / "transfer-dest.txt", export_body);

    const HttpResponse import_response = route_request(harness, import_request);
    TEST_ASSERT(import_response.status == 200);
    const Json imported = Json::parse(import_response.body);
    TEST_ASSERT(imported.at("source_type").get<std::string>() == "file");
    TEST_ASSERT(imported.at("files_copied").get<std::uint64_t>() == 1);
    TEST_ASSERT(imported.at("bytes_copied").get<std::uint64_t>() == 22);
    TEST_ASSERT(imported.at("replaced").get<bool>() == false);
    TEST_ASSERT(imported.at("warnings").empty());
    TEST_ASSERT(fs::read_file_bytes(root / "transfer-dest.txt") == "route transfer payload");
}

static void assert_transfer_import_optional_defaults(
    TestRouteHarness& harness,
    const fs::path& root,
    const std::string& export_body
) {
    HttpRequest optional_defaults_import =
        transfer_import_request(root / "transfer-defaults.txt", export_body);
    optional_defaults_import.headers.erase(server_contract::TRANSFER_SYMLINK_MODE_HEADER);
    optional_defaults_import.headers.erase(server_contract::TRANSFER_COMPRESSION_HEADER);
    const HttpResponse optional_defaults_response =
        route_request(harness, optional_defaults_import);
    TEST_ASSERT(optional_defaults_response.status == 200);
    TEST_ASSERT(fs::read_file_bytes(root / "transfer-defaults.txt") == "route transfer payload");
}

static void assert_transfer_import_header_validation(
    TestRouteHarness& harness,
    const fs::path& root,
    const std::string& export_body
) {
    HttpRequest missing_create_parent =
        transfer_import_request(root / "missing-create-parent.txt", export_body);
    missing_create_parent.headers.erase(server_contract::TRANSFER_CREATE_PARENT_HEADER);
    assert_bad_request_for_transfer_import(
        harness,
        missing_create_parent,
        root / "missing-create-parent.txt",
        server_contract::TRANSFER_CREATE_PARENT_HEADER
    );

    HttpRequest invalid_create_parent =
        transfer_import_request(root / "invalid-create-parent.txt", export_body);
    invalid_create_parent.headers[server_contract::TRANSFER_CREATE_PARENT_HEADER] = "yes";
    assert_bad_request_for_transfer_import(
        harness,
        invalid_create_parent,
        root / "invalid-create-parent.txt",
        server_contract::TRANSFER_CREATE_PARENT_HEADER
    );

    HttpRequest invalid_source_type =
        transfer_import_request(root / "invalid-source-type.txt", export_body);
    invalid_source_type.headers[server_contract::TRANSFER_SOURCE_TYPE_HEADER] = "folder";
    assert_bad_request_for_transfer_import(
        harness,
        invalid_source_type,
        root / "invalid-source-type.txt",
        server_contract::TRANSFER_SOURCE_TYPE_HEADER
    );

    HttpRequest invalid_overwrite =
        transfer_import_request(root / "invalid-overwrite.txt", export_body);
    invalid_overwrite.headers[server_contract::TRANSFER_OVERWRITE_HEADER] = "clobber";
    assert_bad_request_for_transfer_import(
        harness,
        invalid_overwrite,
        root / "invalid-overwrite.txt",
        server_contract::TRANSFER_OVERWRITE_HEADER
    );

    HttpRequest invalid_compression =
        transfer_import_request(root / "invalid-compression.txt", export_body);
    invalid_compression.headers[server_contract::TRANSFER_COMPRESSION_HEADER] = "gzip";
    assert_bad_request_for_transfer_import(
        harness,
        invalid_compression,
        root / "invalid-compression.txt",
        server_contract::TRANSFER_COMPRESSION_HEADER
    );

    HttpRequest invalid_symlink_mode =
        transfer_import_request(root / "invalid-symlink-mode.txt", export_body);
    invalid_symlink_mode.headers[server_contract::TRANSFER_SYMLINK_MODE_HEADER] = "copy";
    assert_bad_request_for_transfer_import(
        harness,
        invalid_symlink_mode,
        root / "invalid-symlink-mode.txt",
        server_contract::TRANSFER_SYMLINK_MODE_HEADER
    );
}

static void assert_transfer_import_rejects_file_merge_into_directory(
    TestRouteHarness& harness,
    const fs::path& root,
    const std::string& export_body
) {
    fs::create_directories(root / "merge-dir");
    HttpRequest merge_file_into_directory_request =
        transfer_import_request(root / "merge-dir", export_body, "file", "merge");
    const HttpResponse merge_file_into_directory_response =
        route_request(harness, merge_file_into_directory_request);
    TEST_ASSERT(merge_file_into_directory_response.status == 400);
    const Json merge_file_into_directory_error =
        Json::parse(merge_file_into_directory_response.body);
    TEST_ASSERT(
        merge_file_into_directory_error.at("code").get<std::string>()
        == "transfer_destination_unsupported"
    );
}

static void assert_transfer_import_routes(
    TestRouteHarness& harness,
    const fs::path& root,
    const std::string& export_body
) {
    assert_transfer_import_success(harness, root, export_body);
    assert_transfer_import_optional_defaults(harness, root, export_body);
    assert_transfer_import_header_validation(harness, root, export_body);
    assert_transfer_import_rejects_file_merge_into_directory(harness, root, export_body);
}

static void assert_sandbox_denied(const HttpResponse& response) {
    TEST_ASSERT(response.status == 400);
    TEST_ASSERT(Json::parse(response.body).at("code").get<std::string>() == "sandbox_denied");
}

static void assert_sandbox_export_and_path_info_denied(
    TestRouteHarness& sandbox,
    const fs::path& outside
) {
    const HttpResponse sandbox_export_denied = route_request(
        sandbox,
        transfer_export_request(Json{{"path", (outside / "outside.txt").string()}})
    );
    assert_sandbox_denied(sandbox_export_denied);

    const HttpResponse sandbox_path_info_denied = route_request(
        sandbox,
        make_json_http_request(
            "/v1/transfer/path-info",
            Json{{"path", (outside / "dest.txt").string()}}
        )
    );
    assert_sandbox_denied(sandbox_path_info_denied);
}

static std::string assert_sandbox_export_allowed(
    TestRouteHarness& sandbox,
    const fs::path& read_allowed
) {
    const HttpResponse sandbox_export_allowed = route_request(
        sandbox,
        transfer_export_request(Json{{"path", (read_allowed / "source.txt").string()}})
    );
    TEST_ASSERT(sandbox_export_allowed.status == 200);
    return tar::decode_framed_transfer_archive(sandbox_export_allowed.body);
}

static void assert_sandbox_import_denied(
    TestRouteHarness& sandbox,
    const fs::path& outside,
    const std::string& export_body
) {
    HttpRequest sandbox_import_denied_request =
        transfer_import_request(outside / "dest.txt", export_body);
    const HttpResponse sandbox_import_denied =
        route_request(sandbox, sandbox_import_denied_request);
    assert_sandbox_denied(sandbox_import_denied);
}

#ifndef _WIN32
static void assert_sandbox_symlink_target_denied(
    TestRouteHarness& sandbox,
    const fs::path& write_allowed
) {
    std::string denied_symlink_archive;
    tar::append_tar_directory(denied_symlink_archive, ".");
    tar::append_tar_symlink(
        denied_symlink_archive,
        "allowed-link",
        "denied-link-target/secret.txt"
    );
    tar::finalize_tar(denied_symlink_archive);

    HttpRequest sandbox_symlink_target_denied_request =
        transfer_import_request(write_allowed, denied_symlink_archive, "directory", "merge");
    const HttpResponse sandbox_symlink_target_denied =
        route_request(sandbox, sandbox_symlink_target_denied_request);
    assert_sandbox_denied(sandbox_symlink_target_denied);
    TEST_ASSERT(!fs::exists(write_allowed / "allowed-link"));
}
#endif

static void assert_sandbox_patch_denied(
    TestRouteHarness& sandbox,
    const fs::path& outside,
    const fs::path& write_allowed
) {
    const std::string patch_denied_text = "*** Begin Patch\n"
                                          "*** Add File: "
                                          + (outside / "patched.txt").string()
                                          + "\n"
                                            "+denied\n"
                                            "*** End Patch\n";
    const HttpResponse sandbox_patch_denied = route_request(
        sandbox,
        make_json_http_request(
            "/v1/patch/apply",
            Json{{"workdir", write_allowed.string()}, {"patch", patch_denied_text}}
        )
    );
    assert_sandbox_denied(sandbox_patch_denied);
    TEST_ASSERT(!fs::exists(outside / "patched.txt"));
}

static void assert_sandbox_exec_denied(TestRouteHarness& sandbox, const fs::path& outside) {
    const HttpResponse sandbox_exec_denied = route_request(
        sandbox,
        make_json_http_request(
            "/v1/exec/start",
            Json{
                {"cmd", "printf denied"},
                {"workdir", outside.string()},
                {"login", false},
                {"tty", false},
                {"yield_time_ms", 250},
            }
        )
    );
    assert_sandbox_denied(sandbox_exec_denied);
}

static void assert_sandbox_routes(const fs::path& root) {
    const fs::path sandbox_root = root / "sandbox";
    const fs::path exec_allowed = sandbox_root / "exec";
    const fs::path read_allowed = sandbox_root / "read";
    const fs::path write_allowed = sandbox_root / "write";
    const fs::path outside = sandbox_root / "outside";
    const fs::path denied_link_target_root = write_allowed / "denied-link-target";
    fs::create_directories(exec_allowed);
    fs::create_directories(read_allowed);
    fs::create_directories(write_allowed);
    fs::create_directories(denied_link_target_root);
    fs::create_directories(outside);
    write_text_file(read_allowed / "source.txt", "sandbox source");
    write_text_file(outside / "outside.txt", "outside");

    TestRouteHarness sandbox(root);
    sandbox.state.config.sandbox_configured = true;
    sandbox.state.config.sandbox.exec_cwd.allow.push_back(exec_allowed.string());
    sandbox.state.config.sandbox.read.allow.push_back(read_allowed.string());
    sandbox.state.config.sandbox.write.allow.push_back(write_allowed.string());
    sandbox.state.config.sandbox.write.deny.push_back(denied_link_target_root.string());
    enable_test_daemon_sandbox(sandbox.state);
    sandbox.refresh_context();

    assert_sandbox_export_and_path_info_denied(sandbox, outside);
    const std::string sandbox_export_body = assert_sandbox_export_allowed(sandbox, read_allowed);
    assert_sandbox_import_denied(sandbox, outside, sandbox_export_body);

#ifndef _WIN32
    assert_sandbox_symlink_target_denied(sandbox, write_allowed);
#endif

    assert_sandbox_patch_denied(sandbox, outside, write_allowed);
    assert_sandbox_exec_denied(sandbox, outside);
}

void run_platform_neutral_server_route_tests(TestRouteHarness& harness, const fs::path& root) {
    assert_shared_server_contract();
    assert_target_info_and_basic_helpers(harness);
    assert_transfer_export_errors(harness, root);
    assert_image_routes(harness, root);
    assert_patch_route_audit_fields(harness, root);
    assert_transfer_path_info_routes(harness, root);
    const std::string export_body = assert_transfer_export_and_exclude_routes(harness, root);
    assert_transfer_import_routes(harness, root, export_body);
    assert_sandbox_routes(root);
}
