//! P2.1 health derivation: 4-bucket worker liveness from
//! (now, last_seen, connected, is_operator). Pure module — no I/O.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "../../packages/protocol/dist/")]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Online,
    RecentlyLost,
    Offline,
    AboutToGc,
}

/// Worker is considered RecentlyLost for the first 5 minutes after WS
/// disconnect; transient network blips fall inside this window.
pub const RECENTLY_LOST_MAX_SECS: i64 = 5 * 60;

/// Beyond 5 minutes the worker is fully Offline; the bucket caps at 6 days
/// so the next bucket (about-to-gc) gets a clean 1-day window.
pub const OFFLINE_MAX_SECS: i64 = 6 * 24 * 60 * 60;

/// 7 days is the conventional GC threshold; the last 24 hours before that
/// surface as AboutToGc so the operator can intervene.
pub const ABOUT_TO_GC_MAX_SECS: i64 = 7 * 24 * 60 * 60;

/// Decide the avatar's Health state. `now_ts` is unix seconds; `last_seen`
/// is the last time we received any worker message (None if never). The
/// operator override exists because the operator avatar has no worker WS —
/// its presence in the avatars map already means the console is connected.
pub fn derive(now_ts: i64, last_seen: Option<i64>, connected: bool, is_operator: bool) -> Health {
    if is_operator {
        return Health::Online;
    }
    if connected {
        return Health::Online;
    }
    let Some(ts) = last_seen else {
        return Health::Offline;
    };
    let delta = now_ts - ts;
    if delta < 0 {
        // Clock skew / NTP step — be charitable.
        return Health::Online;
    }
    if delta <= RECENTLY_LOST_MAX_SECS {
        Health::RecentlyLost
    } else if delta <= OFFLINE_MAX_SECS {
        Health::Offline
    } else if delta <= ABOUT_TO_GC_MAX_SECS {
        Health::AboutToGc
    } else {
        Health::Offline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn online_when_connected() {
        assert_eq!(derive(NOW, Some(NOW - 100), true, false), Health::Online);
        assert_eq!(derive(NOW, None, true, false), Health::Online);
    }

    #[test]
    fn online_when_operator() {
        assert_eq!(derive(NOW, None, false, true), Health::Online);
        assert_eq!(derive(NOW, Some(NOW - 10_000_000), false, true), Health::Online);
    }

    #[test]
    fn offline_when_disconnected_no_last_seen() {
        assert_eq!(derive(NOW, None, false, false), Health::Offline);
    }

    #[test]
    fn recently_lost_within_5min() {
        assert_eq!(derive(NOW, Some(NOW - 1), false, false), Health::RecentlyLost);
        assert_eq!(derive(NOW, Some(NOW - 299), false, false), Health::RecentlyLost);
        assert_eq!(derive(NOW, Some(NOW - 300), false, false), Health::RecentlyLost);
    }

    #[test]
    fn offline_after_5min() {
        assert_eq!(derive(NOW, Some(NOW - 301), false, false), Health::Offline);
        assert_eq!(derive(NOW, Some(NOW - OFFLINE_MAX_SECS), false, false), Health::Offline);
    }

    #[test]
    fn about_to_gc_after_6_days() {
        assert_eq!(
            derive(NOW, Some(NOW - OFFLINE_MAX_SECS - 1), false, false),
            Health::AboutToGc
        );
        assert_eq!(
            derive(NOW, Some(NOW - ABOUT_TO_GC_MAX_SECS), false, false),
            Health::AboutToGc
        );
    }

    #[test]
    fn offline_after_7_days() {
        assert_eq!(
            derive(NOW, Some(NOW - ABOUT_TO_GC_MAX_SECS - 1), false, false),
            Health::Offline
        );
    }

    #[test]
    fn clock_skew_treated_as_online() {
        assert_eq!(derive(NOW, Some(NOW + 10), false, false), Health::Online);
    }
}
