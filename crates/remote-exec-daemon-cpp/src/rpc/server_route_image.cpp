#include <string>

#include "codec/base64_codec.h"
#include "image/image_ops.h"
#include "core/logging.h"
#include "rpc/rpc_failures.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_image.h"

namespace {

ImageFailure invalid_detail_failure(const std::string& detail) {
    return ImageFailure(
        ImageRpcCode::InvalidDetail,
        "view_image.detail only supports `original`; omit `detail` for default original behavior, got `" + detail +
            "`");
}

} // namespace

HttpResponse handle_image_read(AppState& state, const HttpRequest& request) {
    return handle_image_rpc_route("image/read", [&](HttpResponse& response) {
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
    });
}
