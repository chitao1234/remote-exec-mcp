#include <string>

#include "base64_codec.h"
#include "image_ops.h"
#include "core/logging.h"
#include "rpc_failures.h"
#include "server_request_utils.h"
#include "server_route_image.h"

namespace {

ImageFailure invalid_detail_failure(const std::string& detail) {
    return ImageFailure(
        ImageRpcCode::InvalidDetail,
        "view_image.detail only supports `original`; omit `detail` for default original behavior, got `" + detail +
            "`");
}

} // namespace

HttpResponse handle_image_read(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const std::string detail = body.value("detail", std::string());
        if (!detail.empty() && detail != "original") {
            throw invalid_detail_failure(detail);
        }

        const std::string path = resolve_authorized_input_path(state, body, "path", SANDBOX_READ);
        const ImageReadResult image = read_image_original(path);
        write_json(response,
                   Json{
                       {"image_url", "data:" + image.mime_type + ";base64," + base64_encode_bytes(image.bytes)},
                       {"detail", "original"},
                   });
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("image/read failed: ") + ex.what());
        write_rpc_error(response, 400, image_error_code_name(ImageRpcCode::SandboxDenied), ex.what());
    } catch (const ImageFailure& failure) {
        log_message(LOG_WARN, "server", "image/read failed: " + failure.message);
        write_rpc_error(
            response, image_error_status(failure.code), image_error_code_name(failure.code), failure.message);
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "image/read failed: " + message);
        write_rpc_error(response,
                        image_error_status(ImageRpcCode::Internal),
                        image_error_code_name(ImageRpcCode::Internal),
                        message);
    }

    return response;
}
