//! Per-route refusal watch. `#1848 vibe-talk-route-health-alert`.
//!
//! The access log says what each request did, but nothing reads it back: a
//! route that refuses every request is invisible until somebody hits it
//! (the Discuss follow-up route answered 422 to 19 of 19 requests over five
//! days, `#1844`). This registry counts, per route, what the access layer
//! already sees — method, path, status — over a fixed window, and evaluates
//! the window when it closes: a route with at least [`MIN_SAMPLE`] requests
//! and no success at all (every answer 4xx or 5xx) raises one alert for that
//! window. Alerts are logged at ERROR and held for the health answer, which
//! is where a coordinator's tick reads them.
//!
//! Route keys fold id-shaped path segments (`{id}`), so two channels count
//! as one route. Counting is one mutex-protected map update per request;
//! the request path and the access log line are unchanged.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// How long one counting window runs.
pub const WINDOW_MS: i64 = 3_600_000;
/// A window smaller than this never alerts, whatever it refused.
pub const MIN_SAMPLE: u64 = 5;

/// What one finished window found wrong with one route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAlert {
    /// The route key: `METHOD /template/with/{id}`.
    pub route: String,
    /// Requests in the window.
    pub total: u64,
    /// Of them, 4xx answers.
    pub client_errors: u64,
    /// Of them, 5xx answers.
    pub server_errors: u64,
    /// The status answered most often.
    pub dominant_status: u16,
}

/// One route's counts, as the health answer shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteCounts {
    /// The route key.
    pub route: String,
    /// Requests in the current window.
    pub total: u64,
    /// Of them, 4xx answers.
    pub client_errors: u64,
    /// Of them, 5xx answers.
    pub server_errors: u64,
    /// The most recent status answered.
    pub last_status: u16,
}

#[derive(Default)]
struct Counts {
    total: u64,
    client_errors: u64,
    server_errors: u64,
    last_status: u16,
    by_status: BTreeMap<u16, u64>,
}

impl Counts {
    fn record(&mut self, status: u16) {
        self.total += 1;
        match status {
            400..=499 => self.client_errors += 1,
            500..=599 => self.server_errors += 1,
            _ => {}
        }
        self.last_status = status;
        *self.by_status.entry(status).or_default() += 1;
    }

    fn successes(&self) -> u64 {
        self.total - self.client_errors - self.server_errors
    }

    fn dominant_status(&self) -> u16 {
        self.by_status
            .iter()
            .max_by_key(|(_, count)| *count)
            .map_or(self.last_status, |(status, _)| *status)
    }
}

#[derive(Default)]
struct Window {
    started_ms: Option<i64>,
    routes: BTreeMap<String, Counts>,
}

/// The registry the access layer counts into. One per server.
#[derive(Default)]
pub struct RouteHealth {
    current: Mutex<Window>,
    last_alerts: Mutex<Vec<RouteAlert>>,
}

impl RouteHealth {
    /// A fresh registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Count one answered `/api/` request.
    pub fn record(&self, method: &str, path: &str, status: u16, now_ms: i64) {
        let key = route_key(method, path);
        let mut window = self.current.lock().expect("route health window");
        if window.started_ms.is_none() {
            window.started_ms = Some(now_ms);
        }
        window.routes.entry(key).or_default().record(status);
    }

    /// Close the window when it has run its length and return its alerts,
    /// starting a fresh window. Inside the window, returns nothing and
    /// changes nothing. Each route alerts at most once per window because
    /// evaluation is the act that closes it.
    pub fn evaluate(&self, now_ms: i64) -> Vec<RouteAlert> {
        let finished = {
            let mut window = self.current.lock().expect("route health window");
            let Some(started) = window.started_ms else {
                return Vec::new();
            };
            if now_ms - started < WINDOW_MS {
                return Vec::new();
            }
            window.started_ms = Some(now_ms);
            std::mem::take(&mut window.routes)
        };
        let alerts: Vec<RouteAlert> = finished
            .into_iter()
            .filter(|(_, counts)| counts.total >= MIN_SAMPLE && counts.successes() == 0)
            .map(|(route, counts)| RouteAlert {
                dominant_status: counts.dominant_status(),
                route,
                total: counts.total,
                client_errors: counts.client_errors,
                server_errors: counts.server_errors,
            })
            .collect();
        *self.last_alerts.lock().expect("route health alerts") = alerts.clone();
        alerts
    }

    /// The current window's counts, busiest first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<RouteCounts> {
        let window = self.current.lock().expect("route health window");
        let mut routes: Vec<RouteCounts> = window
            .routes
            .iter()
            .map(|(route, counts)| RouteCounts {
                route: route.clone(),
                total: counts.total,
                client_errors: counts.client_errors,
                server_errors: counts.server_errors,
                last_status: counts.last_status,
            })
            .collect();
        routes.sort_by(|a, b| b.total.cmp(&a.total).then(a.route.cmp(&b.route)));
        routes
    }

    /// The last finished window's alerts.
    #[must_use]
    pub fn alerts(&self) -> Vec<RouteAlert> {
        self.last_alerts.lock().expect("route health alerts").clone()
    }
}

/// One path segment folded into a route template: an id is all digits and
/// at least six of them (a snowflake or a numeric id), or a UUID shape.
/// Names and short numbers (`v1`, `42`) stay as they are.
fn is_id_segment(segment: &str) -> bool {
    if segment.len() >= 6 && segment.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    segment.len() == 36
        && segment.bytes().filter(|byte| *byte == b'-').count() == 4
        && segment
            .bytes()
            .all(|byte| byte == b'-' || byte.is_ascii_hexdigit())
}

/// The counting key of one request: method plus folded path.
#[must_use]
pub fn route_key(method: &str, path: &str) -> String {
    let mut key = String::with_capacity(method.len() + path.len() + 8);
    key.push_str(method);
    for segment in path.split('/') {
        key.push('/');
        key.push_str(if is_id_segment(segment) {
            "{id}"
        } else {
            segment
        });
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_all(health: &RouteHealth, method: &str, path: &str, statuses: &[u16]) {
        for (index, status) in statuses.iter().enumerate() {
            health.record(method, path, *status, 1_000 + index as i64);
        }
    }

    #[test]
    fn an_all_refused_route_alerts_once_the_window_closes() {
        let health = RouteHealth::new();
        record_all(
            &health,
            "POST",
            "/api/v1/channels/100000000000000001/catchup/messages",
            &[422; 6],
        );
        assert!(health.evaluate(1_000 + WINDOW_MS - 1).is_empty());
        let alerts = health.evaluate(1_000 + WINDOW_MS);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].total, 6);
        assert_eq!(alerts[0].client_errors, 6);
        assert_eq!(alerts[0].dominant_status, 422);
        assert_eq!(
            alerts[0].route,
            "POST /api/v1/channels/{id}/catchup/messages"
        );
        // The next window starts empty: no second alert for the same refusals.
        assert!(health.evaluate(1_000 + 2 * WINDOW_MS).is_empty());
    }

    #[test]
    fn a_route_with_any_success_or_too_few_requests_never_alerts() {
        let health = RouteHealth::new();
        record_all(&health, "GET", "/api/v1/channels", &[200, 404, 404, 404, 404, 404]);
        record_all(&health, "POST", "/api/v1/channels/{id}/reply", &[500; 4]);
        assert!(health.evaluate(1_000 + WINDOW_MS).is_empty());
    }

    #[test]
    fn ids_fold_and_names_stay() {
        assert_eq!(
            route_key("GET", "/api/v1/channels/100000000000000001/timeline"),
            "GET /api/v1/channels/{id}/timeline"
        );
        assert_eq!(
            route_key("GET", "/api/v1/channels/200000000000000009/timeline"),
            "GET /api/v1/channels/{id}/timeline"
        );
        assert_eq!(route_key("GET", "/api/v1/search"), "GET /api/v1/search");
        let health = RouteHealth::new();
        record_all(
            &health,
            "POST",
            "/api/v1/channels/100000000000000001/catchup/messages",
            &[422; 3],
        );
        record_all(
            &health,
            "POST",
            "/api/v1/channels/200000000000000009/catchup/messages",
            &[422; 3],
        );
        let snapshot = health.snapshot();
        assert_eq!(snapshot.len(), 1, "{snapshot:?}");
        assert_eq!(snapshot[0].total, 6, "two channels count as one route");
    }
}
