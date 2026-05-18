#pragma once

#include <string>

struct ImageReadResult {
    std::string mime_type;
    std::string bytes;
};

ImageReadResult read_image_original(const std::string& path);
