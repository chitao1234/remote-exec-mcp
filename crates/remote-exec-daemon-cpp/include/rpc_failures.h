#pragma once

#include <stdexcept>
#include <string>

enum class TransferRpcCode {
    SandboxDenied,
    PathNotAbsolute,
    DestinationExists,
    ParentMissing,
    DestinationUnsupported,
    CompressionUnsupported,
    SourceUnsupported,
    SourceMissing,
    Internal,
    TransferFailed,
};

enum class ImageRpcCode {
    SandboxDenied,
    InvalidDetail,
    Missing,
    NotFile,
    DecodeFailed,
    Internal,
};

class TransferFailure : public std::runtime_error {
public:
    TransferFailure(TransferRpcCode code, const std::string& message);

    TransferRpcCode code;
    std::string message;
};

class ImageFailure : public std::runtime_error {
public:
    ImageFailure(ImageRpcCode code, const std::string& message);

    ImageRpcCode code;
    std::string message;
};

const char* transfer_error_code_name(TransferRpcCode code);
const char* image_error_code_name(ImageRpcCode code);
int transfer_error_status(TransferRpcCode code);
int image_error_status(ImageRpcCode code);
