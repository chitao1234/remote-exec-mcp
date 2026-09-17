#include "rpc/server_route_file.h"

#include <string>

#include "core/logging.h"
#include "policy/file_ops.h"
#include "rpc/file_request_utils.h"
#include "rpc/server_route_common.h"

HttpResponse handle_file_read(const FileRouteContext& context, const HttpRequest& request) {
    return handle_file_rpc_route("file/read", [&](HttpResponse& response) {
        const FileReadRequestSpec parsed = prepare_file_read_request(request);
        const FileReadOutcome outcome = read_file_text(context.paths, context.text_decode, parsed);
        write_json(
            response,
            Json{
                {"output", outcome.output},
                {"lines_returned", outcome.lines_returned},
                {"total_lines", outcome.total_lines},
                {"eof", outcome.eof},
            }
        );
    });
}

HttpResponse handle_file_write(const FileRouteContext& context, const HttpRequest& request) {
    return handle_file_rpc_route("file/write", [&](HttpResponse& response) {
        const FileWriteRequestSpec parsed = prepare_file_write_request(request);
        const FileWriteOutcome outcome =
            write_file_text(context.paths, context.text_decode, parsed);
        write_json(
            response,
            Json{
                {"created", outcome.created},
                {"line_count", outcome.line_count},
            }
        );
    });
}

HttpResponse handle_file_edit(const FileRouteContext& context, const HttpRequest& request) {
    return handle_file_rpc_route("file/edit", [&](HttpResponse& response) {
        const FileEditRequestSpec parsed = prepare_file_edit_request(request);
        const FileEditOutcome outcome = edit_file_text(context.paths, context.text_decode, parsed);
        write_json(
            response,
            Json{
                {"replacements", outcome.replacements},
                {"line_count", outcome.line_count},
            }
        );
    });
}
