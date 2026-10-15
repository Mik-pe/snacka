// Source-only draft. Compile/run only after the root releases its CPU window.
// Public libopus entry points only; no Snacka or foreign implementation source.
use std::{env, ffi::{CStr, c_char, c_int, c_void}, fs, path::Path, ptr::NonNull};

const LAST_DURATION: c_int = 4039;
const RESET: c_int = 4028;
const SENTINEL: u32 = 0x7fc0_1234;

#[link(name = "opus")]
unsafe extern "C" {
    fn opus_decoder_create(rate: c_int, channels: c_int, error: *mut c_int) -> *mut c_void;
    fn opus_decoder_destroy(state: *mut c_void);
    fn opus_decoder_ctl(state: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_decode_float(state: *mut c_void, data: *const u8, length: c_int,
        output: *mut f32, samples: c_int, fec: c_int) -> c_int;
    fn opus_get_version_string() -> *const c_char;
}

struct Decoder {
    state: NonNull<c_void>,
    rate: usize,
    channels: usize,
    category: String,
    output: Vec<f32>,
}

impl Decoder {
    fn new(rate: usize, channels: usize, category: &str) -> Self {
        let mut error = 0;
        // SAFETY: Documented rate/channel geometry and live error storage.
        let state = unsafe { opus_decoder_create(rate as c_int, channels as c_int, &mut error) };
        assert_eq!(error, 0);
        Self { state: NonNull::new(state).unwrap(), rate, channels,
            category: category.to_owned(),
            output: vec![f32::from_bits(SENTINEL); rate * 120 / 1000 * channels + 7] }
    }

    fn duration(&self) -> (c_int, c_int) {
        let mut value = -12345;
        // SAFETY: GET_LAST_PACKET_DURATION takes a live writable int pointer.
        let code = unsafe { opus_decoder_ctl(self.state.as_ptr(), LAST_DURATION, &mut value) };
        (code, value)
    }

    fn observe(&self, label: &str, returned: c_int) {
        let (code, duration) = self.duration();
        println!("{{\"category\":{:?},\"rate\":{},\"channels\":{},\"event\":{:?},\"return_code\":{},\"duration_ctl_code\":{},\"last_packet_samples\":{}}}",
            self.category, self.rate, self.channels, label, returned, code, duration);
    }

    fn step(&mut self, label: &str, packet: Option<&[u8]>, capacity: usize, fec: bool) {
        assert!(capacity <= self.rate * 120 / 1000);
        let before = self.duration();
        self.output.fill(f32::from_bits(SENTINEL));
        let (data, length) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as c_int));
        // SAFETY: The physical buffer always holds the full advertised capacity
        // plus a suffix. Negative tests reduce the advertised capacity only.
        // Null data/zero length is the documented PLC API.
        let returned = unsafe { opus_decode_float(self.state.as_ptr(), data, length,
            self.output.as_mut_ptr(), capacity as c_int, c_int::from(fec)) };
        let after = self.duration();
        let written = if returned > 0 { returned as usize * self.channels } else { 0 };
        assert!(written <= capacity * self.channels);
        let finite = self.output[..written].iter().all(|v| v.is_finite());
        let suffix_unchanged = self.output[written..].iter().all(|v| v.to_bits() == SENTINEL);
        println!("{{\"category\":{:?},\"rate\":{},\"channels\":{},\"event\":{:?},\"advertised_capacity\":{},\"fec\":{},\"return_code\":{},\"before_ctl_code\":{},\"before_samples\":{},\"after_ctl_code\":{},\"after_samples\":{},\"finite_returned_pcm\":{},\"suffix_unchanged\":{}}}",
            self.category, self.rate, self.channels, label, capacity, fec, returned,
            before.0, before.1, after.0, after.1, finite, suffix_unchanged);
    }

    fn reset(&mut self) {
        // SAFETY: RESET_STATE takes no variadic value on the owned live decoder.
        let code = unsafe { opus_decoder_ctl(self.state.as_ptr(), RESET) };
        self.observe("reset", code);
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: This object owns the matching create allocation exactly once.
        unsafe { opus_decoder_destroy(self.state.as_ptr()) };
    }
}

fn packets(path: &Path) -> Vec<Vec<u8>> {
    let bytes = fs::read(path).unwrap();
    let mut cursor = 0;
    let mut result = Vec::new();
    while cursor < bytes.len() {
        let length = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        result.push(bytes[cursor..cursor + length].to_vec());
        cursor += length;
    }
    assert!(result.len() >= 6);
    result
}

fn main() {
    let root = env::args().nth(1).expect("existing decoder-gain functional witness directory");
    // SAFETY: Public API returns a process-lifetime NUL-terminated version string.
    let version = unsafe { CStr::from_ptr(opus_get_version_string()) }.to_str().unwrap();
    println!("{{\"oracle\":{:?},\"scope\":\"ordinary public duration/status only; no timing or quality acceptance\"}}", version);
    for mode in ["silk", "hybrid", "celt", "silk-fec"] {
        for coded in [1, 2] {
            let category = format!("{mode}-{coded}");
            let input = packets(&Path::new(&root).join(format!("{category}.packets")));
            for rate in [8000, 12000, 16000, 24000, 48000] {
                for channels in [1, 2] {
                    let mut decoder = Decoder::new(rate, channels, &category);
                    let maximum = rate * 120 / 1000;
                    let frame = rate / 50;
                    let quantum = rate / 400;
                    decoder.observe("fresh", 0);
                    decoder.step("coded-20ms-0", Some(&input[0]), maximum, false);
                    decoder.step("coded-20ms-1", Some(&input[1]), maximum, false);
                    decoder.step("plc-7.5ms", None, quantum * 3, false);
                    decoder.step("recovery-request-40ms-prefix-plus-tail", Some(&input[4]), frame * 2, true);
                    decoder.step("normal-following-recovery-20ms", Some(&input[4]), maximum, false);
                    decoder.step("plc-40ms", None, frame * 2, false);
                    decoder.step("48-implicit-2.5ms-frames-total-120ms", Some(&[0xe3, 48]), maximum, false);
                    decoder.step("malformed-framing", Some(&[3]), maximum, false);
                    decoder.step("normal-advertised-capacity-one-sample", Some(&input[0]), 1, false);
                    decoder.step("invalid-plc-duration", None, quantum - 1, false);
                    decoder.reset();
                    decoder.step("fresh-implicit-hybrid-10ms", Some(&[0x60]), maximum, false);
                    decoder.step("coded-after-implicit-short-20ms", Some(&input[0]), maximum, false);
                }
            }
        }
    }
}
