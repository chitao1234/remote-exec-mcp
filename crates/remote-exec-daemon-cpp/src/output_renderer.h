#pragma once

#include <cstddef>
#include <string>

unsigned long approximate_output_token_count(std::size_t bytes);
std::string render_output(const std::string& output, unsigned long max_output_tokens);
