#include <set>
#include <stdexcept>
#include <string>
#include <vector>

#ifndef _WIN32
#include <unistd.h>
#endif

#include "rpc_failures.h"
#include "transfer_ops_internal.h"
#include "transfer_glob.h"

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

#ifndef _WIN32
std::string read_symlink_target(const std::string& path) {
    char target_buffer[4096];
    const ssize_t target_len =
        readlink(path.c_str(), target_buffer, sizeof(target_buffer) - 1);
    if (target_len < 0) {
        throw std::runtime_error("unable to read symlink " + path);
    }
    target_buffer[target_len] = '\0';
    return std::string(target_buffer);
}

void append_preserved_symlink_entry(
    TransferArchiveSink* archive,
    const std::string& source_path,
    const std::string& archive_path
) {
    append_symlink_entry(archive, archive_path, read_symlink_target(source_path));
}
#endif

void append_directory_contents(
    TransferArchiveSink* archive,
    const std::string& current_path,
    const std::string& current_rel,
    const transfer_glob::Matcher& exclude_matcher,
    ExportContext* context
);

bool append_followed_symlink_entry(
    TransferArchiveSink* archive,
    const std::string& child_path,
    const std::string& child_rel,
    const transfer_glob::Matcher& exclude_matcher,
    ExportContext* context
) {
    if (is_directory_follow(child_path)) {
        if (context->followed_directories.count(child_path) != 0) {
            handle_skipped_symlink(context, child_path);
            return true;
        }
        context->followed_directories.insert(child_path);
        append_directory_entry(archive, child_rel);
        append_directory_contents(
            archive,
            child_path,
            child_rel,
            exclude_matcher,
            context
        );
        return true;
    }
    if (is_regular_file_follow(child_path)) {
        append_file_entry_from_path(archive, child_rel, child_path);
        return true;
    }
    return false;
}

void append_directory_contents(
    TransferArchiveSink* archive,
    const std::string& current_path,
    const std::string& current_rel,
    const transfer_glob::Matcher& exclude_matcher,
    ExportContext* context
) {
    const std::vector<DirectoryEntry> entries = list_directory_entries(current_path);
    for (std::size_t i = 0; i < entries.size(); ++i) {
        const DirectoryEntry& entry = entries[i];
        const std::string child_path = join_path(current_path, entry.name);
        const std::string child_rel =
            current_rel.empty() ? entry.name : current_rel + "/" + entry.name;

        if (entry.is_directory) {
            if (exclude_matcher.is_excluded_directory(child_rel)) {
                continue;
            }
        } else if (exclude_matcher.is_excluded_path(child_rel)) {
            continue;
        }

        if (entry.is_directory) {
            append_directory_entry(archive, child_rel);
            append_directory_contents(
                archive,
                child_path,
                child_rel,
                exclude_matcher,
                context
            );
            continue;
        }
        if (entry.is_symlink) {
            if (context->options.symlink_mode == TransferSymlinkMode::Skip) {
                handle_skipped_symlink(context, child_path);
                continue;
            }
#ifdef _WIN32
            if (context->options.symlink_mode == TransferSymlinkMode::Follow &&
                append_followed_symlink_entry(
                    archive,
                    child_path,
                    child_rel,
                    exclude_matcher,
                    context
                )) {
                continue;
            }
            handle_skipped_symlink(context, child_path);
            continue;
#else
            if (context->options.symlink_mode == TransferSymlinkMode::Preserve) {
                append_preserved_symlink_entry(archive, child_path, child_rel);
                continue;
            }
            if (context->options.symlink_mode == TransferSymlinkMode::Follow) {
                if (append_followed_symlink_entry(
                        archive,
                        child_path,
                        child_rel,
                        exclude_matcher,
                        context)) {
                    continue;
                }
                handle_unsupported_entry(context, child_path);
                continue;
            }
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "transfer source contains unsupported symlink " + child_path
            );
#endif
        }
        if (!entry.is_regular_file) {
            handle_unsupported_entry(context, child_path);
            continue;
        }

        append_file_entry_from_path(archive, child_rel, child_path);
    }
}

void export_directory_as_tar(
    TransferArchiveSink* archive,
    const std::string& absolute_path,
    const transfer_glob::Matcher& exclude_matcher,
    ExportContext* context
) {
    append_directory_entry(archive, ".");
    append_directory_contents(archive, absolute_path, "", exclude_matcher, context);
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
        if (options.symlink_mode == TransferSymlinkMode::Preserve) {
            append_preserved_symlink_entry(archive, absolute_path, SINGLE_FILE_ENTRY);
        } else if (options.symlink_mode == TransferSymlinkMode::Follow) {
            append_file_entry_from_path(archive, SINGLE_FILE_ENTRY, absolute_path);
        } else {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "transfer source contains unsupported symlink " + absolute_path
            );
        }
    } else {
        append_file_entry_from_path(archive, SINGLE_FILE_ENTRY, absolute_path);
    }
#endif
    append_archive_terminator(archive);
}

ExportOptions normalized_options(
    TransferSymlinkMode symlink_mode,
    const std::vector<std::string>& exclude
) {
    ExportOptions options;
    options.symlink_mode = symlink_mode;
    options.exclude = exclude;
    validate_transfer_options(options);
    return options;
}

void validate_export_path(const std::string& absolute_path, const ExportOptions& options) {
    if (!is_absolute_path(absolute_path)) {
        throw TransferFailure(
            TransferRpcCode::PathNotAbsolute,
            "transfer path is not absolute"
        );
    }
    if (!path_exists(absolute_path)) {
        throw TransferFailure(
            TransferRpcCode::SourceMissing,
            "transfer source missing"
        );
    }
    if (is_symlink_path(absolute_path)) {
#ifdef _WIN32
        if (options.symlink_mode != TransferSymlinkMode::Follow) {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "transfer source contains unsupported symlink " + absolute_path
            );
        }
#else
        if (options.symlink_mode == TransferSymlinkMode::Skip) {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "transfer source contains unsupported symlink " + absolute_path
            );
        }
#endif
    }
}

}  // namespace

TransferSourceType export_path_source_type(
    const std::string& absolute_path,
    TransferSymlinkMode symlink_mode
) {
    const ExportOptions options = normalized_options(
        symlink_mode,
        std::vector<std::string>()
    );
    validate_export_path(absolute_path, options);
#ifndef _WIN32
    if (is_symlink_path(absolute_path) && options.symlink_mode == TransferSymlinkMode::Preserve) {
        return TransferSourceType::File;
    }
#endif
    if (is_regular_file(absolute_path) ||
        (is_symlink_path(absolute_path) && options.symlink_mode == TransferSymlinkMode::Follow &&
         is_regular_file_follow(absolute_path))) {
        return TransferSourceType::File;
    }
    if (is_directory(absolute_path) ||
        (is_symlink_path(absolute_path) && options.symlink_mode == TransferSymlinkMode::Follow && is_directory_follow(absolute_path))) {
        return TransferSourceType::Directory;
    }
    throw TransferFailure(
        TransferRpcCode::SourceUnsupported,
        "transfer source must be a regular file or directory"
    );
}

void export_path_to_sink_as(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    TransferSourceType source_type,
    TransferSymlinkMode symlink_mode,
    const std::vector<std::string>& exclude
) {
    ExportContext context;
    context.options = normalized_options(symlink_mode, exclude);
    validate_export_path(absolute_path, context.options);
    const transfer_glob::Matcher exclude_matcher(context.options.exclude);

    if (source_type == TransferSourceType::File) {
        export_file_as_tar(&sink, absolute_path, context.options);
        return;
    }
    if (source_type == TransferSourceType::Directory) {
        export_directory_as_tar(&sink, absolute_path, exclude_matcher, &context);
        return;
    }
    if (source_type == TransferSourceType::Multiple) {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "multiple source type is only supported for import"
        );
    }
    throw TransferFailure(
        TransferRpcCode::SourceUnsupported,
        "unsupported transfer source type"
    );
}

TransferSourceType export_path_to_sink(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    TransferSymlinkMode symlink_mode,
    const std::vector<std::string>& exclude
) {
    const TransferSourceType source_type = export_path_source_type(absolute_path, symlink_mode);
    export_path_to_sink_as(sink, absolute_path, source_type, symlink_mode, exclude);
    return source_type;
}

ExportedPayload export_path(
    const std::string& absolute_path,
    TransferSymlinkMode symlink_mode,
    const std::vector<std::string>& exclude
) {
    std::string archive;
    StringTransferArchiveSink sink(&archive);
    const TransferSourceType source_type = export_path_to_sink(
        sink,
        absolute_path,
        symlink_mode,
        exclude
    );
    return ExportedPayload{source_type, archive};
}
