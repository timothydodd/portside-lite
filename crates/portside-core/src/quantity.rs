//! Kubernetes resource quantity parsing ("250m", "1.5", "512Mi", "1G", "1e3").

/// Parse a CPU quantity into cores. Returns 0.0 for anything unparseable.
pub fn parse_cpu(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    let (num, scale) = match s.as_bytes()[s.len() - 1] {
        b'n' => (&s[..s.len() - 1], 1e-9),
        b'u' => (&s[..s.len() - 1], 1e-6),
        b'm' => (&s[..s.len() - 1], 1e-3),
        _ => (s, 1.0),
    };
    num.parse::<f64>().map(|v| v * scale).unwrap_or(0.0)
}

/// Parse a memory quantity into bytes. Binary suffixes (Ki, Mi, …) are powers
/// of 1024; decimal suffixes (k, M, G, …) are powers of 1000.
pub fn parse_memory(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    const SUFFIXES: &[(&str, f64)] = &[
        ("Ki", 1024.0),
        ("Mi", 1024.0 * 1024.0),
        ("Gi", 1024.0 * 1024.0 * 1024.0),
        ("Ti", 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("Pi", 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("Ei", 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("k", 1e3),
        ("K", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
        ("P", 1e15),
        ("E", 1e18),
        ("m", 1e-3),
    ];
    for (suffix, mult) in SUFFIXES {
        if let Some(num) = s.strip_suffix(suffix) {
            return num.parse::<f64>().map(|v| v * mult).unwrap_or(0.0);
        }
    }
    // Plain number or exponent form ("1e3").
    s.parse::<f64>().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu() {
        assert_eq!(parse_cpu("250m"), 0.25);
        assert_eq!(parse_cpu("2"), 2.0);
        assert_eq!(parse_cpu("1.5"), 1.5);
        assert!((parse_cpu("123456789n") - 0.123456789).abs() < 1e-12);
        assert_eq!(parse_cpu("500u"), 0.0005);
        assert_eq!(parse_cpu("garbage"), 0.0);
    }

    #[test]
    fn memory() {
        assert_eq!(parse_memory("1Ki"), 1024.0);
        assert_eq!(parse_memory("512Mi"), 512.0 * 1024.0 * 1024.0);
        assert_eq!(parse_memory("1G"), 1e9);
        assert_eq!(parse_memory("1e3"), 1000.0);
        assert_eq!(parse_memory("4096"), 4096.0);
        assert_eq!(parse_memory(""), 0.0);
    }
}
