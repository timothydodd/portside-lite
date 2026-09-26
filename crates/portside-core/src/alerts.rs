//! Deciding what deserves a desktop notification. A problem notifies once when
//! it first qualifies, stays quiet while it persists, and can notify again only
//! after it has cleared and come back.

use std::collections::HashSet;

use crate::models::{Issue, Severity};

/// Issues that qualify for notification under the user's preferences.
fn qualifies(issue: &Issue, include_warnings: bool) -> bool {
    issue.severity == Severity::Critical || (include_warnings && issue.severity == Severity::Warning)
}

/// Compare the current issues against the keys already notified for a
/// cluster. Returns the newly qualifying issues and the set to remember next
/// time (currently qualifying keys only, so resolved problems can re-notify).
pub fn diff_alerts<'a>(notified: &HashSet<String>, issues: &'a [Issue], include_warnings: bool) -> (Vec<&'a Issue>, HashSet<String>) {
    let current: Vec<&Issue> = issues.iter().filter(|i| qualifies(i, include_warnings)).collect();
    let fresh = current.iter().copied().filter(|i| !notified.contains(&i.key)).collect();
    let remember = current.iter().map(|i| i.key.clone()).collect();
    (fresh, remember)
}

/// One notification for a batch of new problems on a cluster.
pub fn alert_message(cluster_name: &str, fresh: &[&Issue]) -> Option<(String, String)> {
    let first = fresh.first()?;
    if fresh.len() == 1 {
        let where_ = first.namespace.as_deref().map(|n| format!(" · {n}")).unwrap_or_default();
        return Some((format!("{cluster_name}: {}", first.title), format!("{}{where_}", first.detail)));
    }
    let critical = fresh.iter().filter(|i| i.severity == Severity::Critical).count();
    let noun = if critical == fresh.len() { "critical problems" } else { "new problems" };
    let mut body: Vec<String> = fresh.iter().take(3).map(|i| format!("• {}", i.title)).collect();
    if fresh.len() > 3 {
        body.push(format!("…and {} more", fresh.len() - 3));
    }
    Some((format!("{cluster_name}: {} {noun}", fresh.len()), body.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(key: &str, severity: Severity) -> Issue {
        Issue {
            key: key.into(),
            severity,
            category: "pod".into(),
            rule: "r".into(),
            kind: "Pod".into(),
            namespace: Some("apps".into()),
            name: key.into(),
            title: format!("CrashLoopBackOff: {key}"),
            detail: "exited 1".into(),
            hint: None,
            since_ms: None,
            actions: vec![],
            first_seen_ms: None,
        }
    }

    #[test]
    fn notifies_once_then_again_after_resolving() {
        let a = [issue("a", Severity::Critical)];
        let (fresh, seen) = diff_alerts(&HashSet::new(), &a, false);
        assert_eq!(fresh.len(), 1);
        let (fresh, seen) = diff_alerts(&seen, &a, false);
        assert!(fresh.is_empty(), "still broken → stay quiet");
        let (_, seen) = diff_alerts(&seen, &[], false);
        assert!(seen.is_empty(), "resolved → forgotten");
        let (fresh, _) = diff_alerts(&seen, &a, false);
        assert_eq!(fresh.len(), 1, "came back → notify again");
    }

    #[test]
    fn warnings_only_when_enabled() {
        let w = [issue("w", Severity::Warning), issue("i", Severity::Info)];
        assert!(diff_alerts(&HashSet::new(), &w, false).0.is_empty());
        assert_eq!(diff_alerts(&HashSet::new(), &w, true).0.len(), 1, "info never notifies");
    }

    #[test]
    fn batches_into_one_message() {
        let v: Vec<Issue> = (0..5).map(|n| issue(&format!("p{n}"), Severity::Critical)).collect();
        let refs: Vec<&Issue> = v.iter().collect();
        let (title, body) = alert_message("Homelab", &refs).unwrap();
        assert_eq!(title, "Homelab: 5 critical problems");
        assert!(body.ends_with("…and 2 more"));
        let (title, body) = alert_message("Homelab", &refs[..1]).unwrap();
        assert_eq!(title, "Homelab: CrashLoopBackOff: p0");
        assert_eq!(body, "exited 1 · apps");
        assert!(alert_message("Homelab", &[]).is_none());
    }
}
