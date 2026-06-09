use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use sha1::Sha1;

use crate::{RcwError, RcwResult};

type HmacSha1 = Hmac<Sha1>;

pub const DEFAULT_TOTP_DIGITS: u32 = 6;
pub const DEFAULT_SKEW_WINDOWS: i64 = 1;

pub fn random_seed() -> Vec<u8> {
    let mut seed = vec![0_u8; 20];
    OsRng.fill_bytes(&mut seed);
    seed
}

pub fn current_code(seed: &[u8], period_seconds: u64, now_unix_seconds: u64) -> RcwResult<String> {
    if period_seconds == 0 {
        return Err(RcwError::InvalidConfig(
            "TOTP period must be greater than zero".to_owned(),
        ));
    }
    let counter = now_unix_seconds / period_seconds;
    Ok(hotp(seed, counter, DEFAULT_TOTP_DIGITS))
}

pub fn verify_code(
    code: &str,
    seed: &[u8],
    period_seconds: u64,
    now_unix_seconds: u64,
    skew_windows: i64,
) -> RcwResult<bool> {
    if !code.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(false);
    }
    let current = (now_unix_seconds / period_seconds) as i64;
    for offset in -skew_windows..=skew_windows {
        let counter = current + offset;
        if counter < 0 {
            continue;
        }
        if hotp(seed, counter as u64, DEFAULT_TOTP_DIGITS) == code {
            return Ok(true);
        }
    }
    Ok(false)
}

fn hotp(seed: &[u8], counter: u64, digits: u32) -> String {
    let mut mac = HmacSha1::new_from_slice(seed).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = (((digest[offset] & 0x7f) as u32) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    let divisor = 10_u32.pow(digits);
    format!("{:0width$}", binary % divisor, width = digits as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_current_and_adjacent_windows() {
        let seed = b"12345678901234567890";
        let period = 120;
        let now = 1_780_000_000;
        let code = current_code(seed, period, now).unwrap();

        assert!(verify_code(&code, seed, period, now, 1).unwrap());
        assert!(verify_code(&code, seed, period, now + period, 1).unwrap());
        assert!(!verify_code("abcdef", seed, period, now, 1).unwrap());
    }
}
