#pragma once

#include <memory>
#include <thread>

void consume_daemon_thread(std::unique_ptr<std::thread>* thread);
void join_daemon_thread(std::unique_ptr<std::thread>* thread);
