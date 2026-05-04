#include <cerrno>
#include <cstring>
#include <fstream>
#include <string>
#include <sys/stat.h>

#include "filesystem_sandbox.h"
#include "logging.h"
#include "path_policy.h"
#include "rpc_failures.h"
#include "server_route_image.h"

namespace {

std::string resolve_workdir(const AppState& state, const Json& body) {
    const std::string raw = body.value("workdir", state.config.default_workdir);
    if (raw.empty()) {
        return state.config.default_workdir;
    }

    const PathPolicy policy = host_path_policy();
    if (is_absolute_for_policy(policy, raw)) {
        return normalize_for_system(policy, raw);
    }
    return join_for_policy(policy, state.config.default_workdir, raw);
}

const CompiledFilesystemSandbox* active_sandbox(const AppState& state) {
    return state.sandbox_enabled ? &state.sandbox : NULL;
}

void authorize_sandbox_path(
    const AppState& state,
    SandboxAccess access,
    const std::string& path
) {
    authorize_path(host_path_policy(), active_sandbox(state), access, path);
}

std::string resolve_input_path(
    const AppState& state,
    const Json& body,
    const std::string& key
) {
    const std::string raw = body.at(key).get<std::string>();
    const PathPolicy policy = host_path_policy();
    if (is_absolute_for_policy(policy, raw)) {
        return normalize_for_system(policy, raw);
    }
    return join_for_policy(policy, resolve_workdir(state, body), raw);
}

ImageFailure invalid_detail_failure(const std::string& detail) {
    return ImageFailure(
        ImageRpcCode::InvalidDetail,
        "view_image.detail only supports `original`; omit `detail` for default original behavior, got `" +
            detail + "`"
    );
}

ImageFailure missing_image_failure(const std::string& path) {
    return ImageFailure(
        ImageRpcCode::Missing,
        "unable to locate image at `" + path + "`: No such file or directory"
    );
}

ImageFailure not_file_image_failure(const std::string& path) {
    return ImageFailure(
        ImageRpcCode::NotFile,
        "image path `" + path + "` is not a file"
    );
}

ImageFailure decode_failed_image(const std::string& message) {
    return ImageFailure(ImageRpcCode::DecodeFailed, message);
}

ImageFailure internal_image_failure(const std::string& message) {
    return ImageFailure(ImageRpcCode::Internal, message);
}

std::string read_binary_file_bytes(const std::string& path) {
    errno = 0;
    std::ifstream input(path.c_str(), std::ios::binary);
    if (!input) {
        const int error_code = errno;
        if (error_code != 0) {
            throw internal_image_failure(
                "unable to read image at `" + path + "`: " + std::strerror(error_code)
            );
        }
        throw internal_image_failure("unable to read image at `" + path + "`");
    }
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

void require_regular_image_file(const std::string& path) {
    struct stat st;
    if (stat(path.c_str(), &st) == 0) {
        if ((st.st_mode & S_IFMT) != S_IFREG) {
            throw not_file_image_failure(path);
        }
        return;
    }

    const int error_code = errno;
    if (error_code == ENOENT || error_code == ENOTDIR) {
        throw missing_image_failure(path);
    }

    throw internal_image_failure(
        "unable to access image at `" + path + "`: " + std::strerror(error_code)
    );
}

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
    throw decode_failed_image(
        "unable to process image at `" + path + "`: unsupported image format"
    );
}

}  // namespace

HttpResponse handle_image_read(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const std::string detail = body.value("detail", std::string());
        if (!detail.empty() && detail != "original") {
            throw invalid_detail_failure(detail);
        }

        const std::string path = resolve_input_path(state, body, "path");
        authorize_sandbox_path(state, SANDBOX_READ, path);
        require_regular_image_file(path);

        const std::string bytes = read_binary_file_bytes(path);
        const std::string mime = image_mime_type(path, bytes);
        write_json(
            response,
            Json{
                {"image_url", "data:" + mime + ";base64," + base64_encode_bytes(bytes)},
                {"detail", "original"},
            }
        );
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("image/read failed: ") + ex.what());
        write_rpc_error(
            response,
            400,
            image_error_code_name(ImageRpcCode::SandboxDenied),
            ex.what()
        );
    } catch (const ImageFailure& failure) {
        log_message(LOG_WARN, "server", "image/read failed: " + failure.message);
        write_rpc_error(
            response,
            image_error_status(failure.code),
            image_error_code_name(failure.code),
            failure.message
        );
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "image/read failed: " + message);
        write_rpc_error(
            response,
            image_error_status(ImageRpcCode::Internal),
            image_error_code_name(ImageRpcCode::Internal),
            message
        );
    }

    return response;
}
