# SPDX-License-Identifier: MIT
"""Compare owned SILK excitation parsing with the installed libopus oracle.

This is a microbenchmark of decoder initialization and signed pulse parsing,
not whole-codec performance. Both processes traverse the same 54 WB/20 ms
frozen cases and must yield the same summed final entropy interval. Compile
and allocation are excluded from timing. No external codec source is read.
"""

import argparse
import ctypes
from pathlib import Path
import re
import statistics
import subprocess
import sys
import tempfile

from generate_entropy_fixtures import configure

ROOT = Path(__file__).resolve().parents[2]

RUST = r"""use std::hint::black_box;
use std::time::Instant;
#[path = "@ENTROPY@"] mod entropy;
mod opus { #[path = "@SILK@"] pub mod silk; }
fn main() {
    let data = include_bytes!("@FIXTURE@");
    let mut cases=Vec::new();
    let mut cursor=12;
    for _ in 0..u32::from_le_bytes(data[8..12].try_into().unwrap()) {
        let shape=data[cursor];
        let kind=data[cursor+1];
        let len=u16::from_le_bytes(data[cursor+2..cursor+4].try_into().unwrap()) as usize;
        let coded=u16::from_le_bytes(data[cursor+4..cursor+6].try_into().unwrap()) as usize;
        cursor+=18;
        if shape==5 { cases.push((kind,&data[cursor..cursor+len])); }
        cursor+=len+coded*2;
    }
    let frame=opus::silk::Frame { bandwidth:opus::silk::Bandwidth::Wide, duration:opus::silk::FrameDuration::Ms20 };
    let rounds=std::env::args().nth(1).unwrap().parse::<usize>().unwrap();
    let mut out=[0;320];
    let start=Instant::now();
    let mut sum=0u64;
    for _ in 0..rounds {
        for &(index,packet) in &cases {
            let kind=opus::silk::FrameType { signal: match index/2 {0=>opus::silk::SignalType::Inactive,1=>opus::silk::SignalType::Unvoiced,_=>opus::silk::SignalType::Voiced}, offset: if index&1==0 {opus::silk::QuantizationOffset::Low} else {opus::silk::QuantizationOffset::High} };
            let mut decoder=entropy::Decoder::new(black_box(packet));
            opus::silk::decode_pulses(&mut decoder,frame,kind,&mut out).unwrap();
            sum=sum.wrapping_add(u64::from(black_box(decoder.range())));
        }
    }
    println!("frames={} ns_per_frame={} checksum={}",rounds*cases.len(),start.elapsed().as_nanos()/(rounds*cases.len()) as u128,sum);
}
"""

C = r"""#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
extern void oracle_ec_dec_init(void*,unsigned char*,uint32_t);
extern void oracle_silk_decode_pulses(void*,int16_t*,int,int,int);
extern uint32_t oracle_range(void*);
typedef struct { unsigned char *data; uint32_t capacity, tail, reservoir; int reservoir_bits, total; uint32_t position, range, value, auxiliary; int pending, error; } State;
static uint32_t u32(unsigned char*p) {return p[0]|(p[1]<<8)|(p[2]<<16)|((uint32_t)p[3]<<24);}
static uint16_t u16(unsigned char*p) {return p[0]|(p[1]<<8);}
int main(int argc,char**argv) {
    FILE*f=fopen(argv[1],"rb");fseek(f,0,SEEK_END);long len=ftell(f);rewind(f);unsigned char*data=malloc(len);fread(data,1,len,f);fclose(f);
    unsigned char*packets[1000];unsigned lengths[1000],kinds[1000],count=0;
    unsigned cursor=12;
    for(unsigned i=0;i<u32(data+8);i++) {unsigned shape=data[cursor],kind=data[cursor+1],size=u16(data+cursor+2),coded=u16(data+cursor+4);cursor+=18;if(shape==5){packets[count]=data+cursor;lengths[count]=size;kinds[count]=kind;count++;}cursor+=size+coded*2;}
    unsigned rounds=(unsigned)strtoul(argv[2],0,10);
    int16_t out[320];uint64_t sum=0;struct timespec start,end;clock_gettime(CLOCK_MONOTONIC,&start);
    for(unsigned round=0;round<rounds;round++) for(unsigned i=0;i<count;i++) {State state;oracle_ec_dec_init(&state,packets[i],lengths[i]);oracle_silk_decode_pulses(&state,out,kinds[i]/2,kinds[i]&1,320);sum+=oracle_range(&state);}
    clock_gettime(CLOCK_MONOTONIC,&end);uint64_t ns=(end.tv_sec-start.tv_sec)*1000000000ull+end.tv_nsec-start.tv_nsec;
    printf("frames=%u ns_per_frame=%llu checksum=%llu\n",rounds*count,ns/(rounds*count),sum);
    free(data);
}
"""


def run(command):
    result = subprocess.run(command, check=True, capture_output=True, text=True)
    match = re.fullmatch(r"frames=(\d+) ns_per_frame=(\d+) checksum=(\d+)\s*", result.stdout)
    if not match:
        raise RuntimeError("unexpected benchmark output: " + result.stdout)
    return tuple(int(value) for value in match.groups())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="separately installed libopus 1.6.1 static archive")
    parser.add_argument("--rounds", type=int, default=20000)
    parser.add_argument("--runs", type=int, default=3)
    args = parser.parse_args()
    if args.rounds < 1 or args.runs < 1:
        parser.error("rounds and runs must be positive")
    fixture = ROOT / "tests/fixtures/opus-silk-pulses.bin"
    with tempfile.TemporaryDirectory(prefix="snacka-silk-benchmark-") as temporary:
        directory = Path(temporary)
        oracle = directory / "oracle.so"
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        glue = [str(Path(__file__).with_name(name)) for name in ("entropy_bridge.c", "silk_bridge.c")]
        archive = str(args.archive.resolve())
        subprocess.run(["cc", flag, "-fPIC", *glue, archive, "-lm", "-o", str(oracle)], check=True)
        configure(ctypes.CDLL(str(oracle)))
        source = RUST.replace("@ENTROPY@", str(ROOT / "src/entropy.rs")).replace(
            "@SILK@", str(ROOT / "src/opus/silk/mod.rs")).replace("@FIXTURE@", str(fixture))
        rust_source, c_source = directory / "rust.rs", directory / "native.c"
        rust_source.write_text(source)
        c_source.write_text(C)
        rust, native = directory / "rust", directory / "native"
        subprocess.run(["rustc", "-O", "--edition", "2024", "--cfg", 'feature="opus"',
                        "-A", "dead_code", "-A", "unused_imports", str(rust_source),
                        "-o", str(rust)], check=True)
        subprocess.run(["cc", "-O3", str(c_source), *glue, archive, "-lm", "-o", str(native)], check=True)
        results = [[], []]
        frames, checksum = None, None
        for iteration in range(args.runs):
            order = (0, 1) if iteration % 2 == 0 else (1, 0)
            for index in order:
                command = [str(rust), str(args.rounds)] if index == 0 else [
                    str(native), str(fixture), str(args.rounds)]
                count, timing, digest = run(command)
                if checksum is not None and (count != frames or digest != checksum):
                    raise RuntimeError("native and owned decoding checksum differ")
                frames, checksum = count, digest
                results[index].append(timing)
        own, reference = (statistics.median(values) for values in results)
        print(f"frames_per_run={frames}, checksum={checksum}, rust_ns={results[0]}, libopus_ns={results[1]}, "
              f"rust_median_ns={own}, libopus_median_ns={reference}, rust_over_libopus={own/reference:.4f}")


if __name__ == "__main__":
    main()
