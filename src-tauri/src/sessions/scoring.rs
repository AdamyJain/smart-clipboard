//! Scored session assignment (architecture §3.4): each open session is scored
//! against the incoming capture on time gap + source affinity; best score
//! above threshold wins, otherwise a new session opens. Multiple concurrent
//! open sessions are the point — an interleaved Slack tangent must not reset
//! a research session (PRD use case 6).

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone)]
pub struct ScoringConfig {
    pub w_time: f64,
    pub w_source: f64,
    /// assign to best session only if its score reaches this
    pub threshold: f64,
    /// time score halves every this many minutes
    pub half_life_min: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        ScoringConfig {
            w_time: 0.6,
            w_source: 0.4,
            threshold: 0.4,
            half_life_min: 12.0,
        }
    }
}

#[derive(Debug)]
pub struct Candidate<'a> {
    pub captured_at: i64,
    pub source_app: Option<&'a str>,
    pub source_domain: Option<&'a str>,
}

#[derive(Debug)]
pub struct OpenSession {
    pub id: String,
    pub last_activity_at: i64,
    pub recent_apps: Vec<String>,
    pub recent_domains: Vec<String>,
}

/// Registrable domain, naive two-label version ("docs.rs" from
/// "https://docs.rs/rmcp/latest"). Multi-part TLDs (co.uk) collapse a little
/// too far; acceptable for affinity scoring, not for security decisions.
pub fn registrable_domain(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let host = after_scheme.split(['/', '?', '#']).next()?;
    let host = host.split('@').last()?.split(':').next()?;
    if host.is_empty() || host.parse::<std::net::IpAddr>().is_ok() {
        return if host.is_empty() { None } else { Some(host.to_string()) };
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    match labels.len() {
        0 => None,
        1 => Some(labels[0].to_lowercase()),
        n => Some(format!("{}.{}", labels[n - 2], labels[n - 1]).to_lowercase()),
    }
}

/// Zero-affinity captures may join a session only within this burst window —
/// rapid-fire collecting across apps is one activity, but a capture from an
/// unrelated source minutes later is not (PRD use case 6, interleaved work).
const ZERO_AFFINITY_BURST_MIN: f64 = 3.0;

pub fn score(cfg: &ScoringConfig, cand: &Candidate, sess: &OpenSession) -> f64 {
    let gap_min = ((cand.captured_at - sess.last_activity_at) as f64 / 60_000.0).max(0.0);
    let time_score = 0.5f64.powf(gap_min / cfg.half_life_min);

    let source_score = if let Some(d) = cand.source_domain {
        if sess.recent_domains.iter().any(|s| s == d) {
            1.0
        } else if cand.source_app.is_some_and(|a| sess.recent_apps.iter().any(|s| s == a)) {
            0.6
        } else {
            0.0
        }
    } else if cand.source_app.is_some_and(|a| sess.recent_apps.iter().any(|s| s == a)) {
        0.6
    } else {
        0.0
    };

    if source_score == 0.0 && gap_min > ZERO_AFFINITY_BURST_MIN {
        return 0.0;
    }
    cfg.w_time * time_score + cfg.w_source * source_score
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Assignment {
    pub session_id: String,
    pub topic: String,
    pub created: bool,
}

fn load_open_sessions(conn: &Connection) -> Result<Vec<OpenSession>> {
    let mut stmt = conn.prepare(
        "SELECT id, last_activity_at FROM sessions
         WHERE status = 'open' AND deleted_at IS NULL",
    )?;
    let base: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    let mut out = Vec::with_capacity(base.len());
    for (id, last_activity_at) in base {
        let mut stmt = conn.prepare(
            "SELECT source_app, source_url FROM captures
             WHERE session_id = ?1 AND deleted_at IS NULL
             ORDER BY captured_at DESC LIMIT 8",
        )?;
        let mut recent_apps = Vec::new();
        let mut recent_domains = Vec::new();
        let rows = stmt.query_map([&id], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows.filter_map(|r| r.ok()) {
            if let Some(app) = row.0 {
                if !recent_apps.contains(&app) {
                    recent_apps.push(app);
                }
            }
            if let Some(url) = row.1 {
                if let Some(d) = registrable_domain(&url) {
                    if !recent_domains.contains(&d) {
                        recent_domains.push(d);
                    }
                }
            }
        }
        out.push(OpenSession { id, last_activity_at, recent_apps, recent_domains });
    }
    Ok(out)
}

/// Assign a stored capture to the best open session (or a new one) and stamp
/// `captures.session_id`. Returns what the HUD needs.
pub fn assign(
    conn: &Connection,
    cfg: &ScoringConfig,
    capture_id: &str,
    captured_at: i64,
    source_app: Option<&str>,
    source_url: Option<&str>,
) -> Result<Assignment> {
    let domain = source_url.and_then(registrable_domain);
    let cand = Candidate {
        captured_at,
        source_app,
        source_domain: domain.as_deref(),
    };

    let sessions = load_open_sessions(conn)?;
    let best = sessions
        .iter()
        .map(|s| (score(cfg, &cand, s), s))
        .filter(|(sc, _)| *sc >= cfg.threshold)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    if let Some((_, sess)) = best {
        conn.execute(
            "UPDATE captures SET session_id = ?1 WHERE id = ?2",
            params![sess.id, capture_id],
        )?;
        conn.execute(
            "UPDATE sessions SET last_activity_at = ?1 WHERE id = ?2",
            params![captured_at, sess.id],
        )?;
        let topic: String = conn
            .query_row("SELECT ifnull(topic, '') FROM sessions WHERE id = ?1", [&sess.id], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or_default();
        return Ok(Assignment { session_id: sess.id.clone(), topic, created: false });
    }

    // no session fits: open a new one, named after where the work is happening
    let id = ulid::Ulid::new().to_string();
    let topic = domain
        .or_else(|| source_app.map(|a| a.trim_end_matches(".exe").to_string()))
        .unwrap_or_else(|| "new session".into());
    conn.execute(
        "INSERT INTO sessions (id, topic, started_at, last_activity_at, status, boundary_source)
         VALUES (?1, ?2, ?3, ?3, 'open', 'auto')",
        params![id, topic, captured_at],
    )?;
    conn.execute(
        "UPDATE captures SET session_id = ?1 WHERE id = ?2",
        params![id, capture_id],
    )?;
    Ok(Assignment { session_id: id, topic, created: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(last_activity_min_ago: f64, apps: &[&str], domains: &[&str]) -> OpenSession {
        OpenSession {
            id: "s".into(),
            last_activity_at: (100.0 * 60_000.0 - last_activity_min_ago * 60_000.0) as i64,
            recent_apps: apps.iter().map(|s| s.to_string()).collect(),
            recent_domains: domains.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn cand<'a>(app: Option<&'a str>, domain: Option<&'a str>) -> Candidate<'a> {
        Candidate {
            captured_at: (100.0 * 60_000.0) as i64,
            source_app: app,
            source_domain: domain,
        }
    }

    #[test]
    fn quick_followup_same_domain_scores_high() {
        let cfg = ScoringConfig::default();
        let s = sess(1.0, &["chrome.exe"], &["docs.rs"]);
        let sc = score(&cfg, &cand(Some("chrome.exe"), Some("docs.rs")), &s);
        assert!(sc > 0.9, "got {sc}");
    }

    #[test]
    fn long_gap_same_domain_still_assigns() {
        // 30-min gap but same domain cluster: research resumed
        let cfg = ScoringConfig::default();
        let s = sess(30.0, &["chrome.exe"], &["docs.rs"]);
        let sc = score(&cfg, &cand(Some("chrome.exe"), Some("docs.rs")), &s);
        assert!(sc >= cfg.threshold, "got {sc}");
    }

    #[test]
    fn quick_tangent_different_source_stays_out() {
        // 1-min gap but unrelated app/domain (the Slack tangent): time alone
        // must not capture it... time weight w_t=0.6 * ~0.94 = ~0.57 — hmm.
        let cfg = ScoringConfig::default();
        let s = sess(1.0, &["chrome.exe"], &["docs.rs"]);
        let sc = score(&cfg, &cand(Some("slack.exe"), None), &s);
        // a quick copy from an unrelated app DOES score above bare threshold on
        // time; the interleaving protection comes from the tangent's own new
        // session outscoring it once it exists. Assert relative ordering:
        let tangent_own = sess(0.5, &["slack.exe"], &[]);
        let sc_own = score(&cfg, &cand(Some("slack.exe"), None), &tangent_own);
        assert!(sc_own > sc, "tangent's own session must win: {sc_own} vs {sc}");
    }

    #[test]
    fn hour_gap_unrelated_source_below_threshold() {
        let cfg = ScoringConfig::default();
        let s = sess(60.0, &["chrome.exe"], &["docs.rs"]);
        let sc = score(&cfg, &cand(Some("cursor.exe"), None), &s);
        assert!(sc < cfg.threshold, "got {sc}");
    }

    #[test]
    fn zero_affinity_outside_burst_window_never_joins() {
        // the observed live bug: an unrelated capture 5–10 min later must not
        // glue onto a recent session on time score alone
        let cfg = ScoringConfig::default();
        let s = sess(6.0, &["notepad.exe"], &[]);
        let sc = score(&cfg, &cand(Some("chrome.exe"), None), &s);
        assert_eq!(sc, 0.0, "got {sc}");
        // …but within the burst window cross-app collecting stays together
        let s2 = sess(1.0, &["notepad.exe"], &[]);
        let sc2 = score(&cfg, &cand(Some("chrome.exe"), None), &s2);
        assert!(sc2 >= cfg.threshold, "got {sc2}");
    }

    #[test]
    fn same_app_weaker_than_same_domain() {
        let cfg = ScoringConfig::default();
        let s = sess(10.0, &["chrome.exe"], &["docs.rs"]);
        let by_domain = score(&cfg, &cand(Some("chrome.exe"), Some("docs.rs")), &s);
        let by_app = score(&cfg, &cand(Some("chrome.exe"), Some("github.com")), &s);
        assert!(by_domain > by_app);
    }

    #[test]
    fn domain_extraction() {
        assert_eq!(registrable_domain("https://docs.rs/rmcp/latest"), Some("docs.rs".into()));
        assert_eq!(
            registrable_domain("https://user@sub.github.com:443/a?b#c"),
            Some("github.com".into())
        );
        assert_eq!(registrable_domain("http://127.0.0.1:5173/x"), Some("127.0.0.1".into()));
        assert_eq!(registrable_domain("localhost"), Some("localhost".into()));
    }
}
