#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <fstream>
#include <iterator>
#include <string>

#include "base64_codec.h"
#include "config.h"
#include "filesystem_sandbox.h"
#include "http_helpers.h"
#include "path_policy.h"
#include "platform.h"
#include "port_forward_endpoint.h"
#include "port_tunnel.h"
#include "process_session.h"
#include "server_routes.h"
#include "test_filesystem.h"
#include "test_server_routes_shared.h"
#include "transfer_ops.h"

namespace fs = test_fs;

fs::path make_server_routes_test_root(const std::string& directory_name) {
    const fs::path root = fs::temp_directory_path() / directory_name;
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

static DaemonConfig make_config(const fs::path& root) {
    DaemonConfig config;
    config.target = "cpp-test";
    config.listen_host = "127.0.0.1";
    config.listen_port = 0;
    config.default_workdir = root.string();
    config.default_shell.clear();
    config.allow_login_shell = true;
    config.http_auth_bearer_token.clear();
    config.max_request_header_bytes = 65536;
    config.max_request_body_bytes = 536870912;
    config.transfer_limits = default_transfer_limit_config();
    config.max_open_sessions = 64;
    config.port_forward_limits = default_port_forward_limit_config();
    config.yield_time = default_yield_time_config();
    return config;
}

void initialize_server_routes_state(AppState& state, const fs::path& root) {
    state.config = make_config(root);
    state.daemon_instance_id = "test-instance";
    state.hostname = "test-host";
    state.default_shell = platform::resolve_default_shell("");
    state.port_tunnel_service =
        create_port_tunnel_service(state.config.port_forward_limits);
}

static void enable_sandbox(AppState& state) {
    state.sandbox_enabled = state.config.sandbox_configured;
    if (state.sandbox_enabled) {
        state.sandbox = compile_filesystem_sandbox(host_path_policy(), state.config.sandbox);
    }
}

static HttpRequest json_request(const std::string& path, const Json& body) {
    HttpRequest request;
    request.method = "POST";
    request.path = path;
    request.headers["content-type"] = "application/json";
    request.body = body.dump();
    return request;
}

static void write_text_file(const fs::path& path, const std::string& value) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    output << value;
}

#ifndef _WIN32
static std::string octal_field(std::size_t width, std::uint64_t value) {
    char buffer[64];
    std::snprintf(
        buffer,
        sizeof(buffer),
        "%0*llo",
        static_cast<int>(width - 1),
        static_cast<unsigned long long>(value)
    );
    std::string field(width, '\0');
    const std::string digits(buffer);
    const std::size_t start = width - 1 - std::min(width - 1, digits.size());
    field.replace(
        start,
        std::min(width - 1, digits.size()),
        digits.substr(digits.size() - std::min(width - 1, digits.size()))
    );
    field[width - 1] = ' ';
    return field;
}

static void set_bytes(
    std::string* header,
    std::size_t offset,
    std::size_t width,
    const std::string& value
) {
    header->replace(offset, std::min(width, value.size()), value.substr(0, width));
}

static void write_checksum(std::string* header) {
    std::fill(header->begin() + 148, header->begin() + 156, ' ');
    unsigned int checksum = 0;
    for (std::string::const_iterator it = header->begin(); it != header->end(); ++it) {
        checksum += static_cast<unsigned char>(*it);
    }
    const std::string field = octal_field(8, checksum);
    header->replace(148, 8, field);
}

static void append_tar_directory(std::string* archive, const std::string& path) {
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, path);
    header.replace(100, 8, octal_field(8, 0755));
    header.replace(108, 8, octal_field(8, 0));
    header.replace(116, 8, octal_field(8, 0));
    header.replace(124, 12, octal_field(12, 0));
    header.replace(136, 12, octal_field(12, 0));
    header[156] = '5';
    set_bytes(&header, 257, 6, "ustar ");
    set_bytes(&header, 263, 2, " \0");
    write_checksum(&header);
    archive->append(header);
}

static void append_tar_symlink(
    std::string* archive,
    const std::string& path,
    const std::string& target
) {
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, path);
    header.replace(100, 8, octal_field(8, 0777));
    header.replace(108, 8, octal_field(8, 0));
    header.replace(116, 8, octal_field(8, 0));
    header.replace(124, 12, octal_field(12, 0));
    header.replace(136, 12, octal_field(12, 0));
    header[156] = '2';
    set_bytes(&header, 157, 100, target);
    set_bytes(&header, 257, 6, "ustar ");
    set_bytes(&header, 263, 2, " \0");
    write_checksum(&header);
    archive->append(header);
}

static void finalize_tar(std::string* archive) {
    archive->append(1024, '\0');
}
#endif

static std::string read_text_file(const fs::path& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

static std::string read_binary_file(const fs::path& path) {
    return read_text_file(path);
}

static void write_binary_file(const fs::path& path, const std::string& value) {
    write_text_file(path, value);
}

static std::string decode_data_url_bytes(const std::string& image_url) {
    const std::size_t comma = image_url.find(',');
    assert(comma != std::string::npos);
    return base64_decode_bytes(image_url.substr(comma + 1));
}

static HttpRequest transfer_import_request(
    const fs::path& destination,
    const std::string& archive
) {
    HttpRequest request;
    request.method = "POST";
    request.path = "/v1/transfer/import";
    request.headers["x-remote-exec-source-type"] = "file";
    request.headers["x-remote-exec-destination-path"] = destination.string();
    request.headers["x-remote-exec-overwrite"] = "replace";
    request.headers["x-remote-exec-create-parent"] = "true";
    request.headers["x-remote-exec-symlink-mode"] = "preserve";
    request.headers["x-remote-exec-compression"] = "none";
    request.body = archive;
    return request;
}

static void assert_bad_request_for_transfer_import(
    AppState& state,
    const HttpRequest& request,
    const fs::path& destination,
    const std::string& message_fragment
) {
    const HttpResponse response = route_request(state, request);
    assert(response.status == 400);
    const Json body = Json::parse(response.body);
    assert(body.at("code").get<std::string>() == "bad_request");
    assert(
        body.at("message").get<std::string>().find(message_fragment) != std::string::npos
    );
    assert(!fs::exists(destination));
}

static void assert_target_info_and_basic_helpers(AppState& state) {
    HttpRequest info_request;
    info_request.method = "POST";
    info_request.path = "/v1/target-info";
    info_request.headers[request_id_header_name()] = "client-req-123";
    const HttpResponse info_response = route_request(state, info_request);
    assert(info_response.status == 200);
    assert(info_response.headers.at(request_id_header_name()) == "client-req-123");
    const Json info = Json::parse(info_response.body);
    assert(info.at("target").get<std::string>() == "cpp-test");
    assert(info.at("supports_pty").get<bool>() == process_session_supports_pty());
    assert(info.at("supports_image_read").get<bool>());
    assert(info.at("supports_port_forward").get<bool>());
    assert(info.at("port_forward_protocol_version").get<int>() == 4);

    HttpRequest generated_request;
    generated_request.method = "POST";
    generated_request.path = "/v1/health";
    const HttpResponse generated_response = route_request(state, generated_request);
    assert(generated_response.status == 200);
    assert(generated_response.headers.at(request_id_header_name()).find("req_cpp_") == 0);

    assert(normalize_port_forward_endpoint("8080") == "127.0.0.1:8080");
    assert(base64_decode_bytes(base64_encode_bytes(std::string("hello\0world", 11))).size() == 11);
}

static void assert_transfer_export_errors(AppState& state, const fs::path& root) {
    const HttpResponse compression_response = route_request(
        state,
        json_request(
            "/v1/transfer/export",
            Json{{"path", (root / "missing.txt").string()}, {"compression", "zstd"}}
        )
    );
    assert(compression_response.status == 400);
    assert(
        Json::parse(compression_response.body).at("code").get<std::string>() ==
        "transfer_compression_unsupported"
    );

    const HttpResponse missing_source_response = route_request(
        state,
        json_request("/v1/transfer/export", Json{{"path", (root / "missing.txt").string()}})
    );
    assert(missing_source_response.status == 400);
    assert(
        Json::parse(missing_source_response.body).at("code").get<std::string>() ==
        "transfer_source_missing"
    );
}

static void assert_image_routes(AppState& state, const fs::path& root) {
    const fs::path image_file = root / "tiny.png";
    write_binary_file(
        image_file,
        base64_decode_bytes(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aL9sAAAAASUVORK5CYII="
        )
    );
    const std::string original_image = read_binary_file(image_file);

    const HttpResponse image_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "tiny.png"}, {"workdir", root.string()}}
        )
    );
    assert(image_response.status == 200);
    const Json image = Json::parse(image_response.body);
    assert(image.at("detail").get<std::string>() == "original");
    assert(image.at("image_url").get<std::string>().find("data:image/png;base64,") == 0);
    assert(decode_data_url_bytes(image.at("image_url").get<std::string>()) == original_image);

    const HttpResponse invalid_detail_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "tiny.png"}, {"workdir", root.string()}, {"detail", "low"}}
        )
    );
    assert(invalid_detail_response.status == 400);
    const Json invalid_detail = Json::parse(invalid_detail_response.body);
    assert(invalid_detail.at("code").get<std::string>() == "invalid_detail");

    const HttpResponse missing_image_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "missing.png"}, {"workdir", root.string()}}
        )
    );
    assert(missing_image_response.status == 400);
    const Json missing_image = Json::parse(missing_image_response.body);
    assert(missing_image.at("code").get<std::string>() == "image_missing");

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

#ifndef _WIN32
    const fs::path blocked_image_dir = root / "blocked-image";
    fs::create_directories(blocked_image_dir);
    write_binary_file(
        blocked_image_dir / "blocked.png",
        base64_decode_bytes(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4////fwAJ+wP9KobjigAAAABJRU5ErkJggg=="
        )
    );
    fs::permissions(blocked_image_dir, fs::perms::none, fs::perm_options::replace);
    const HttpResponse blocked_image_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "blocked-image/blocked.png"}, {"workdir", root.string()}}
        )
    );
    fs::permissions(
        blocked_image_dir,
        fs::perms::owner_all,
        fs::perm_options::replace
    );
    assert(blocked_image_response.status == 500);
    const Json blocked_image_error = Json::parse(blocked_image_response.body);
    assert(blocked_image_error.at("code").get<std::string>() == "internal_error");
#endif
}

static void assert_patch_route_audit_fields(AppState& state, const fs::path& root) {
    const fs::path patch_file = root / "patch-audit.txt";
    const std::string patch_text =
        "*** Begin Patch\n"
        "*** Add File: patch-audit.txt\n"
        "+audit\n"
        "*** End Patch\n";

    const HttpResponse response = route_request(
        state,
        json_request(
            "/v1/patch/apply",
            Json{{"workdir", root.string()}, {"patch", patch_text}}
        )
    );
    assert(response.status == 200);
    const Json body = Json::parse(response.body);
    assert(body.at("output").get<std::string>().find("A patch-audit.txt") != std::string::npos);
    assert(body.at("daemon_instance_id").get<std::string>() == state.daemon_instance_id);
    assert(body.at("updated_paths").size() == 1);
    assert(body.at("updated_paths")[0].get<std::string>() == "A patch-audit.txt");
    assert(read_text_file(patch_file) == "audit\n");
}

static void assert_transfer_path_info_routes(AppState& state, const fs::path& root) {
    const fs::path source_file = root / "transfer-source.txt";
    write_text_file(source_file, "route transfer payload");

    const HttpResponse source_info_response = route_request(
        state,
        json_request("/v1/transfer/path-info", Json{{"path", source_file.string()}})
    );
    assert(source_info_response.status == 200);
    const Json source_info = Json::parse(source_info_response.body);
    assert(source_info.at("exists").get<bool>());
    assert(!source_info.at("is_directory").get<bool>());

    const HttpResponse root_info_response = route_request(
        state,
        json_request("/v1/transfer/path-info", Json{{"path", root.string()}})
    );
    assert(root_info_response.status == 200);
    const Json root_info = Json::parse(root_info_response.body);
    assert(root_info.at("exists").get<bool>());
    assert(root_info.at("is_directory").get<bool>());

    const HttpResponse relative_info_response = route_request(
        state,
        json_request("/v1/transfer/path-info", Json{{"path", "relative/path.txt"}})
    );
    assert(relative_info_response.status == 400);
    const Json relative_info_error = Json::parse(relative_info_response.body);
    assert(relative_info_error.at("code").get<std::string>() == "transfer_path_not_absolute");

#ifndef _WIN32
    const fs::path blocked_transfer_dir = root / "blocked-transfer";
    fs::create_directories(blocked_transfer_dir);
    write_text_file(blocked_transfer_dir / "inside.txt", "secret");
    fs::permissions(blocked_transfer_dir, fs::perms::none, fs::perm_options::replace);
    const HttpResponse blocked_transfer_info_response = route_request(
        state,
        json_request(
            "/v1/transfer/path-info",
            Json{{"path", (blocked_transfer_dir / "inside.txt").string()}}
        )
    );
    fs::permissions(
        blocked_transfer_dir,
        fs::perms::owner_all,
        fs::perm_options::replace
    );
    assert(blocked_transfer_info_response.status == 500);
    const Json blocked_transfer_info_error = Json::parse(blocked_transfer_info_response.body);
    assert(blocked_transfer_info_error.at("code").get<std::string>() == "internal_error");
#endif
}

static std::string assert_transfer_export_and_exclude_routes(
    AppState& state,
    const fs::path& root
) {
    const fs::path source_file = root / "transfer-source.txt";
    write_text_file(source_file, "route transfer payload");

    const HttpResponse export_response = route_request(
        state,
        json_request("/v1/transfer/export", Json{{"path", source_file.string()}})
    );
    assert(export_response.status == 200);
    assert(export_response.headers.at("Content-Type") == "application/octet-stream");
    assert(export_response.headers.at("x-remote-exec-source-type") == "file");
    assert(export_response.headers.at("x-remote-exec-compression") == "none");
    assert(!export_response.body.empty());

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
        state,
        json_request(
            "/v1/transfer/export",
            Json{{"path", exclude_source.string()}, {"exclude", exclude_patterns}}
        )
    );
    assert(export_excluded_response.status == 200);
    const ImportSummary excluded_import = import_path(
        export_excluded_response.body,
        TransferSourceType::Directory,
        (root / "transfer-exclude-dest").string(),
        "replace",
        true
    );
    assert(excluded_import.warnings.empty());
    assert(read_text_file(root / "transfer-exclude-dest" / "keep.txt") == "keep");
    assert(read_text_file(root / "transfer-exclude-dest" / "logs" / "readme.txt") == "keep");
    assert(!fs::exists(root / "transfer-exclude-dest" / "top.log"));
    assert(!fs::exists(root / "transfer-exclude-dest" / ".git"));
    assert(!fs::exists(root / "transfer-exclude-dest" / "logs" / "app.log"));

    Json malformed_exclude = Json::array();
    malformed_exclude.push_back("tmp/[abc");
    const HttpResponse invalid_exclude_response = route_request(
        state,
        json_request(
            "/v1/transfer/export",
            Json{{"path", exclude_source.string()}, {"exclude", malformed_exclude}}
        )
    );
    assert(invalid_exclude_response.status == 400);
    const Json invalid_exclude = Json::parse(invalid_exclude_response.body);
    assert(invalid_exclude.at("code").get<std::string>() == "transfer_failed");
    assert(
        invalid_exclude.at("message").get<std::string>().find("invalid exclude pattern") !=
        std::string::npos
    );

    return export_response.body;
}

static void assert_transfer_import_routes(
    AppState& state,
    const fs::path& root,
    const std::string& export_body
) {
    HttpRequest import_request;
    import_request.method = "POST";
    import_request.path = "/v1/transfer/import";
    import_request.headers["x-remote-exec-source-type"] = "file";
    import_request.headers["x-remote-exec-destination-path"] = (root / "transfer-dest.txt").string();
    import_request.headers["x-remote-exec-overwrite"] = "replace";
    import_request.headers["x-remote-exec-create-parent"] = "true";
    import_request.headers["x-remote-exec-symlink-mode"] = "preserve";
    import_request.headers["x-remote-exec-compression"] = "none";
    import_request.body = export_body;

    const HttpResponse import_response = route_request(state, import_request);
    assert(import_response.status == 200);
    const Json imported = Json::parse(import_response.body);
    assert(imported.at("source_type").get<std::string>() == "file");
    assert(imported.at("files_copied").get<std::uint64_t>() == 1);
    assert(imported.at("bytes_copied").get<std::uint64_t>() == 22);
    assert(imported.at("replaced").get<bool>() == false);
    assert(imported.at("warnings").empty());
    assert(read_text_file(root / "transfer-dest.txt") == "route transfer payload");

    HttpRequest optional_defaults_import =
        transfer_import_request(root / "transfer-defaults.txt", export_body);
    optional_defaults_import.headers.erase("x-remote-exec-symlink-mode");
    optional_defaults_import.headers.erase("x-remote-exec-compression");
    const HttpResponse optional_defaults_response =
        route_request(state, optional_defaults_import);
    assert(optional_defaults_response.status == 200);
    assert(read_text_file(root / "transfer-defaults.txt") == "route transfer payload");

    HttpRequest missing_create_parent =
        transfer_import_request(root / "missing-create-parent.txt", export_body);
    missing_create_parent.headers.erase("x-remote-exec-create-parent");
    assert_bad_request_for_transfer_import(
        state,
        missing_create_parent,
        root / "missing-create-parent.txt",
        "x-remote-exec-create-parent"
    );

    HttpRequest invalid_create_parent =
        transfer_import_request(root / "invalid-create-parent.txt", export_body);
    invalid_create_parent.headers["x-remote-exec-create-parent"] = "yes";
    assert_bad_request_for_transfer_import(
        state,
        invalid_create_parent,
        root / "invalid-create-parent.txt",
        "x-remote-exec-create-parent"
    );

    HttpRequest invalid_source_type =
        transfer_import_request(root / "invalid-source-type.txt", export_body);
    invalid_source_type.headers["x-remote-exec-source-type"] = "folder";
    assert_bad_request_for_transfer_import(
        state,
        invalid_source_type,
        root / "invalid-source-type.txt",
        "x-remote-exec-source-type"
    );

    HttpRequest invalid_overwrite =
        transfer_import_request(root / "invalid-overwrite.txt", export_body);
    invalid_overwrite.headers["x-remote-exec-overwrite"] = "clobber";
    assert_bad_request_for_transfer_import(
        state,
        invalid_overwrite,
        root / "invalid-overwrite.txt",
        "x-remote-exec-overwrite"
    );

    HttpRequest invalid_compression =
        transfer_import_request(root / "invalid-compression.txt", export_body);
    invalid_compression.headers["x-remote-exec-compression"] = "gzip";
    assert_bad_request_for_transfer_import(
        state,
        invalid_compression,
        root / "invalid-compression.txt",
        "x-remote-exec-compression"
    );

    HttpRequest invalid_symlink_mode =
        transfer_import_request(root / "invalid-symlink-mode.txt", export_body);
    invalid_symlink_mode.headers["x-remote-exec-symlink-mode"] = "copy";
    assert_bad_request_for_transfer_import(
        state,
        invalid_symlink_mode,
        root / "invalid-symlink-mode.txt",
        "x-remote-exec-symlink-mode"
    );

    fs::create_directories(root / "merge-dir");
    HttpRequest merge_file_into_directory_request;
    merge_file_into_directory_request.method = "POST";
    merge_file_into_directory_request.path = "/v1/transfer/import";
    merge_file_into_directory_request.headers["x-remote-exec-source-type"] = "file";
    merge_file_into_directory_request.headers["x-remote-exec-destination-path"] =
        (root / "merge-dir").string();
    merge_file_into_directory_request.headers["x-remote-exec-overwrite"] = "merge";
    merge_file_into_directory_request.headers["x-remote-exec-create-parent"] = "true";
    merge_file_into_directory_request.headers["x-remote-exec-symlink-mode"] = "preserve";
    merge_file_into_directory_request.headers["x-remote-exec-compression"] = "none";
    merge_file_into_directory_request.body = export_body;
    const HttpResponse merge_file_into_directory_response =
        route_request(state, merge_file_into_directory_request);
    assert(merge_file_into_directory_response.status == 400);
    const Json merge_file_into_directory_error =
        Json::parse(merge_file_into_directory_response.body);
    assert(
        merge_file_into_directory_error.at("code").get<std::string>() ==
        "transfer_destination_unsupported"
    );
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

    AppState sandbox_state;
    initialize_server_routes_state(sandbox_state, root);
    sandbox_state.config.sandbox_configured = true;
    sandbox_state.config.sandbox.exec_cwd.allow.push_back(exec_allowed.string());
    sandbox_state.config.sandbox.read.allow.push_back(read_allowed.string());
    sandbox_state.config.sandbox.write.allow.push_back(write_allowed.string());
    sandbox_state.config.sandbox.write.deny.push_back(denied_link_target_root.string());
    enable_sandbox(sandbox_state);

    const HttpResponse sandbox_export_denied = route_request(
        sandbox_state,
        json_request("/v1/transfer/export", Json{{"path", (outside / "outside.txt").string()}})
    );
    assert(sandbox_export_denied.status == 400);
    assert(
        Json::parse(sandbox_export_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );

    const HttpResponse sandbox_path_info_denied = route_request(
        sandbox_state,
        json_request("/v1/transfer/path-info", Json{{"path", (outside / "dest.txt").string()}})
    );
    assert(sandbox_path_info_denied.status == 400);
    assert(
        Json::parse(sandbox_path_info_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );

    const HttpResponse sandbox_export_allowed = route_request(
        sandbox_state,
        json_request("/v1/transfer/export", Json{{"path", (read_allowed / "source.txt").string()}})
    );
    assert(sandbox_export_allowed.status == 200);

    HttpRequest sandbox_import_denied_request;
    sandbox_import_denied_request.method = "POST";
    sandbox_import_denied_request.path = "/v1/transfer/import";
    sandbox_import_denied_request.headers["x-remote-exec-source-type"] = "file";
    sandbox_import_denied_request.headers["x-remote-exec-destination-path"] =
        (outside / "dest.txt").string();
    sandbox_import_denied_request.headers["x-remote-exec-overwrite"] = "replace";
    sandbox_import_denied_request.headers["x-remote-exec-create-parent"] = "true";
    sandbox_import_denied_request.headers["x-remote-exec-symlink-mode"] = "preserve";
    sandbox_import_denied_request.headers["x-remote-exec-compression"] = "none";
    sandbox_import_denied_request.body = sandbox_export_allowed.body;
    const HttpResponse sandbox_import_denied =
        route_request(sandbox_state, sandbox_import_denied_request);
    assert(sandbox_import_denied.status == 400);
    assert(
        Json::parse(sandbox_import_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );

#ifndef _WIN32
    std::string denied_symlink_archive;
    append_tar_directory(&denied_symlink_archive, ".");
    append_tar_symlink(
        &denied_symlink_archive,
        "allowed-link",
        "denied-link-target/secret.txt"
    );
    finalize_tar(&denied_symlink_archive);

    HttpRequest sandbox_symlink_target_denied_request;
    sandbox_symlink_target_denied_request.method = "POST";
    sandbox_symlink_target_denied_request.path = "/v1/transfer/import";
    sandbox_symlink_target_denied_request.headers["x-remote-exec-source-type"] = "directory";
    sandbox_symlink_target_denied_request.headers["x-remote-exec-destination-path"] =
        write_allowed.string();
    sandbox_symlink_target_denied_request.headers["x-remote-exec-overwrite"] = "merge";
    sandbox_symlink_target_denied_request.headers["x-remote-exec-create-parent"] = "true";
    sandbox_symlink_target_denied_request.headers["x-remote-exec-symlink-mode"] = "preserve";
    sandbox_symlink_target_denied_request.headers["x-remote-exec-compression"] = "none";
    sandbox_symlink_target_denied_request.body = denied_symlink_archive;
    const HttpResponse sandbox_symlink_target_denied =
        route_request(sandbox_state, sandbox_symlink_target_denied_request);
    assert(sandbox_symlink_target_denied.status == 400);
    assert(
        Json::parse(sandbox_symlink_target_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );
    assert(!fs::exists(write_allowed / "allowed-link"));
#endif

    const std::string patch_denied_text =
        "*** Begin Patch\n"
        "*** Add File: " + (outside / "patched.txt").string() + "\n"
        "+denied\n"
        "*** End Patch\n";
    const HttpResponse sandbox_patch_denied = route_request(
        sandbox_state,
        json_request("/v1/patch/apply", Json{{"workdir", write_allowed.string()}, {"patch", patch_denied_text}})
    );
    assert(sandbox_patch_denied.status == 400);
    assert(
        Json::parse(sandbox_patch_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );
    assert(!fs::exists(outside / "patched.txt"));

    const HttpResponse sandbox_exec_denied = route_request(
        sandbox_state,
        json_request(
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
    assert(sandbox_exec_denied.status == 400);
    assert(
        Json::parse(sandbox_exec_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );
}

void run_platform_neutral_server_route_tests(AppState& state, const fs::path& root) {
    assert_target_info_and_basic_helpers(state);
    assert_transfer_export_errors(state, root);
    assert_image_routes(state, root);
    assert_patch_route_audit_fields(state, root);
    assert_transfer_path_info_routes(state, root);
    const std::string export_body = assert_transfer_export_and_exclude_routes(state, root);
    assert_transfer_import_routes(state, root, export_body);
    assert_sandbox_routes(root);
}
