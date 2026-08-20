use std::hint::black_box;
use std::time::Instant;

use codey_lib::fastctx::protocol::{ResponseDisposition, response_disposition};
use serde_json::Value;

const ITERATIONS: usize = 4_000;
const PAYLOAD_BYTES: usize = 1024 * 1024;

fn main() {
    let mode = std::env::args()
        .find(|argument| matches!(argument.as_str(), "legacy" | "optimized"))
        .unwrap_or_else(|| "optimized".to_string());
    let payload = response_payload(PAYLOAD_BYTES);
    let started = Instant::now();
    let mut checksum = 0_usize;
    match mode.as_str() {
        "legacy" => {
            for _ in 0..ITERATIONS {
                let value: Value = serde_json::from_slice(black_box(&payload)).unwrap();
                let succeeded = value.get("id").and_then(Value::as_u64) == Some(7)
                    && value.get("result").is_some()
                    && value.get("error").is_none();
                checksum = checksum.wrapping_add(usize::from(black_box(succeeded)));
                black_box(value);
            }
        }
        "optimized" => {
            for _ in 0..ITERATIONS {
                let succeeded =
                    response_disposition(black_box(&payload), "7") == ResponseDisposition::Success;
                checksum = checksum.wrapping_add(usize::from(black_box(succeeded)));
            }
        }
        other => panic!("unknown mode {other}; expected legacy or optimized"),
    }
    let elapsed = started.elapsed();
    println!(
        "{{\"mode\":\"{}\",\"iterations\":{},\"payloadBytes\":{},\"elapsedNs\":{},\"nsPerOp\":{},\"checksum\":{},\"peakRssBytes\":{}}}",
        mode,
        ITERATIONS,
        payload.len(),
        elapsed.as_nanos(),
        elapsed.as_nanos() / ITERATIONS as u128,
        checksum,
        peak_rss_bytes()
    );
}

fn response_payload(bytes: usize) -> Vec<u8> {
    let prefix = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"text\":\"";
    let suffix = b"\"}}\n";
    let mut payload = Vec::with_capacity(bytes);
    payload.extend_from_slice(prefix);
    payload.resize(bytes.saturating_sub(suffix.len()), b'A');
    payload.extend_from_slice(suffix);
    payload
}

#[cfg(target_os = "macos")]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result == 0 {
        unsafe { usage.assume_init().ru_maxrss as u64 }
    } else {
        0
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result == 0 {
        unsafe { usage.assume_init().ru_maxrss as u64 }.saturating_mul(1024)
    } else {
        0
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0
}
