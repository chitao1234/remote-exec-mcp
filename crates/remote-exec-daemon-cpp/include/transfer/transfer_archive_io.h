#pragma once

#include <cstddef>
#include <string>

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
