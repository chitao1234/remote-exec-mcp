#pragma once

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <memory>
#include <thread>
#endif

#ifdef _WIN32
void consume_port_tunnel_thread(HANDLE* thread, DWORD thread_id);
#else
void consume_port_tunnel_thread(std::unique_ptr<std::thread>* thread);
#endif
