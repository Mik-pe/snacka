#!/usr/bin/env python3
"""Reference-decode emitted mapped and projected coded transport packets."""
import ctypes as C
import json
import math
from pathlib import Path
import struct
import sys

directory=Path(sys.argv[1])
opus=C.CDLL('/opt/homebrew/lib/libopus.dylib')
opus.opus_multistream_decoder_create.argtypes=[C.c_int,C.c_int,C.c_int,C.c_int,C.POINTER(C.c_ubyte),C.POINTER(C.c_int)]
opus.opus_multistream_decoder_create.restype=C.c_void_p
opus.opus_multistream_decoder_destroy.argtypes=[C.c_void_p]
opus.opus_multistream_decode_float.argtypes=[C.c_void_p,C.c_void_p,C.c_int,C.POINTER(C.c_float),C.c_int,C.c_int]
opus.opus_multistream_decode_float.restype=C.c_int
opus.opus_multistream_decoder_ctl.argtypes=[C.c_void_p,C.c_int]
opus.opus_multistream_decoder_ctl.restype=C.c_int
summaries=[json.loads(line) for line in (directory/'streams.jsonl').read_text().splitlines()]
for summary in summaries:
    mapping=(C.c_ubyte*len(summary['mapping']))(*summary['mapping'])
    error=C.c_int()
    native=opus.opus_multistream_decoder_create(48_000,summary['output_channels'],summary['streams'],summary['coupled'],mapping,C.byref(error))
    assert native and error.value==0
    count=0
    data=(directory/(summary['name']+'.packets')).read_bytes()
    offset=0
    try:
        while offset<len(data):
            length,interval,samples,flags,channels=struct.unpack_from('<5I',data,offset)
            offset+=20
            packet=data[offset:offset+length]
            offset+=length
            if flags&1: assert opus.opus_multistream_decoder_ctl(native,4028)==0
            output=(C.c_float*(samples*channels))()
            storage=C.create_string_buffer(packet)
            decoded=opus.opus_multistream_decode_float(native,storage,length,output,samples,0)
            assert decoded==samples,(summary['name'],count,decoded)
            interval_native=C.c_uint32()
            assert opus.opus_multistream_decoder_ctl(native,4031,C.byref(interval_native))==0
            assert interval_native.value==interval,(summary['name'],count,hex(interval_native.value),hex(interval))
            assert all(math.isfinite(x) for x in output)
            count+=1
    finally:
        opus.opus_multistream_decoder_destroy(native)
    assert count==summary['packets']
    summary['native_packets']=count
(directory/'native-validation.json').write_text(json.dumps(summaries,indent=2)+'\n')
print(json.dumps({'profiles':len(summaries),'packets':sum(x['packets'] for x in summaries),'native_final_ranges':'exact','native_pcm':'finite; perceptual multistream quality unmeasured'}))
