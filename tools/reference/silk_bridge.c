// SPDX-License-Identifier: MIT
// Test-only ABI glue. No codec implementation is included here.
#include <stdint.h>
extern void silk_decode_pulses(void *state, int16_t *pulses, int signal_type,
                              int quantization_offset, int frame_length);
void oracle_silk_decode_pulses(void *state, int16_t *pulses, int signal_type,
                              int quantization_offset, int frame_length) {
    silk_decode_pulses(state, pulses, signal_type, quantization_offset, frame_length);
}
