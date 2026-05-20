#pragma once

#include <cstddef>
#include <cstdint>
#include <string>

#include "http/server_transport.h"
#include "transfer/transfer_ops.h"

class TransferStreamArchiveReader : public TransferArchiveReader {
public:
    TransferStreamArchiveReader();

    bool read_exact_or_eof(char* data, std::size_t size);

protected:
    virtual std::size_t read_transport(char* data, std::size_t size) = 0;

private:
    void read_preface();
    void read_transport_exact(char* data, std::size_t size, const std::string& label);
    bool read_next_data_frame_or_terminal();

    std::string data_;
    std::size_t offset_;
    bool preface_read_;
    bool terminal_;
};

class HttpBodyTransferArchiveReader : public TransferStreamArchiveReader {
public:
    explicit HttpBodyTransferArchiveReader(HttpRequestBodyStream* body);

protected:
    std::size_t read_transport(char* data, std::size_t size);

private:
    HttpRequestBodyStream* body_;
};

class StringTransferStreamArchiveReader : public TransferStreamArchiveReader {
public:
    explicit StringTransferStreamArchiveReader(const std::string* body);

protected:
    std::size_t read_transport(char* data, std::size_t size);

private:
    const std::string* body_;
    std::size_t offset_;
};

class ChunkedTransferStreamArchiveSink : public TransferArchiveSink {
public:
    explicit ChunkedTransferStreamArchiveSink(SOCKET client);

    void send_preface();
    void write(const char* data, std::size_t size);
    void send_complete();
    void send_error_payload(const std::string& payload);

private:
    SOCKET client_;
    std::uint64_t archive_bytes_;
};

std::string framed_transfer_body(const std::string& archive);
