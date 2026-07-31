#include <string>

#include "codec/base64_codec.h"
#include "core/logging.h"
#include "image/image_ops.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_image.h"

HttpResponse handle_image_read(const ImageRouteContext& context, const HttpRequest& request) {
    return handle_image_rpc_route("image/read", [&](HttpResponse& response) {
        const Json body = parse_json_body(request);
        const std::string path =
            resolve_authorized_input_path(context.paths, body, "path", SANDBOX_READ);
        const ImageReadResult image = read_image_original(path);
        write_json(
            response,
            Json{
                {"image_url",
                 "data:" + image.mime_type + ";base64," + base64_encode_bytes(image.bytes)},
                {"detail", body.value("detail", Json(nullptr))},
            }
        );
    });
}
