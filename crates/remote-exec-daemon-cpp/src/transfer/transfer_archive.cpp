#include <algorithm>

#include "rpc/rpc_failures.h"
#include "transfer_archive.h"
#include "transfer_tar_codec.h"

namespace transfer_archive {

StringArchiveReader::StringArchiveReader(const std::string* archive) : archive_(archive), offset_(0) {
}

bool StringArchiveReader::read_exact_or_eof(char* data, std::size_t size) {
    if (size == 0U) {
        return true;
    }
    if (offset_ >= archive_->size()) {
        return false;
    }
    if (archive_->size() - offset_ < size) {
        throw TransferFailure(TransferRpcCode::TransferFailed, "truncated transfer body");
    }
    std::copy(archive_->data() + offset_, archive_->data() + offset_ + size, data);
    offset_ += size;
    return true;
}

StringArchiveSink::StringArchiveSink(std::string* output) : output_(output) {
}

void StringArchiveSink::write(const char* data, std::size_t size) {
    output_->append(data, size);
}

void read_exact_or_throw(TransferArchiveReader& reader,
                         char* data,
                         std::size_t size,
                         const std::string& error_message) {
    if (!reader.read_exact_or_eof(data, size)) {
        throw TransferFailure(TransferRpcCode::TransferFailed, error_message);
    }
}

std::string read_exact_string(TransferArchiveReader& reader, std::uint64_t size, const std::string& error_message) {
    transfer_tar_codec::ensure_u64_fits_size_t(size, "tar entry size");
    std::string body(static_cast<std::size_t>(size), '\0');
    if (!body.empty()) {
        read_exact_or_throw(reader, &body[0], body.size(), error_message);
    }
    return body;
}

void skip_exact(TransferArchiveReader& reader, std::uint64_t size, const std::string& error_message) {
    char buffer[8192];
    std::uint64_t remaining = size;
    while (remaining > 0U) {
        const std::size_t requested = remaining < sizeof(buffer) ? static_cast<std::size_t>(remaining) : sizeof(buffer);
        read_exact_or_throw(reader, buffer, requested, error_message);
        remaining -= static_cast<std::uint64_t>(requested);
    }
}

} // namespace transfer_archive
