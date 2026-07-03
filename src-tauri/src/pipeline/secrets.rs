//! Layer-3 secret/PII detection: known key formats + entropy heuristics.
//! (Layers 1 and 2 — OS conceal flags and the app exclusion list — run at
//! capture time, before this code ever sees the text.)

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitivity {
    Public,
    Secret,
}

const KEY_PATTERNS: &[&str] = &[
    r"\bsk-[A-Za-z0-9_-]{20,}\b",              // OpenAI-style
    r"\bgsk_[A-Za-z0-9]{20,}\b",               // Groq
    r"\bgh[pousr]_[A-Za-z0-9]{20,}\b",         // GitHub tokens
    r"\bAKIA[0-9A-Z]{16}\b",                   // AWS access key id
    r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",       // Slack
    r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{5,}\b", // JWT
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----",     // PEM
    r"(?i)\b(?:password|passwd|pwd|secret|api[_-]?key|token)\s*[:=]\s*\S+",
];

fn compiled() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        KEY_PATTERNS
            .iter()
            .map(|p| Regex::new(p).expect("valid secret regex"))
            .collect()
    })
}

/// Shannon entropy in bits per character.
fn entropy_per_char(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let n = s.chars().count() as f64;
    if n == 0.0 {
        return 0.0;
    }
    for c in s.chars() {
        *counts.entry(c).or_insert(0.0) += 1.0;
    }
    counts
        .values()
        .map(|c| {
            let p = c / n;
            -p * p.log2()
        })
        .sum()
}

/// A token "looks like a secret" if it's long, has no spaces, mixes character
/// classes, and has high entropy — e.g. a random 40-char API key.
fn high_entropy_token(t: &str) -> bool {
    if t.len() < 24 || t.len() > 512 || t.contains(char::is_whitespace) {
        return false;
    }
    let has_lower = t.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = t.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let classes = has_lower as u8 + has_upper as u8 + has_digit as u8;
    // URLs and file paths are structured, not secrets
    if t.starts_with("http") || t.contains("://") || t.contains('/') && t.contains('.') {
        return false;
    }
    classes >= 3 && entropy_per_char(t) > 4.2
}

pub fn detect(text: &str) -> Sensitivity {
    let t = text.trim();
    for re in compiled() {
        if re.is_match(t) {
            return Sensitivity::Secret;
        }
    }
    // check whitespace-separated tokens for bare high-entropy strings
    if t.split_whitespace().take(50).any(high_entropy_token) {
        return Sensitivity::Secret;
    }
    Sensitivity::Public
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_key_formats_are_secret() {
        let secrets = [
            "sk-proj-a1B2c3D4e5F6g7H8i9J0a1B2c3D4",
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "AKIAIOSFODNN7EXAMPLE",
            "xoxb-1234567890-abcdefghijkl",
            "password = hunter2secret",
            "API_KEY: abc123def456",
            "-----BEGIN RSA PRIVATE KEY-----",
        ];
        for s in secrets {
            assert_eq!(detect(s), Sensitivity::Secret, "should flag: {s}");
        }
    }

    #[test]
    fn high_entropy_bare_token_is_secret() {
        assert_eq!(
            detect("k9Xq2mVp7Rt4Lw8Zn3Jf6Hd1Bs5Gy0Ce"),
            Sensitivity::Secret
        );
    }

    #[test]
    fn normal_content_is_public() {
        let public = [
            "hello world, meeting at 3pm",
            "#3B82F6",
            "https://github.com/asg017/sqlite-vec",
            "const store = useAuthStore();",
            "alice@example.com",
            r"C:\Users\adamy\Desktop\projects",
            "SQLCipher provides transparent 256-bit AES encryption",
        ];
        for s in public {
            assert_eq!(detect(s), Sensitivity::Public, "should NOT flag: {s}");
        }
    }
}
