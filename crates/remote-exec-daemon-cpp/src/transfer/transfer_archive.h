#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

#include "transfer/transfer_ops.h"

namespace transfer_archive {

class StringArchiveReader : public TransferArchiveReader {
public:
    explicit StringArchiveReader(const std::string* archive);

    bool read_exact_or_eof(char* data, std::size_t size);

private:
    const std::string* archive_;
    std::size_t offset_;
};

class StringArchiveSink : public TransferArchiveSink {
public:
    explicit StringArchiveSink(std::string* output);

    void write(const char* data, std::size_t size);

private:
    std::string* output_;
};

void read_exact_or_throw(TransferArchiveReader& reader, char* data, std::size_t size, const std::string& error_message);
std::string read_exact_string(TransferArchiveReader& reader, std::uint64_t size, const std::string& error_message);
void skip_exact(TransferArchiveReader& reader, std::uint64_t size, const std::string& error_message);

} // namespace transfer_archive
