#pragma once

// Private transport helper shared between the common transport code and the
// platform-specific socket backends.
bool peer_disconnected_send_error(int error);
