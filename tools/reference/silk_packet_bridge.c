// SPDX-License-Identifier: MIT
// Optional fixture-only glue for the installed libopus public API.
#include <stdint.h>
extern void *opus_encoder_create(int rate, int channels, int application, int *error);
extern void opus_encoder_destroy(void *state);
extern int opus_encoder_ctl(void *state, int request, ...);
extern int opus_encode_float(void *state, const float *samples, int frame_size,
                             unsigned char *packet, int max_packet);
extern int opus_packet_parse(const unsigned char *packet, int length, unsigned char *toc,
                             const unsigned char *frames[48], int16_t sizes[48], int *offset);
extern void *opus_decoder_create(int rate, int channels, int *error);
extern void opus_decoder_destroy(void *state);
extern int opus_decoder_ctl(void *state, int request, ...);
extern int opus_decode(void *state, const unsigned char *packet, int length,
                       int16_t *output, int frame_size, int decode_fec);
void *oracle_opus_encoder_create(int rate, int channels, int application, int *error) {
    return opus_encoder_create(rate, channels, application, error);
}
void oracle_opus_encoder_destroy(void *state) { opus_encoder_destroy(state); }
int oracle_opus_encoder_ctl(void *state, int request, int value) {
    return opus_encoder_ctl(state, request, value);
}
int oracle_opus_encode_float(void *state, const float *samples, int frame_size,
                             unsigned char *packet, int max_packet) {
    return opus_encode_float(state, samples, frame_size, packet, max_packet);
}
int oracle_opus_packet_parse(const unsigned char *packet, int length, unsigned char *toc,
                             const unsigned char *frames[48], int16_t sizes[48], int *offset) {
    return opus_packet_parse(packet, length, toc, frames, sizes, offset);
}
void *oracle_opus_decoder_create(int rate, int channels, int *error) {
    return opus_decoder_create(rate, channels, error);
}
void oracle_opus_decoder_destroy(void *state) { opus_decoder_destroy(state); }
int oracle_opus_decoder_reset(void *state) { return opus_decoder_ctl(state, 4028); }
int oracle_opus_decoder_range(void *state, uint32_t *range) {
    return opus_decoder_ctl(state, 4031, range);
}
int oracle_opus_decode(void *state, const unsigned char *packet, int length,
                       int16_t *output, int frame_size) {
    return opus_decode(state, packet, length, output, frame_size, 0);
}
