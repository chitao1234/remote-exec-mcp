#include "transfer/transfer_archive_io.h"

TransferArchiveReader::~TransferArchiveReader() {
}

TransferArchiveSink::~TransferArchiveSink() {
}

void TransferArchiveSink::write_string(const std::string& data) {
    write(data.data(), data.size());
}
