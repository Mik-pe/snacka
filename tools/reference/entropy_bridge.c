// SPDX-License-Identifier: MIT
#include <stdint.h>
#include <string.h>
extern void ec_enc_init(void *state, unsigned char *data, uint32_t size);
void oracle_ec_enc_init(void *state, unsigned char *data, uint32_t size) { ec_enc_init(state, data, size); }
extern void ec_encode(void *state, unsigned low, unsigned high, unsigned total);
void oracle_ec_encode(void *state, unsigned low, unsigned high, unsigned total) { ec_encode(state, low, high, total); }
extern void ec_enc_bit_logp(void *state, int bit, unsigned logp);
void oracle_ec_enc_bit_logp(void *state, int bit, unsigned logp) { ec_enc_bit_logp(state, bit, logp); }
extern void ec_enc_uint(void *state, uint32_t value, uint32_t total);
void oracle_ec_enc_uint(void *state, uint32_t value, uint32_t total) { ec_enc_uint(state, value, total); }
extern void ec_enc_bits(void *state, uint32_t value, unsigned bits);
void oracle_ec_enc_bits(void *state, uint32_t value, unsigned bits) { ec_enc_bits(state, value, bits); }
extern void ec_enc_done(void *state);
void oracle_ec_enc_done(void *state) { ec_enc_done(state); }
extern void ec_enc_shrink(void *state, uint32_t size);
void oracle_ec_enc_shrink(void *state, uint32_t size) { ec_enc_shrink(state, size); }
extern void ec_dec_init(void *state, unsigned char *data, uint32_t size);
void oracle_ec_dec_init(void *state, unsigned char *data, uint32_t size) { ec_dec_init(state, data, size); }
extern unsigned ec_decode(void *state, unsigned total);
unsigned oracle_ec_decode(void *state, unsigned total) { return ec_decode(state, total); }
extern void ec_dec_update(void *state, unsigned low, unsigned high, unsigned total);
void oracle_ec_dec_update(void *state, unsigned low, unsigned high, unsigned total) { ec_dec_update(state, low, high, total); }
extern int ec_dec_bit_logp(void *state, unsigned logp);
int oracle_ec_dec_bit_logp(void *state, unsigned logp) { return ec_dec_bit_logp(state, logp); }
extern uint32_t ec_dec_uint(void *state, uint32_t total);
uint32_t oracle_ec_dec_uint(void *state, uint32_t total) { return ec_dec_uint(state, total); }
extern uint32_t ec_dec_bits(void *state, unsigned bits);
uint32_t oracle_ec_dec_bits(void *state, unsigned bits) { return ec_dec_bits(state, bits); }
extern uint32_t ec_tell_frac(void *state);
uint32_t oracle_ec_tell_frac(void *state) { return ec_tell_frac(state); }
typedef struct { unsigned char *data; uint32_t capacity, tail, reservoir; int reservoir_bits, total; uint32_t position, range, value, auxiliary; int pending, error; } OracleLayout;
uint32_t oracle_range(void *state) { OracleLayout snapshot; memcpy(&snapshot, state, sizeof(snapshot)); return snapshot.range; }
uint32_t oracle_tell(void *state) { OracleLayout snapshot; memcpy(&snapshot, state, sizeof(snapshot)); return snapshot.total-(32-__builtin_clz(snapshot.range)); }
int oracle_error(void *state) { OracleLayout snapshot; memcpy(&snapshot, state, sizeof(snapshot)); return snapshot.error; }
unsigned oracle_size(void) { return sizeof(OracleLayout); }
extern const char *opus_get_version_string(void);
const char *oracle_version(void) { return opus_get_version_string(); }
