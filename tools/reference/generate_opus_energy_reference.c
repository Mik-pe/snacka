// SPDX-License-Identifier: MIT
// Independent driver for the unchanged RFC 6716 Appendix A decoder.
// Compile this file with the archive celt/{quant_bands,entcode,entdec,entenc,laplace}.c,
// include celt/ and include/, and use -O2 -ffp-contract=off -DVAR_ARRAYS -lm.
// The emitted digest is the golden value in src/opus/energy.rs.
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include "quant_bands.h"
#include "entdec.h"
static uint64_t hash = UINT64_C(14695981039346656037);
static void word(uint32_t value) {
  for (unsigned k = 0; k < 4; ++k) { hash ^= (uint8_t)(value >> (k * 8)); hash *= UINT64_C(1099511628211); }
}
static void sample(const float *energy, ec_dec *decoder) {
  for (int i = 0; i < 42; ++i) { uint32_t bits; memcpy(&bits, &energy[i], 4); word(bits); }
  word(ec_tell(decoder)); word(decoder->rng);
}
int main(void) {
  CELTMode mode_storage = {0}; mode_storage.nbEBands = 21; CELTMode *mode = &mode_storage;
  unsigned char packet[256];
  const int sizes[] = {1,2,3,4,8,16,128,256};
  for (int size_index = 0; size_index < 8; ++size_index) {
    int size = sizes[size_index];
    for (int channels = 1; channels <= 2; ++channels) {
      for (int lm = 0; lm < 4; ++lm) {
        float energy[42] = {0};
        for (int frame = 0; frame < 12; ++frame) {
          for (int byte = 0; byte < size; ++byte) packet[byte] = (unsigned char)(byte*73+frame*41+lm*29+channels*17);
          ec_dec decoder; ec_dec_init(&decoder, packet, size);
          int intra = ec_tell(&decoder) + 3 <= size*8 ? ec_dec_bit_logp(&decoder, 3) : 0;
          unquant_coarse_energy(mode, 0, 21, energy, intra, &decoder, channels, lm);
          sample(energy, &decoder);
          if (size >= 128) {
            int fine[21], priority[21];
            for (int band = 0; band < 21; ++band) { fine[band] = band%5; priority[band] = band%2; }
            unquant_fine_energy(mode, 0, 21, energy, fine, &decoder, channels);
            sample(energy, &decoder);
            unquant_energy_finalise(mode, 0, 21, energy, fine, priority, 19, &decoder, channels);
            sample(energy, &decoder);
          }
        }
      }
    }
  }
  printf("%016llx\n", (unsigned long long)hash);
}
