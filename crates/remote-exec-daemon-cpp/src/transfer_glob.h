#pragma once

#include <string>
#include <vector>

namespace transfer_glob {

class Matcher {
public:
    Matcher();
    explicit Matcher(const std::vector<std::string>& patterns);

    bool is_excluded_path(const std::string& relative_path) const;
    bool is_excluded_directory(const std::string& relative_path) const;

private:
    std::vector<std::string> patterns_;
};

}  // namespace transfer_glob
