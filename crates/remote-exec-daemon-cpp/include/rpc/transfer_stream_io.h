#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

#include "transfer/transfer_ops.h"

class TransferStreamByteReader {
public:
    virtual ~TransferStreamByteReader() {}
    virtual std::size_t read(char* data, std::size_t size) = 0;
};

class TransferStreamChunkWriter {
public:
    virtual ~TransferStreamChunkWriter() {}
    void write_chunk(const std::string& data);
    virtual void write_chunk(const char* data, std::size_t size) = 0;
    virtual void finish() = 0;
};

class TransferStreamArchiveReader : public TransferArchiveReader {
public:
    explicit TransferStreamArchiveReader(TransferStreamByteReader* reader);

    bool read_exact_or_eof(char* data, std::size_t size);

private:
    void read_preface();
    void read_transport_exact(char* data, std::size_t size, const std::string& label);
    bool read_next_data_frame_or_terminal();

    TransferStreamByteReader* reader_;
    std::string data_;
    std::size_t offset_;
    bool preface_read_;
    bool terminal_;
};

class StringTransferStreamByteReader : public TransferStreamByteReader {
public:
    explicit StringTransferStreamByteReader(const std::string* body);

    std::size_t read(char* data, std::size_t size) override;

private:
    const std::string* body_;
    std::size_t offset_;
};

class ChunkedTransferStreamArchiveSink : public TransferArchiveSink {
public:
    explicit ChunkedTransferStreamArchiveSink(TransferStreamChunkWriter* chunks);

    void send_preface();
    void write(const char* data, std::size_t size);
    void send_complete();
    void send_error_payload(const std::string& payload);

private:
    TransferStreamChunkWriter* chunks_;
    std::uint64_t archive_bytes_;
};

std::string framed_transfer_body(const std::string& archive);
