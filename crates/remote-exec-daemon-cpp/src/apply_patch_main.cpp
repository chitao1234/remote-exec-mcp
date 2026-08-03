#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>

#include "patch/patch_engine.h"

namespace {

const char* const HELP =
    "Apply a Codex-style patch read from standard input.\n"
    "\n"
    "Usage: apply_patch [OPTIONS]\n"
    "\n"
    "Options:\n"
    "  -h, --help              Print this help text\n"
    "      --help-file <PATH>  With --help, print help text from PATH instead\n";

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

int run(int argc, char* argv[]) {
    if (argc == 2 && is_help_argument(argv[1])) {
        std::cout << HELP;
        return std::cout ? 0 : 1;
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
    std::cout << result.output;
    return std::cout ? 0 : 1;
}

} // namespace

int main(int argc, char* argv[]) {
    try {
        return run(argc, argv);
    } catch (const std::exception& error) {
        std::cerr << "apply_patch: " << error.what() << "\n";
        return 1;
    }
}
