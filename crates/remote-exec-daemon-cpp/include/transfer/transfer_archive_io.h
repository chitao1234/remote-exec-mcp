#pragma once

#include <cstddef>
#include <string>

const std::size_t TRANSFER_ARCHIVE_IO_BUFFER_SIZE = 256U * 1024U;

class TransferArchiveReader {
public:
    virtual ~TransferArchiveReader();
    virtual bool read_exact_or_eof(char* data, std::size_t size) = 0;
};

class TransferArchiveSink {
public:
    virtual ~TransferArchiveSink();
    virtual void write(const char* data, std::size_t size) = 0;

    void write_string(const std::string& data);
};
