//! Parsing of container log lines as returned by the Kubernetes log API with
//! `timestamps=true` ("<RFC3339Nano> <message>"), plus level detection.

use crate::jiff::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }

    pub fn from_str(s: &str) -> Option<Level> {
        Some(match s.to_ascii_lowercase().as_str() {
            "trace" | "trc" | "verbose" => Level::Trace,
            "debug" | "dbg" | "dbug" => Level::Debug,
            "info" | "inf" | "information" | "notice" => Level::Info,
            "warn" | "wrn" | "warning" => Level::Warning,
            "error" | "err" | "fail" | "fatal" | "critical" | "crit" | "panic" | "alert"
            | "emerg" => Level::Error,
            _ => return None,
        })
    }
}

/// Split the API-server timestamp prefix off a line. Returns nanoseconds since
/// the epoch and the remaining message.
pub fn split_timestamp(line: &str) -> Option<(i64, &str)> {
    let (ts, rest) = line.split_once(' ').unwrap_or((line, ""));
    let parsed: Timestamp = ts.parse().ok()?;
    Some((parsed.as_nanosecond() as i64, rest))
}

/// Remove ANSI color escapes.
pub fn strip_ansi(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\x1b') {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            // CSI: parameters then a single final byte in @..~
            for n in chars.by_ref() {
                if ('@'..='~').contains(&n) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Best-effort level detection. Structured forms win (JSON `level`, logfmt
/// `level=`, klog `E0926`, a leading `[WARN]`/`info:` token); otherwise fall
/// back to the same marker search Portside uses.
pub fn detect_level(msg: &str) -> Level {
    let trimmed = msg.trim_start();

    if trimmed.starts_with('{') {
        if let Some(l) = json_level(trimmed) {
            return l;
        }
    }
    if let Some(l) = logfmt_level(trimmed) {
        return l;
    }
    if let Some(l) = klog_level(trimmed) {
        return l;
    }
    if let Some(l) = leading_token_level(trimmed) {
        return l;
    }

    let upper_has = |needle: &str| {
        trimmed
            .as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
    };
    if upper_has("ERROR") || upper_has("EXCEPTION") || upper_has("FATAL") || upper_has("PANIC") {
        Level::Error
    } else if upper_has("WARN") {
        Level::Warning
    } else if upper_has("DEBUG") {
        Level::Debug
    } else if upper_has("TRACE") {
        Level::Trace
    } else {
        Level::Info
    }
}

fn json_level(s: &str) -> Option<Level> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let obj = v.as_object()?;
    for key in ["level", "lvl", "severity", "log.level", "loglevel", "@l"] {
        if let Some(val) = obj.get(key) {
            if let Some(s) = val.as_str() {
                if let Some(l) = Level::from_str(s) {
                    return Some(l);
                }
            }
            // bunyan/pino numeric levels
            if let Some(n) = val.as_i64() {
                return Some(match n {
                    ..=10 => Level::Trace,
                    11..=20 => Level::Debug,
                    21..=30 => Level::Info,
                    31..=40 => Level::Warning,
                    _ => Level::Error,
                });
            }
        }
    }
    None
}

fn logfmt_level(s: &str) -> Option<Level> {
    for key in ["level=", "lvl=", "severity="] {
        if let Some(idx) = s.find(key) {
            // must be at a token boundary
            if idx > 0 && !s.as_bytes()[idx - 1].is_ascii_whitespace() {
                continue;
            }
            let rest = &s[idx + key.len()..];
            let val: String = rest
                .trim_start_matches('"')
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if let Some(l) = Level::from_str(&val) {
                return Some(l);
            }
        }
    }
    None
}

/// klog header: `E0926 12:34:56.789012   1234 file.go:12] message`
fn klog_level(s: &str) -> Option<Level> {
    let b = s.as_bytes();
    if b.len() < 6 || !b[1..5].iter().all(u8::is_ascii_digit) || b[5] != b' ' {
        return None;
    }
    Some(match b[0] {
        b'E' | b'F' => Level::Error,
        b'W' => Level::Warning,
        b'I' => Level::Info,
        _ => return None,
    })
}

/// `[WARN] …`, `WARN: …`, `info: …`, `ERROR …`
fn leading_token_level(s: &str) -> Option<Level> {
    let token: String = s
        .trim_start_matches('[')
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if token.is_empty() {
        return None;
    }
    let after = &s.trim_start_matches('[')[token.len()..];
    let delimited = after.is_empty()
        || after.starts_with(']')
        || after.starts_with(':')
        || after.starts_with(' ')
        || after.starts_with('|');
    if delimited {
        Level::from_str(&token)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_api_timestamp() {
        let (ns, msg) = split_timestamp("2026-09-26T10:00:00.123456789Z hello world").unwrap();
        assert_eq!(msg, "hello world");
        assert_eq!(ns % 1_000_000_000, 123_456_789);
        assert!(split_timestamp("not a timestamp").is_none());
    }

    #[test]
    fn levels() {
        assert_eq!(detect_level(r#"{"level":"warn","msg":"x"}"#), Level::Warning);
        assert_eq!(detect_level(r#"{"level":50,"msg":"x"}"#), Level::Error);
        assert_eq!(detect_level("time=now level=error msg=boom"), Level::Error);
        assert_eq!(
            detect_level("E0926 12:34:56.789012   1234 file.go:12] failed"),
            Level::Error
        );
        assert_eq!(detect_level("I0926 12:34:56.789012   1 x.go:1] ok"), Level::Info);
        assert_eq!(detect_level("[WARN] cache miss"), Level::Warning);
        assert_eq!(detect_level("info: Microsoft.Hosting started"), Level::Info);
        assert_eq!(detect_level("unhandled exception in handler"), Level::Error);
        assert_eq!(detect_level("GET /health 200"), Level::Info);
        // a leading "information" token beats a later "error" substring
        assert_eq!(detect_level("info: 0 errors found"), Level::Info);
    }

    #[test]
    fn ansi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m text"), "red text");
    }
}
