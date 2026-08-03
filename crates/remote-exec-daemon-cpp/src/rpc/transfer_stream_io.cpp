#include "rpc/transfer_stream_io.h"

#include <algorithm>
#include <cstring>
#include <limits>

#include "rpc/server_contract.h"
#include "rpc/transfer_stream_codec.h"

void TransferStreamChunkWriter::write_chunk(const std::string& data) {
    write_chunk(data.data(), data.size());
}

TransferStreamArchiveReader::TransferStreamArchiveReader(TransferStreamByteReader* reader)
    : reader_(reader), offset_(0U), preface_read_(false), terminal_(false) {
}

bool TransferStreamArchiveReader::read_exact_or_eof(char* data, std::size_t size) {
    read_preface();
    if (size == 0U) {
        return true;
    }

    std::size_t offset = 0U;
    while (offset < size) {
        if (offset_ < data_.size()) {
            const std::size_t available = data_.size() - offset_;
            const std::size_t count = std::min<std::size_t>(available, size - offset);
            std::memcpy(data + offset, data_.data() + offset_, count);
            offset_ += count;
            offset += count;
            if (offset_ == data_.size()) {
                data_.clear();
                offset_ = 0U;
            }
            continue;
        }

        if (!read_next_data_frame_or_terminal()) {
            if (offset == 0U) {
                return false;
            }
            throw TransferFailure(TransferRpcCode::TransferFailed, "truncated transfer body");
        }
    }
    return true;
}

void TransferStreamArchiveReader::read_preface() {
    if (preface_read_) {
        return;
    }
    std::string preface(server_contract::TRANSFER_STREAM_PREFACE_LEN, '\0');
    read_transport_exact(&preface[0], preface.size(), "transfer stream preface");
    if (preface
        != std::string(
            server_contract::TRANSFER_STREAM_PREFACE,
            server_contract::TRANSFER_STREAM_PREFACE_LEN
        )) {
        throw TransferFailure(TransferRpcCode::BadRequest, "invalid transfer stream preface");
    }
    preface_read_ = true;
}

void TransferStreamArchiveReader::read_transport_exact(
    char* data,
    std::size_t size,
    const std::string& label
) {
    std::size_t offset = 0U;
    while (offset < size) {
        const std::size_t received = reader_->read(data + offset, size - offset);
        if (received == 0U) {
            throw TransferFailure(
                TransferRpcCode::TransferFailed,
                "transfer stream ended before " + label
            );
        }
        offset += received;
    }
}

bool TransferStreamArchiveReader::read_next_data_frame_or_terminal() {
    if (terminal_) {
        return false;
    }

    for (;;) {
        unsigned char header[12];
        read_transport_exact(
            reinterpret_cast<char*>(header),
            server_contract::TRANSFER_STREAM_FRAME_HEADER_LEN,
            "transfer stream frame header"
        );
        const unsigned char frame_type = header[0];
        if (header[1] != 0U) {
            throw TransferFailure(
                TransferRpcCode::BadRequest,
                "transfer stream frame flags must be zero"
            );
        }
        if (header[2] != 0U || header[3] != 0U) {
            throw TransferFailure(
                TransferRpcCode::BadRequest,
                "transfer stream frame reserved field must be zero"
            );
        }
        const std::uint64_t payload_len = transfer_stream::read_u64_be(header + 4);
        const std::uint64_t limit = frame_type == transfer_stream::FRAME_DATA
                                        ? server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES
                                        : server_contract::TRANSFER_STREAM_CONTROL_FRAME_MAX_BYTES;
        if (payload_len > limit) {
            throw TransferFailure(
                TransferRpcCode::BadRequest,
                "transfer stream frame payload is too large"
            );
        }
        if (payload_len > static_cast<std::uint64_t>(std::numeric_limits<std::size_t>::max())) {
            throw TransferFailure(
                TransferRpcCode::BadRequest,
                "transfer stream frame payload is too large"
            );
        }

        std::string payload(static_cast<std::size_t>(payload_len), '\0');
        if (!payload.empty()) {
            read_transport_exact(&payload[0], payload.size(), "transfer stream frame payload");
        }

        if (frame_type == transfer_stream::FRAME_DATA) {
            if (payload.empty()) {
                continue;
            }
            data_.swap(payload);
            offset_ = 0U;
            return true;
        }
        if (frame_type == transfer_stream::FRAME_COMPLETE) {
            transfer_stream::parse_complete_payload(payload);
            terminal_ = true;
            return false;
        }
        if (frame_type == transfer_stream::FRAME_ERROR) {
            transfer_stream::throw_error_payload(payload);
        }
        throw TransferFailure(TransferRpcCode::BadRequest, "transfer stream frame type is unknown");
    }
}

StringTransferStreamByteReader::StringTransferStreamByteReader(const std::string* body)
    : body_(body), offset_(0U) {
}

std::size_t StringTransferStreamByteReader::read(char* data, std::size_t size) {
    if (offset_ >= body_->size()) {
        return 0U;
    }
    const std::size_t count = std::min<std::size_t>(size, body_->size() - offset_);
    std::memcpy(data, body_->data() + offset_, count);
    offset_ += count;
    return count;
}

ChunkedTransferStreamArchiveSink::ChunkedTransferStreamArchiveSink(TransferStreamChunkWriter* chunks
)
    : chunks_(chunks), archive_bytes_(0U), pending_() {
}

void ChunkedTransferStreamArchiveSink::send_preface() {
    chunks_->write_chunk(std::string(
        server_contract::TRANSFER_STREAM_PREFACE,
        server_contract::TRANSFER_STREAM_PREFACE_LEN
    ));
}

void ChunkedTransferStreamArchiveSink::write(const char* data, std::size_t size) {
    const std::size_t frame_size = server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES;
    if (!pending_.empty()) {
        const std::size_t needed = frame_size - pending_.size();
        const std::size_t count = std::min<std::size_t>(needed, size);
        pending_.append(data, count);
        data += count;
        size -= count;
        if (pending_.size() == frame_size) {
            flush_pending();
        }
    }

    while (size >= frame_size) {
        write_data_frame(data, frame_size);
        data += frame_size;
        size -= frame_size;
    }

    if (size > 0U) {
        pending_.append(data, size);
    }
}

void ChunkedTransferStreamArchiveSink::send_complete() {
    flush_pending();
    chunks_->write_chunk(transfer_stream::complete_frame(archive_bytes_));
    chunks_->finish();
}

void ChunkedTransferStreamArchiveSink::send_error_payload(const std::string& payload) {
    flush_pending();
    chunks_->write_chunk(transfer_stream::error_frame(payload));
    chunks_->finish();
}

void ChunkedTransferStreamArchiveSink::flush_pending() {
    if (!pending_.empty()) {
        write_data_frame(pending_.data(), pending_.size());
        pending_.clear();
    }
}

void ChunkedTransferStreamArchiveSink::write_data_frame(const char* data, std::size_t size) {
    chunks_->write_chunk(transfer_stream::data_frame(data, size));
    archive_bytes_ += static_cast<std::uint64_t>(size);
}

std::string framed_transfer_body(const std::string& archive) {
    std::string body(
        server_contract::TRANSFER_STREAM_PREFACE,
        server_contract::TRANSFER_STREAM_PREFACE_LEN
    );
    std::size_t offset = 0U;
    while (offset < archive.size()) {
        const std::size_t size = std::min<std::size_t>(
            server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES,
            archive.size() - offset
        );
        body += transfer_stream::data_frame(archive.data() + offset, size);
        offset += size;
    }
    body += transfer_stream::complete_frame(static_cast<std::uint64_t>(archive.size()));
    return body;
}
