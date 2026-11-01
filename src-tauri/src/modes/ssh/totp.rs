//! TOTP (RFC 6238) for SSH second factors.
//!
//! A bastion that wants a one-time code makes every agent call depend on a
//! human typing six digits. Enrolling the TOTP seed once turns that into a
//! value the app can compute, so the same `ssh_exec` works headless — and the
//! user still decides per profile whether to hand that factor over.
//!
//! The seed lives in the OS credential store (same place as the password),
//! never in the SQLite profile row and never in the UI's settings table.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use sha1::Sha1;

/// Seconds per code, per RFC 6238 / every authenticator app default.
const PERIOD: u64 = 30;
const DIGITS: u32 = 6;

/// Accept what people actually paste: a bare base32 seed (any case, spaces or
/// dashes as separators), or a full `otpauth://totp/...` URI from a QR code.
/// Rejects anything that is not base32 so a wrong paste fails at enrolment
/// instead of at 3am during an auth.
pub fn normalize_secret(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty secret".to_string());
    }
    let seed = if trimmed.to_ascii_lowercase().starts_with("otpauth://") {
        let uri = trimmed;
        let query = uri
            .split_once('?')
            .map(|(_, q)| q)
            .ok_or_else(|| "otpauth URI has no query string".to_string())?;
        let mut found: Option<String> = None;
        for pair in query.split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            if k.eq_ignore_ascii_case("secret") {
                found = Some(v.to_string());
                break;
            }
        }
        found.ok_or_else(|| "otpauth URI has no `secret=` parameter".to_string())?
    } else {
        trimmed.to_string()
    };

    let cleaned: String = seed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .collect();
    let upper = cleaned.to_ascii_uppercase();
    let sans_padding = upper.trim_end_matches('=').to_string();
    BASE32_NOPAD
        .decode(sans_padding.as_bytes())
        .map_err(|e| format!("secret is not valid base32: {}", e))?;
    Ok(sans_padding)
}

/// Code for an explicit moment — the testable core.
pub fn code_at(secret: &str, unix_secs: u64, digits: u32, period: u64) -> Result<String, String> {
    let key = BASE32_NOPAD
        .decode(secret.as_bytes())
        .map_err(|e| format!("secret is not valid base32: {}", e))?;
    if key.is_empty() {
        return Err("empty secret".to_string());
    }
    let counter = unix_secs / period;

    let mut mac = Hmac::<Sha1>::new_from_slice(&key).map_err(|e| format!("hmac key: {}", e))?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();

    // RFC 4226 §5.3 dynamic truncation.
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | digest[offset + 3] as u32;
    let modulo = 10u32.pow(digits);
    Ok(format!("{:0width$}", binary % modulo, width = digits as usize))
}

/// Code for right now, plus how long it stays valid (for UI previews).
pub fn current_code(secret: &str) -> Result<(String, u64), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock: {}", e))?
        .as_secs();
    let code = code_at(secret, now, DIGITS, PERIOD)?;
    Ok((code, PERIOD - (now % PERIOD)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 Appendix B seeds/vectors (SHA-1, 8 digits; we ask for 6).
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"; // "12345678901234567890"

    #[test]
    fn rfc6238_sha1_vectors_match() {
        // Values from the RFC for the 20-byte SHA-1 seed, truncated to 6.
        for (secs, expected) in [
            (59u64, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(
                code_at(RFC_SECRET, secs, 6, 30).unwrap(),
                expected,
                "T={}",
                secs
            );
        }
    }

    #[test]
    fn accepts_what_people_paste() {
        assert_eq!(normalize_secret("gezd gnbv gy3t qojq").unwrap(), "GEZDGNBVGY3TQOJQ");
        assert_eq!(normalize_secret("GEZD-GNBV-GY3T-QOJQ").unwrap(), "GEZDGNBVGY3TQOJQ");
        assert_eq!(
            normalize_secret("GEZDGNBVGY3TQOJQ====").unwrap(),
            "GEZDGNBVGY3TQOJQ"
        );
        let uri = "otpauth://totp/JumpServer:yangxing?secret=GEZDGNBVGY3TQOJQ&issuer=JumpServer";
        assert_eq!(normalize_secret(uri).unwrap(), "GEZDGNBVGY3TQOJQ");
    }

    #[test]
    fn rejects_junk_at_enrolment() {
        assert!(normalize_secret("").is_err());
        assert!(normalize_secret("not a secret!").is_err());
        assert!(normalize_secret("otpauth://totp/x?issuer=y").is_err());
    }

    #[test]
    fn current_code_is_six_digits_with_a_reset_window() {
        let (code, left) = current_code("GEZDGNBVGY3TQOJQ").unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert!(left >= 1 && left <= 30, "left={}", left);
    }
}