#include <set>
#include <stdexcept>
#include <string>
#include <vector>

#ifndef _WIN32
#include <unistd.h>
#endif

#include "transfer_ops_internal.h"

namespace {

using namespace transfer_ops_internal;

class StringTransferArchiveSink : public TransferArchiveSink {
public:
    explicit StringTransferArchiveSink(std::string* output) : output_(output) {}

    void write(const char* data, std::size_t size) {
        output_->append(data, size);
    }

private:
    std::string* output_;
};

struct ExportContext {
    ExportOptions options;
    std::vector<TransferWarning> warnings;
    std::set<std::string> followed_directories;
};

void add_warning(
    std::vector<TransferWarning>* warnings,
    const std::string& code,
    const std::string& message
) {
    warnings->push_back(TransferWarning{code, message});
}

void handle_unsupported_entry(ExportContext* context, const std::string& path) {
    add_warning(
        &context->warnings,
        "transfer_skipped_unsupported_entry",
        "Skipped unsupported transfer source entry `" + path + "`."
    );
}

void handle_skipped_symlink(ExportContext* context, const std::string& path) {
    add_warning(
        &context->warnings,
        "transfer_skipped_symlink",
        "Skipped symlink transfer source entry `" + path + "`."
    );
}

void append_directory_contents(
    TransferArchiveSink* archive,
    const std::string& current_path,
    const std::string& current_rel,
    ExportContext* context
) {
    const std::vector<DirectoryEntry> entries = list_directory_entries(current_path);
    for (std::size_t i = 0; i < entries.size(); ++i) {
        const DirectoryEntry& entry = entries[i];
        const std::string child_path = join_path(current_path, entry.name);
        const std::string child_rel =
            current_rel.empty() ? entry.name : current_rel + "/" + entry.name;

        if (entry.is_directory) {
            append_directory_entry(archive, child_rel);
            append_directory_contents(archive, child_path, child_rel, context);
            continue;
        }
        if (entry.is_symlink) {
            if (context->options.symlink_mode == "reject") {
                throw std::runtime_error("transfer source contains unsupported symlink " + child_path);
            }
            if (context->options.symlink_mode == "skip") {
                handle_skipped_symlink(context, child_path);
                continue;
            }
#ifdef _WIN32
            throw std::runtime_error("transfer source contains unsupported symlink " + child_path);
#else
            if (context->options.symlink_mode == "preserve") {
                char target_buffer[4096];
                const ssize_t target_len =
                    readlink(child_path.c_str(), target_buffer, sizeof(target_buffer) - 1);
                if (target_len < 0) {
                    throw std::runtime_error("unable to read symlink " + child_path);
                }
                target_buffer[target_len] = '\0';
                append_symlink_entry(archive, child_rel, std::string(target_buffer));
                continue;
            }
            if (context->options.symlink_mode == "follow") {
                if (is_directory_follow(child_path)) {
                    if (context->followed_directories.count(child_path) != 0) {
                        handle_skipped_symlink(context, child_path);
                        continue;
                    }
                    context->followed_directories.insert(child_path);
                    append_directory_entry(archive, child_rel);
                    append_directory_contents(archive, child_path, child_rel, context);
                    continue;
                }
                if (is_regular_file_follow(child_path)) {
                    append_file_entry_from_path(archive, child_rel, child_path);
                    continue;
                }
                handle_unsupported_entry(context, child_path);
                continue;
            }
            throw std::runtime_error("transfer source contains unsupported symlink " + child_path);
#endif
        }
        if (!entry.is_regular_file) {
            handle_unsupported_entry(context, child_path);
            continue;
        }

        append_file_entry_from_path(archive, child_rel, child_path);
    }
}

void reject_directory_symlinks(const std::string& current_path) {
    const std::vector<DirectoryEntry> entries = list_directory_entries(current_path);
    for (std::size_t i = 0; i < entries.size(); ++i) {
        const DirectoryEntry& entry = entries[i];
        const std::string child_path = join_path(current_path, entry.name);
        if (entry.is_symlink) {
            throw std::runtime_error("transfer source contains unsupported symlink " + child_path);
        }
        if (entry.is_directory) {
            reject_directory_symlinks(child_path);
        }
    }
}

void preflight_export_path(
    const std::string& absolute_path,
    const std::string& source_type,
    const ExportOptions& options
) {
    if (source_type == "directory" && options.symlink_mode == "reject") {
        reject_directory_symlinks(absolute_path);
    }
}

void export_directory_as_tar(
    TransferArchiveSink* archive,
    const std::string& absolute_path,
    ExportContext* context
) {
    append_directory_entry(archive, ".");
    append_directory_contents(archive, absolute_path, "", context);
    append_transfer_summary_entry(archive, context->warnings);
    append_archive_terminator(archive);
}

void export_file_as_tar(
    TransferArchiveSink* archive,
    const std::string& absolute_path,
    const ExportOptions& options
) {
#ifdef _WIN32
    (void)options;
    append_file_entry_from_path(archive, SINGLE_FILE_ENTRY, absolute_path);
#else
    if (is_symlink_path(absolute_path)) {
        if (options.symlink_mode == "preserve") {
            char target_buffer[4096];
            const ssize_t target_len =
                readlink(absolute_path.c_str(), target_buffer, sizeof(target_buffer) - 1);
            if (target_len < 0) {
                throw std::runtime_error("unable to read symlink " + absolute_path);
            }
            target_buffer[target_len] = '\0';
            append_symlink_entry(archive, SINGLE_FILE_ENTRY, std::string(target_buffer));
        } else if (options.symlink_mode == "follow") {
            append_file_entry_from_path(archive, SINGLE_FILE_ENTRY, absolute_path);
        } else {
            throw std::runtime_error("transfer source contains unsupported symlink " + absolute_path);
        }
    } else {
        append_file_entry_from_path(archive, SINGLE_FILE_ENTRY, absolute_path);
    }
#endif
    append_archive_terminator(archive);
}

ExportOptions normalized_options(const std::string& symlink_mode) {
    ExportOptions options{symlink_mode.empty() ? "preserve" : symlink_mode};
    validate_transfer_options(options);
    return options;
}

void validate_export_path(const std::string& absolute_path, const ExportOptions& options) {
    if (!is_absolute_path(absolute_path)) {
        throw std::runtime_error("transfer path is not absolute");
    }
    if (!path_exists(absolute_path)) {
        throw std::runtime_error("transfer source missing");
    }
    if (is_symlink_path(absolute_path)) {
#ifdef _WIN32
        (void)options;
        throw std::runtime_error("transfer source contains unsupported symlink " + absolute_path);
#else
        if (options.symlink_mode == "reject" || options.symlink_mode == "skip") {
            throw std::runtime_error("transfer source contains unsupported symlink " + absolute_path);
        }
#endif
    }
}

}  // namespace

std::string export_path_source_type(
    const std::string& absolute_path,
    const std::string& symlink_mode
) {
    const ExportOptions options = normalized_options(symlink_mode);
    validate_export_path(absolute_path, options);
    if (is_symlink_path(absolute_path) && options.symlink_mode == "preserve") {
        return "file";
    }
    if (is_regular_file(absolute_path) ||
        (is_symlink_path(absolute_path) && options.symlink_mode == "follow" &&
         is_regular_file_follow(absolute_path))) {
        return "file";
    }
    if (is_directory(absolute_path) ||
        (is_symlink_path(absolute_path) && options.symlink_mode == "follow" && is_directory_follow(absolute_path))) {
        preflight_export_path(absolute_path, "directory", options);
        return "directory";
    }
    throw std::runtime_error("transfer source must be a regular file or directory");
}

void export_path_to_sink_as(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    const std::string& source_type,
    const std::string& symlink_mode
) {
    ExportContext context;
    context.options = normalized_options(symlink_mode);
    validate_export_path(absolute_path, context.options);
    preflight_export_path(absolute_path, source_type, context.options);

    if (source_type == "file") {
        export_file_as_tar(&sink, absolute_path, context.options);
        return;
    }
    if (source_type == "directory") {
        export_directory_as_tar(&sink, absolute_path, &context);
        return;
    }
    throw std::runtime_error("unsupported transfer source type");
}

std::string export_path_to_sink(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    const std::string& symlink_mode
) {
    const std::string source_type = export_path_source_type(absolute_path, symlink_mode);
    export_path_to_sink_as(sink, absolute_path, source_type, symlink_mode);
    return source_type;
}

ExportedPayload export_path(
    const std::string& absolute_path,
    const std::string& symlink_mode
) {
    std::string archive;
    StringTransferArchiveSink sink(&archive);
    const std::string source_type = export_path_to_sink(sink, absolute_path, symlink_mode);
    return ExportedPayload{source_type, archive};
}
