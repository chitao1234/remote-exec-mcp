#include <cstdlib>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace {

struct TestCase {
    std::string name;
    std::string command;
};

bool set_bundle_working_directory(const char* argv0) {
#ifdef _WIN32
    (void)argv0;
    char path[MAX_PATH + 1];
    const DWORD length = GetModuleFileNameA(NULL, path, MAX_PATH);
    if (length == 0 || length >= MAX_PATH) {
        return false;
    }
    path[length] = '\0';

    std::string executable_path(path);
    const std::string::size_type separator = executable_path.find_last_of("/\\");
    if (separator == std::string::npos) {
        return true;
    }
    const std::string directory = executable_path.substr(0, separator);
    return SetCurrentDirectoryA(directory.c_str()) != 0;
#else
    const std::string executable_path(argv0 == NULL ? "" : argv0);
    const std::string::size_type separator = executable_path.find_last_of('/');
    if (separator == std::string::npos) {
        return true;
    }
    const std::string directory =
        separator == 0 ? std::string("/") : executable_path.substr(0, separator);
    return chdir(directory.c_str()) == 0;
#endif
}

bool configure_test_logging() {
    const char* configured = std::getenv("REMOTE_EXEC_LOG");
    if (configured != NULL && configured[0] != '\0') {
        return true;
    }

    const char* fallback = std::getenv("REMOTE_EXEC_TEST_LOG");
    const char* value = fallback != NULL && fallback[0] != '\0' ? fallback : "off";
#ifdef _WIN32
    return SetEnvironmentVariableA("REMOTE_EXEC_LOG", value) != 0;
#else
    return setenv("REMOTE_EXEC_LOG", value, 1) == 0;
#endif
}

std::string trim_left(const std::string& value) {
    const std::string::size_type first = value.find_first_not_of(" \t\r");
    return first == std::string::npos ? std::string() : value.substr(first);
}

std::string format_line_number(unsigned long line_number) {
    std::ostringstream output;
    output << line_number;
    return output.str();
}

bool load_manifest(std::vector<TestCase>& tests, std::string& error) {
    std::ifstream input("tests.txt");
    if (!input) {
        error = "unable to open tests.txt";
        return false;
    }

    std::string line;
    unsigned long line_number = 0;
    while (std::getline(input, line)) {
        ++line_number;
        if (!line.empty() && line[line.size() - 1] == '\r') {
            line.erase(line.size() - 1);
        }
        const std::string trimmed = trim_left(line);
        if (trimmed.empty() || trimmed[0] == '#') {
            continue;
        }

        const std::string::size_type separator = trimmed.find_first_of(" \t");
        if (separator == std::string::npos) {
            error = "tests.txt line " + format_line_number(line_number) + " is missing a command";
            return false;
        }

        TestCase test;
        test.name = trimmed.substr(0, separator);
        test.command = trim_left(trimmed.substr(separator));
        if (test.command.empty()) {
            error = "tests.txt line " + format_line_number(line_number) + " is missing a command";
            return false;
        }
        tests.push_back(test);
    }

    if (!input.eof()) {
        error = "unable to read tests.txt";
        return false;
    }
    if (tests.empty()) {
        error = "tests.txt does not contain any tests";
        return false;
    }
    return true;
}

const TestCase* find_test(const std::vector<TestCase>& tests, const std::string& name) {
    for (std::vector<TestCase>::const_iterator it = tests.begin(); it != tests.end(); ++it) {
        if (it->name == name) {
            return &*it;
        }
    }
    return NULL;
}

void print_usage(const char* program) {
    std::cout << "Usage: " << program << " [--list] [TEST ...]\n"
              << "Run every bundled test, or only the named tests.\n";
}

} // namespace

int main(int argc, char** argv) {
    if (!set_bundle_working_directory(argc > 0 ? argv[0] : NULL)) {
        std::cerr << "run-tests: unable to use the test bundle directory\n";
        return 2;
    }

    if (argc == 2 && std::string(argv[1]) == "--help") {
        print_usage(argv[0]);
        return 0;
    }

    if (!configure_test_logging()) {
        std::cerr << "run-tests: unable to configure test logging\n";
        return 2;
    }

    std::vector<TestCase> tests;
    std::string manifest_error;
    if (!load_manifest(tests, manifest_error)) {
        std::cerr << "run-tests: " << manifest_error << '\n';
        return 2;
    }

    if (argc == 2 && std::string(argv[1]) == "--list") {
        for (std::vector<TestCase>::const_iterator it = tests.begin(); it != tests.end(); ++it) {
            std::cout << it->name << '\n';
        }
        return 0;
    }

    std::vector<const TestCase*> selected;
    if (argc <= 1) {
        for (std::vector<TestCase>::const_iterator it = tests.begin(); it != tests.end(); ++it) {
            selected.push_back(&*it);
        }
    } else {
        for (int index = 1; index < argc; ++index) {
            const std::string name(argv[index]);
            if (name == "--help" || name == "--list") {
                std::cerr << "run-tests: " << name << " cannot be combined with test names\n";
                return 2;
            }
            const TestCase* test = find_test(tests, name);
            if (test == NULL) {
                std::cerr << "run-tests: unknown test '" << name << "'\n";
                return 2;
            }
            selected.push_back(test);
        }
    }

    for (std::vector<const TestCase*>::const_iterator it = selected.begin(); it != selected.end();
         ++it) {
        const TestCase& test = **it;
        std::cout << "[ RUN      ] " << test.name << std::endl;
        const int status = std::system(test.command.c_str());
        if (status != 0) {
            std::cerr << "[  FAILED  ] " << test.name << " (status " << status << ")\n";
            return 1;
        }
        std::cout << "[       OK ] " << test.name << std::endl;
    }

    std::cout << "[  PASSED  ] " << selected.size() << " test(s)\n";
    return 0;
}
