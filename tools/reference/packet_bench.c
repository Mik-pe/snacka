// SPDX-License-Identifier: MIT
// Optional libopus timing oracle. Nothing here is linked into Snacka.
#define _POSIX_C_SOURCE 200809L
#include <dlfcn.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

typedef int (*PacketParse)(const unsigned char *, int32_t, unsigned char *,
                          const unsigned char *[48], int16_t[48], int *);
typedef struct { unsigned char *bytes; int32_t length; } Packet;

static void fail(const char *message) {
    fprintf(stderr, "%s\n", message);
    exit(1);
}

int main(int argc, char **argv) {
    if (argc != 4) fail("usage: packet_bench LIBOPUS INPUT REPETITIONS");
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!library) fail(dlerror());
    PacketParse parse = (PacketParse)dlsym(library, "opus_packet_parse");
    if (!parse) fail(dlerror());
    char *end = NULL;
    unsigned long repetitions = strtoul(argv[3], &end, 10);
    if (!*argv[3] || *end || !repetitions) fail("invalid repetition count");
    FILE *input = fopen(argv[2], "rb");
    if (!input) fail("cannot open packet records");
    Packet *packets = NULL;
    size_t count = 0;
    for (;;) {
        unsigned char encoded[4];
        size_t read = fread(encoded, 1, 4, input);
        if (!read && feof(input)) break;
        if (read != 4) fail("truncated packet length");
        uint32_t length = (uint32_t)encoded[0] | (uint32_t)encoded[1] << 8 |
                          (uint32_t)encoded[2] << 16 | (uint32_t)encoded[3] << 24;
        if (length > INT32_MAX) fail("packet length exceeds libopus API");
        Packet *next = realloc(packets, (count + 1) * sizeof(*packets));
        if (!next) fail("packet table allocation failed");
        packets = next;
        packets[count].length = (int32_t)length;
        packets[count].bytes = malloc(length ? length : 1);
        if (!packets[count].bytes) fail("packet allocation failed");
        if (fread(packets[count].bytes, 1, length, input) != length) fail("truncated packet data");
        ++count;
    }
    fclose(input);
    unsigned char toc;
    const unsigned char *frames[48];
    int16_t sizes[48];
    int offset;
    uint64_t checksum = 0;
    struct timespec start, finish;
    if (clock_gettime(CLOCK_MONOTONIC, &start)) fail("cannot read monotonic clock");
    for (unsigned long iteration = 0; iteration < repetitions; ++iteration) {
        for (size_t packet = 0; packet < count; ++packet) {
            int frames_count = parse(packets[packet].bytes, packets[packet].length,
                                     &toc, frames, sizes, &offset);
            if (frames_count < 0) {
                checksum += 1;
            } else {
                checksum += (unsigned)frames_count;
                for (int frame = 0; frame < frames_count; ++frame) {
                    checksum += (uint16_t)sizes[frame];
                }
            }
        }
    }
    if (clock_gettime(CLOCK_MONOTONIC, &finish)) fail("cannot read monotonic clock");
    double elapsed = (double)(finish.tv_sec - start.tv_sec) +
                     (double)(finish.tv_nsec - start.tv_nsec) * 1e-9;
    printf("%.9f %llu\n", elapsed, (unsigned long long)checksum);
    for (size_t packet = 0; packet < count; ++packet) free(packets[packet].bytes);
    free(packets);
    dlclose(library);
    return 0;
}
