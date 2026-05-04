#include "rpc_failures.h"

TransferFailure::TransferFailure(TransferRpcCode code, const std::string& message)
    : std::runtime_error(message), code(code), message(message) {}

ImageFailure::ImageFailure(ImageRpcCode code, const std::string& message)
    : std::runtime_error(message), code(code), message(message) {}

const char* transfer_error_code_name(TransferRpcCode code) {
    switch (code) {
    case TransferRpcCode::SandboxDenied:
        return "sandbox_denied";
    case TransferRpcCode::PathNotAbsolute:
        return "transfer_path_not_absolute";
    case TransferRpcCode::DestinationExists:
        return "transfer_destination_exists";
    case TransferRpcCode::ParentMissing:
        return "transfer_parent_missing";
    case TransferRpcCode::DestinationUnsupported:
        return "transfer_destination_unsupported";
    case TransferRpcCode::CompressionUnsupported:
        return "transfer_compression_unsupported";
    case TransferRpcCode::SourceUnsupported:
        return "transfer_source_unsupported";
    case TransferRpcCode::SourceMissing:
        return "transfer_source_missing";
    case TransferRpcCode::TransferFailed:
        return "transfer_failed";
    }
    return "transfer_failed";
}

const char* image_error_code_name(ImageRpcCode code) {
    switch (code) {
    case ImageRpcCode::SandboxDenied:
        return "sandbox_denied";
    case ImageRpcCode::InvalidDetail:
        return "invalid_detail";
    case ImageRpcCode::Missing:
        return "image_missing";
    case ImageRpcCode::NotFile:
        return "image_not_file";
    case ImageRpcCode::DecodeFailed:
        return "image_decode_failed";
    }
    return "image_decode_failed";
}
