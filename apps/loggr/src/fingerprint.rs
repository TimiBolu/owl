use crate::model::StackFrame;
use sha2::{Digest, Sha256};

pub fn fingerprint_v1(
    application: &str,
    environment: &str,
    exception_type: Option<&str>,
    message: &str,
    stack: &[StackFrame],
) -> String {
    let frames = stack
        .iter()
        .filter(|frame| frame.in_app)
        .take(3)
        .map(|frame| {
            format!(
                "{}:{}:{}",
                frame.module.as_deref().unwrap_or(""),
                frame.function,
                normalize(&frame.file)
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    let canonical = format!(
        "fingerprint-v1\n{}\n{}\n{}\n{}\n{}",
        application.trim().to_lowercase(),
        environment.trim().to_lowercase(),
        exception_type.unwrap_or("").trim(),
        normalize(message),
        frames
    );
    let digest = Sha256::digest(canonical.as_bytes());
    format!("v1_{digest:x}")
}

pub fn normalize(input: &str) -> String {
    input
        .split_whitespace()
        .map(normalize_token)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_token(token: &str) -> String {
    let bare = token.trim_matches(|ch: char| {
        !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_' && ch != '.' && ch != '/'
    });
    if is_uuid(bare) {
        return token.replace(bare, "{uuid}");
    }
    if bare.starts_with("0x") && bare[2..].chars().all(|ch| ch.is_ascii_hexdigit()) {
        return token.replace(bare, "{addr}");
    }
    let mut output = String::with_capacity(token.len());
    let mut in_digits = false;
    for ch in token.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                output.push_str("{n}");
                in_digits = true;
            }
        } else {
            output.push(ch);
            in_digits = false;
        }
    }
    output
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().get(8) == Some(&b'-')
        && value.as_bytes().get(13) == Some(&b'-')
        && value.as_bytes().get(18) == Some(&b'-')
        && value.as_bytes().get(23) == Some(&b'-')
        && value.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volatile_values_do_not_change_fingerprint() {
        let a = fingerprint_v1(
            "api",
            "prod",
            Some("Timeout"),
            "request 9281 failed at 0xabc12",
            &[],
        );
        let b = fingerprint_v1(
            "api",
            "prod",
            Some("Timeout"),
            "request 441 failed at 0xdead9",
            &[],
        );
        assert_eq!(a, b);
    }

    #[test]
    fn meaningful_words_change_fingerprint() {
        assert_ne!(
            fingerprint_v1("api", "prod", None, "checkout failed", &[]),
            fingerprint_v1("api", "prod", None, "login failed", &[])
        );
    }
}
