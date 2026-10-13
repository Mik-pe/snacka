# SPDX-License-Identifier: MIT
"""Check Snacka CELT energy/allocation against an independently installed libopus.

Pass the libopus 1.6.1 static archive and its matching source directory. Only
headers and external function calls are compiled; reference algorithms are not
read, translated, linked into Snacka, or required by ordinary cargo builds.
The temporary Rust executable compiles only this repository's own sources.

Energy histories are compared to 1e-5 relative tolerance because an optimized C
build may contract floating-point operations into FMA. All arithmetic coder
states and all integer allocation results must agree exactly. Failure reports
include the deterministic geometry, packet size, frame or seed and array index.
"""

import argparse
from collections import Counter
import ctypes
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

BRIDGE = r'''
#include <stdint.h>
#include "quant_bands.h"
#include "rate.h"
#include "opus_custom.h"
void *kernel_mode(int *error) { return opus_custom_mode_create(48000,960,error); }
void kernel_coarse(const CELTMode*m,int start,int end,float*energy,int intra,ec_dec*state,int channels,int lm){ unquant_coarse_energy(m,start,end,energy,intra,state,channels,lm); }
void kernel_fine(const CELTMode*m,int start,int end,float*energy,int*fine,ec_dec*state,int channels){ int base[21]={0};unquant_fine_energy(m,start,end,energy,base,fine,state,channels); }
void kernel_final(const CELTMode*m,int start,int end,float*energy,int*fine,int*priorities,int left,ec_dec*state,int channels){unquant_energy_finalise(m,start,end,energy,fine,priorities,left,state,channels);}
int kernel_allocation(const CELTMode*m,int start,int end,int*offsets,int trim,int*intensity,int*dual,int total,int*balance,int*shape,int*fine,int*priorities,int channels,int lm,ec_ctx*state){int caps[21];init_caps(m,caps,lm,channels);return clt_compute_allocation(m,start,end,offsets,caps,trim,intensity,dual,total,balance,shape,fine,priorities,channels,lm,state,0,0,0);}

unsigned kernel_state_size(void) { return sizeof(ec_ctx); }
void kernel_decoder(ec_dec *state, unsigned char *data, unsigned size) { ec_dec_init(state,data,size); }
int kernel_bit(ec_dec *state, unsigned logp) { return ec_dec_bit_logp(state,logp); }
unsigned kernel_tell(ec_ctx *state) { return ec_tell(state); }
unsigned kernel_frac(ec_ctx *state) { return ec_tell_frac(state); }
unsigned kernel_range(const ec_ctx *state) { return state->rng; }
'''

RUST_DRIVER = r'''
#![forbid(unsafe_code)]
#![allow(dead_code)]
#[path="@REPO@/src/entropy.rs"] mod entropy;
mod opus {
 #[path="@REPO@/src/opus/range.rs"] pub mod range;
 #[path="@REPO@/src/opus/energy.rs"] pub mod energy;
 #[path="@REPO@/src/opus/allocation.rs"] pub mod allocation;
}
use opus::{range,energy,allocation};
fn csv<T:ToString>(values: impl IntoIterator<Item=T>)->String { values.into_iter().map(|v|v.to_string()).collect::<Vec<_>>().join(",") }
fn state(s:&energy::EnergyState,d:&range::Decoder<'_>)->String {format!("{}|{}|{}|{}",d.range(),d.tell(),d.tell_frac(),csv(s.energies.iter().flat_map(|r|r.iter()).map(|v|v.to_bits())))}
fn main() {
 for size in [1,2,3,4,8,16,32,64,128,256] {
  for ch in 1..=2 {
   for lm in 0..4 {
    for (start,end) in [(0,21),(0,17),(0,13),(0,8),(17,21)] {
     let mut energies=energy::EnergyState::new();
     for frame in 0..12 {
      let p:Vec<_>=(0..size).map(|i|(i*73+frame*41+lm*29+ch*17) as u8).collect();
      let mut d=range::Decoder::new(&p);
      let c=energy::FrameConfig::new(start,end,ch,lm,(size*8) as u32).unwrap();
      let intra=energy::decode_intra(&mut d,c).unwrap();
      energies.decode_coarse(&mut d,c,intra).unwrap();
      println!("E|{size}|{ch}|{lm}|{start}|{end}|{frame}|coarse|{}",state(&energies,&d));
      if size>=128 {
       let fine=std::array::from_fn(|i|(i%5) as u8);
       let priority=std::array::from_fn(|i|(i%2) as u8);
       energies.decode_fine(&mut d,c,&fine).unwrap();
       println!("E|{size}|{ch}|{lm}|{start}|{end}|{frame}|fine|{}",state(&energies,&d));
       energies.decode_final(&mut d,c,&fine,&priority,19).unwrap();
       println!("E|{size}|{ch}|{lm}|{start}|{end}|{frame}|final|{}",state(&energies,&d));
      }
     }
    }
   }
  }
 }
 for lm in 0..4 {
  for ch in 1..=2 {
   for (start,end) in [(0,21),(0,17),(0,13),(0,8),(17,21)] {
    for total in [0,1,7,8,15,16,32,64,128,256,511,512,1024,2048,4096,16384,32768,65536] {
     for trim in [0,5,10] {
      for seed in 0..4 {
       let p:Vec<_>=(0..128).map(|i|(i*73+seed*41+lm*29+ch*17) as u8).collect();
       let mut d=range::Decoder::new(&p);
       let c=allocation::AllocationConfig::new(start,end,lm as u8,ch).unwrap();
       let boost=std::array::from_fn(|i| if seed%2==1 && (start..end).contains(&i) {(i%3*8) as i32} else {0});
       let info=allocation::AllocationSideInfo{boost_eighth_bits:boost,trim};
       match allocation::decode_allocation(c,&info,total,&mut d) {
        Ok(a)=>println!("A|{lm}|{ch}|{start}|{end}|{total}|{trim}|{seed}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",d.range(),d.tell(),d.tell_frac(),a.coded_bands,a.intensity,u32::from(a.dual_stereo),a.balance_eighth_bits,csv(a.shape_eighth_bits),csv(a.fine_energy_bits),csv(a.fine_priority)),
        Err(e)=>println!("X|{lm}|{ch}|{start}|{end}|{total}|{trim}|{seed}|{e:?}"),
       }
      }
     }
    }
   }
  }
 }
}
'''


def configure(library):
    pointer, integer, unsigned = ctypes.c_void_p, ctypes.c_int, ctypes.c_uint32
    library.kernel_mode.argtypes = [ctypes.POINTER(integer)]
    library.kernel_mode.restype = pointer
    library.kernel_decoder.argtypes = [pointer, pointer, unsigned]
    library.kernel_bit.argtypes = [pointer, unsigned]
    library.kernel_bit.restype = integer
    for name in ["kernel_tell", "kernel_range", "kernel_frac"]:
        function = getattr(library, name)
        function.argtypes, function.restype = [pointer], unsigned
    library.kernel_coarse.argtypes = [pointer, integer, integer, pointer, integer,
                                     pointer, integer, integer]
    library.kernel_fine.argtypes = [pointer, integer, integer, pointer, pointer,
                                   pointer, integer]
    library.kernel_final.argtypes = [pointer, integer, integer, pointer, pointer,
                                    pointer, integer, pointer, integer]
    library.kernel_allocation.argtypes = [pointer, integer, integer, pointer,
                                         integer, pointer, pointer, integer,
                                         pointer, pointer, pointer, pointer,
                                         integer, integer, pointer]
    library.kernel_allocation.restype = integer
    library.opus_get_version_string.restype = ctypes.c_char_p
    version = library.opus_get_version_string().decode()
    if version != "libopus 1.6.1":
        raise RuntimeError(f"expected libopus 1.6.1, received {version}")
    library.kernel_state_size.restype = unsigned
    if library.kernel_state_size() > 256:
        raise RuntimeError("reference state exceeds test scratch capacity")
    error = integer()
    mode = library.kernel_mode(ctypes.byref(error))
    if not mode or error.value:
        raise RuntimeError("reference standard mode creation failed")
    return mode


def compare(library, mode, lines):
    integer, floating = ctypes.c_int, ctypes.c_float
    states, counts, failures = {}, Counter(), []
    max_difference, bit_differences = 0.0, 0
    for line in lines:
        cells = line.split("|")
        if cells[0] == "E":
            size, channels, lm, start, end, frame = map(int, cells[1:7])
            stage = cells[7]
            key = (size, channels, lm, start, end)
            if stage == "coarse":
                energy = states.setdefault(key, (floating * 42)())
                packet = bytes((i * 73 + frame * 41 + lm * 29 + channels * 17) & 255
                               for i in range(size))
                storage = ctypes.create_string_buffer(packet)
                context = (ctypes.c_uint64 * 32)()
                library.kernel_decoder(context, storage, size)
                intra = (library.kernel_bit(context, 3)
                         if library.kernel_tell(context) + 3 <= size * 8 else 0)
                library.kernel_coarse(mode, start, end, energy, intra,
                                      context, channels, lm)
            elif stage == "fine":
                fine = (integer * 21)(*(i % 5 for i in range(21)))
                library.kernel_fine(mode, start, end, energy, fine, context, channels)
            else:
                priority = (integer * 21)(*(i % 2 for i in range(21)))
                library.kernel_final(mode, start, end, energy, fine, priority,
                                     19, context, channels)
            actual = [library.kernel_range(context), library.kernel_tell(context),
                      library.kernel_frac(context)]
            expected = list(map(int, cells[8:11]))
            if actual != expected:
                failures.append({"case": cells[:8], "coder_expected": expected,
                                 "coder_reference": actual})
            words = struct.unpack("<42I", bytes(energy))
            for index, (rust, reference) in enumerate(zip(map(int, cells[11].split(",")), words)):
                rust_float = struct.unpack("<f", struct.pack("<I", rust))[0]
                reference_float = struct.unpack("<f", struct.pack("<I", reference))[0]
                bit_differences += rust != reference
                difference = abs(rust_float - reference_float)
                max_difference = max(max_difference, difference)
                if difference > 1e-5 * max(1.0, abs(rust_float), abs(reference_float)):
                    failures.append({"case": cells[:8], "energy_index": index,
                                     "expected": rust_float, "reference": reference_float})
            counts["energy_stages"] += 1
        else:
            lm, channels, start, end, total, trim, seed = map(int, cells[1:8])
            packet = bytes((i * 73 + seed * 41 + lm * 29 + channels * 17) & 255
                           for i in range(128))
            storage = ctypes.create_string_buffer(packet)
            context = (ctypes.c_uint64 * 32)()
            library.kernel_decoder(context, storage, 128)
            offsets = (integer * 21)(*(i % 3 * 8 if seed % 2 and start <= i < end else 0
                                      for i in range(21)))
            intensity, dual, balance = integer(), integer(), integer()
            shape, fine, priority = (integer * 21)(), (integer * 21)(), (integer * 21)()
            coded = library.kernel_allocation(mode, start, end, offsets, trim,
                                               ctypes.byref(intensity), ctypes.byref(dual), total,
                                               ctypes.byref(balance), shape, fine, priority,
                                               channels, lm, context)
            actual = [library.kernel_range(context), library.kernel_tell(context),
                      library.kernel_frac(context), coded, intensity.value, dual.value,
                      balance.value] + list(shape) + list(fine) + list(priority)
            if cells[0] == "X":
                failures.append({"case": cells[:8], "rust_error": cells[8],
                                 "reference_coded_bands": coded})
            else:
                expected = (list(map(int, cells[8:15])) + list(map(int, cells[15].split(",")))
                            + list(map(int, cells[16].split(",")))
                            + list(map(int, cells[17].split(","))))
                if actual != expected:
                    differences = [(i, a, b) for i, (a, b) in enumerate(zip(expected, actual))
                                   if a != b]
                    failures.append({"case": cells[:8], "differences": differences[:12]})
            counts["allocation_frames"] += 1
        if len(failures) >= 12:
            break
    return {"checked": dict(counts), "failures": failures,
            "energy_bit_differences": bit_differences,
            "energy_max_absolute_difference": max_difference}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="installed libopus 1.6.1 static archive")
    parser.add_argument("headers", type=Path, help="matching libopus source directory")
    parser.add_argument("--failure-output", type=Path, help="optional JSON file outside Git")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix="snacka-kernel-oracle-") as temporary:
        workspace = Path(temporary)
        c_source, rust_source = workspace / "bridge.c", workspace / "own.rs"
        c_source.write_text(BRIDGE)
        rust_source.write_text(RUST_DRIVER.replace("@REPO@", repo.as_posix()))
        library, executable = workspace / "oracle.so", workspace / "own"
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", "-DVAR_ARRAYS",
                        "-I" + str(args.headers / "include"), "-I" + str(args.headers / "celt"),
                        str(c_source), str(args.archive.resolve()), "-lm", "-o", str(library)],
                       check=True)
        subprocess.run(["rustc", "--edition=2024", "--cfg", 'feature="opus"', "-O",
                        str(rust_source), "-o", str(executable)], check=True)
        oracle = ctypes.CDLL(str(library))
        mode = configure(oracle)
        output = subprocess.check_output([str(executable)], text=True)
        report = compare(oracle, mode, output.splitlines())
        print(json.dumps(report, indent=2))
        if report["failures"]:
            if args.failure_output:
                args.failure_output.write_text(json.dumps(report, indent=2) + "\n")
            raise SystemExit(1)


if __name__ == "__main__":
    main()
