#include <algorithm>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>

#include "patch/patch_engine.h"

#ifdef _WIN32
#include <windows.h>

#include "platform/win32_utf8.h"
#endif

namespace {

const char* const HELP =
    "Apply a Codex-style patch read from standard input.\n"
    "\n"
    "Usage: apply_patch [OPTIONS]\n"
    "\n"
    "Options:\n"
    "  -h, --help              Print this help text\n"
    "      --help-file <PATH>  With --help, print help text from PATH instead\n";

void write_text(std::ostream& stream, const std::string& text, bool stderr_stream) {
#ifdef _WIN32
    const HANDLE handle = GetStdHandle(stderr_stream ? STD_ERROR_HANDLE : STD_OUTPUT_HANDLE);
    DWORD console_mode = 0U;
    if (handle != INVALID_HANDLE_VALUE && handle != nullptr
        && GetConsoleMode(handle, &console_mode)) {
        try {
            const std::wstring wide = win32_utf8::wide_from_utf8(text);
            std::size_t offset = 0U;
            while (offset < wide.size()) {
                const std::size_t remaining = wide.size() - offset;
                const DWORD requested = static_cast<DWORD>(
                    std::min<std::size_t>(remaining, static_cast<std::size_t>(0x7FFFFFFFU))
                );
                DWORD written = 0U;
                if (WriteConsoleW(handle, wide.data() + offset, requested, &written, nullptr)
                    == 0) {
                    break;
                }
                offset += written;
            }
            if (offset == wide.size()) {
                return;
            }
        } catch (const std::exception&) {
        }
    }
#else
    (void)stderr_stream;
#endif
    stream << text;
    stream.flush();
}

void write_stdout(const std::string& text) {
    write_text(std::cout, text, false);
}

void write_stderr(const std::string& text) {
    write_text(std::cerr, text, true);
}

void print_file(const std::string& path) {
    std::ifstream input(path.c_str(), std::ios::in | std::ios::binary);
    if (!input) {
        throw std::runtime_error("unable to read help file `" + path + "`");
    }
    std::cout << input.rdbuf();
    if (!std::cout) {
        throw std::runtime_error("unable to write help text");
    }
}

bool has_help_file_prefix(const std::string& argument) {
    return argument.compare(0U, 12U, "--help-file=") == 0;
}

bool is_help_argument(const std::string& argument) {
    return argument == "--help" || argument == "-h";
}

void print_inline_help_file(const std::string& argument) {
    const std::string path = argument.substr(12U);
    if (path.empty()) {
        throw std::runtime_error("--help-file requires a path");
    }
    print_file(path);
}

void print_partial_success(const std::vector<std::string>& updated_paths) {
    std::string output = "Partial success. Updated the following files:\n";
    for (std::size_t i = 0; i < updated_paths.size(); ++i) {
        output += updated_paths[i] + '\n';
    }
    write_stdout(output);
}

int run(int argc, char* argv[]) {
    if (argc == 2 && is_help_argument(argv[1])) {
        write_stdout(HELP);
        return 0;
    }
    if (argc == 4 && is_help_argument(argv[1]) && std::string(argv[2]) == "--help-file") {
        print_file(argv[3]);
        return 0;
    }
    if (argc == 4 && std::string(argv[1]) == "--help-file" && is_help_argument(argv[3])) {
        print_file(argv[2]);
        return 0;
    }
    if (argc == 3 && is_help_argument(argv[1]) && has_help_file_prefix(argv[2])) {
        print_inline_help_file(argv[2]);
        return 0;
    }
    if (argc == 3 && has_help_file_prefix(argv[1]) && is_help_argument(argv[2])) {
        print_inline_help_file(argv[1]);
        return 0;
    }
    if (argc == 2 && std::string(argv[1]) == "--help-file") {
        throw std::runtime_error("--help-file requires a path");
    }
    const bool ignore_help_file = (argc == 3 && std::string(argv[1]) == "--help-file")
                                  || (argc == 2 && has_help_file_prefix(argv[1]));
    if (argc != 1 && !ignore_help_file) {
        throw std::runtime_error("unexpected argument; run `apply_patch --help` for usage");
    }

    const std::string patch(
        (std::istreambuf_iterator<char>(std::cin)),
        std::istreambuf_iterator<char>()
    );
    const PatchApplyResult result = apply_patch(".", patch);
    write_stdout(result.output);
    return 0;
}

} // namespace

int main(int argc, char* argv[]) {
    try {
        return run(argc, argv);
    } catch (const PatchApplyPartialError& error) {
        print_partial_success(error.updated_paths());
        write_stderr(std::string("apply_patch: ") + error.what() + "\n");
        return 1;
    } catch (const std::exception& error) {
        write_stderr(std::string("apply_patch: ") + error.what() + "\n");
        return 1;
    }
}
