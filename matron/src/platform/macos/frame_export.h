#pragma once

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

// Main-thread only. The listener owns the socket path. Each message is a
// complete, tightly packed RGB888 frame, so a slow or absent reader drops
// frames without delaying Verdandi.
class FrameExport {
public:
    FrameExport() {
        const char *path = std::getenv("VERDANDI_FRAME_SOCKET");
        if (!path || !*path) {
            return;
        }
        const size_t length = std::strlen(path);
        if (*path != '/' || length >= sizeof(address.sun_path)) {
            std::fprintf(stderr, "frame export: VERDANDI_FRAME_SOCKET must be an absolute Unix socket path under %zu bytes\n",
                         sizeof(address.sun_path));
            return;
        }
        address.sun_family = AF_UNIX;
        std::memcpy(address.sun_path, path, length + 1);
        address.sun_len = SUN_LEN(&address);
        fd = socket(AF_UNIX, SOCK_DGRAM, 0);
        // Darwin's default buffer is smaller than one OLED frame.
        const int buffer_size = 32768;
        if (fd == -1 || fcntl(fd, F_SETFD, FD_CLOEXEC) == -1 ||
            fcntl(fd, F_SETFL, O_NONBLOCK) == -1 ||
            setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &buffer_size, sizeof(buffer_size)) == -1) {
            std::perror("frame export: cannot initialise Unix socket");
            if (fd != -1) {
                close(fd);
                fd = -1;
            }
        }
    }

    ~FrameExport() {
        if (enabled()) {
            close(fd);
        }
    }

    FrameExport(const FrameExport &) = delete;
    FrameExport &operator=(const FrameExport &) = delete;

    bool enabled() const { return fd != -1; }

    bool repeat_due() const { return std::chrono::steady_clock::now() >= next_repeat; }

    void publish(const unsigned char *rgb24, int stride) {
        using namespace std::chrono;
        next_repeat = steady_clock::now() + milliseconds(500);

        // Cairo RGB24 stores each pixel as a native-endian 0x00RRGGBB word.
        unsigned char packet[8 + 128 * 64 * 3] = {'V', 'D', 'F', '1', 128, 0, 64, 0};
        auto *dst = packet + 8;
        for (int y = 0; y < 64; ++y) {
            const auto *row = rgb24 + y * stride;
            for (int x = 0; x < 128; ++x) {
                uint32_t pixel;
                std::memcpy(&pixel, row + x * 4, sizeof(pixel));
                *dst++ = static_cast<unsigned char>(pixel >> 16);
                *dst++ = static_cast<unsigned char>(pixel >> 8);
                *dst++ = static_cast<unsigned char>(pixel);
            }
        }
        // A later draw or heartbeat retries after socket absence/backpressure.
        sendto(fd, packet, sizeof(packet), 0, reinterpret_cast<const sockaddr *>(&address), address.sun_len);
    }

private:
    int fd = -1;
    sockaddr_un address{};
    std::chrono::steady_clock::time_point next_repeat{};
};
