//! State behind the status bar and the confirm-in-place buttons. Pure: no egui and no clock of its
//! own (callers pass the `Instant`), so the rules can be unit-tested.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a control stays armed before it falls back to its first look.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(4);

/// The controls that need a second click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Armed {
    /// Cancel on the running job (status bar and Upscale view).
    JobCancel,
    /// Stop Capture.
    StopCapture,
    /// Stop it, on a capture that outlived an earlier run.
    StopOrphan,
}

/// Which control, if any, has had its first click. Only one at a time.
#[derive(Default)]
pub struct Confirm {
    armed: Option<(Armed, Instant)>,
}

impl Confirm {
    /// First click: arm `what`, replacing whatever was armed.
    pub fn arm(&mut self, what: Armed, now: Instant) {
        self.armed = Some((what, now + CONFIRM_WINDOW));
    }

    /// Clicking the safe button, pressing Escape, or a successful confirm.
    pub fn disarm(&mut self) {
        self.armed = None;
    }

    /// True while `what` is armed. An arm that has run out is dropped here.
    pub fn is_armed(&mut self, what: Armed, now: Instant) -> bool {
        match self.armed {
            Some((_, until)) if now >= until => {
                self.armed = None;
                false
            }
            Some((armed, _)) => armed == what,
            None => false,
        }
    }

    /// Time until the armed control disarms, to schedule a repaint.
    pub fn expires_in(&self, now: Instant) -> Option<Duration> {
        self.armed
            .map(|(_, until)| until.saturating_duration_since(now))
    }
}

/// What the status bar shows after "left".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remaining {
    /// Not enough progress yet, or the rate is erratic: show nothing rather than a guess.
    Unknown,
    Paused,
    Secs(u64),
}

/// Estimates the time left from the rate of progress, over the last two minutes.
#[derive(Default)]
pub struct TimeLeft {
    samples: VecDeque<(Instant, f32)>,
}

/// Samples closer together than this are skipped.
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
/// Only the last two minutes count, so a slow start does not drag the estimate.
const WINDOW: Duration = Duration::from_secs(120);
/// Nothing is shown before this much time and progress have been seen.
const MIN_SPAN: Duration = Duration::from_secs(10);
const MIN_DELTA: f32 = 0.005;

impl TimeLeft {
    /// Forget everything (a new job).
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    /// Record the job's overall progress, `0.0..=1.0`. Progress going backwards (a reattached job
    /// recounting, a restart) starts the estimate again.
    pub fn record(&mut self, now: Instant, fraction: f32) {
        if let Some(&(_, last)) = self.samples.back()
            && fraction + 0.001 < last
        {
            self.samples.clear();
        }
        if let Some(&(t, _)) = self.samples.back()
            && now.saturating_duration_since(t) < SAMPLE_EVERY
        {
            return;
        }
        self.samples.push_back((now, fraction));
        while let Some(&(t, _)) = self.samples.front() {
            if now.saturating_duration_since(t) > WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn remaining(&self, paused: bool) -> Remaining {
        if paused {
            return Remaining::Paused;
        }
        let (Some(&(t0, p0)), Some(&(t1, p1))) = (self.samples.front(), self.samples.back()) else {
            return Remaining::Unknown;
        };
        let span = t1.saturating_duration_since(t0);
        if self.samples.len() < 3 || span < MIN_SPAN || p1 - p0 < MIN_DELTA {
            return Remaining::Unknown;
        }
        let rate = (p1 - p0) / span.as_secs_f32();
        // Erratic: the rate over the first and second half of the window differ by more than 2x.
        let mid = self.samples[self.samples.len() / 2];
        let (first, second) = (
            (mid.1 - p0) / mid.0.saturating_duration_since(t0).as_secs_f32().max(0.001),
            (p1 - mid.1) / t1.saturating_duration_since(mid.0).as_secs_f32().max(0.001),
        );
        if first <= 0.0 || second <= 0.0 || (first / second).max(second / first) > 2.0 {
            return Remaining::Unknown;
        }
        Remaining::Secs(((1.0 - p1).max(0.0) / rate).round() as u64)
    }
}

/// Where a capture stands against its stop time: `(stop time in seconds, fraction done)`. The stop
/// time is the elapsed time plus what is left; `None` when no stop time is set.
pub fn capture_progress(elapsed: u64, remaining: Option<u64>) -> Option<(u64, f32)> {
    let total = elapsed + remaining?;
    if total == 0 {
        return None;
    }
    Some((total, (elapsed as f32 / total as f32).min(1.0)))
}

/// `HH:MM:SS`, as in the capture clock.
pub fn hms(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, s: u64) -> Instant {
        base + Duration::from_secs(s)
    }

    #[test]
    fn a_control_stays_armed_for_four_seconds_then_falls_back() {
        let t0 = Instant::now();
        let mut c = Confirm::default();
        assert!(!c.is_armed(Armed::JobCancel, t0));
        c.arm(Armed::JobCancel, t0);
        assert!(c.is_armed(Armed::JobCancel, at(t0, 3)));
        assert_eq!(c.expires_in(at(t0, 3)), Some(Duration::from_secs(1)));
        assert!(!c.is_armed(Armed::JobCancel, at(t0, 4)), "expired at 4 s");
        assert!(
            !c.is_armed(Armed::JobCancel, at(t0, 5)),
            "and stays disarmed"
        );
        assert_eq!(c.expires_in(at(t0, 5)), None);
    }

    #[test]
    fn only_one_control_is_armed_at_a_time() {
        let t0 = Instant::now();
        let mut c = Confirm::default();
        c.arm(Armed::JobCancel, t0);
        c.arm(Armed::StopCapture, at(t0, 1));
        assert!(!c.is_armed(Armed::JobCancel, at(t0, 1)));
        assert!(c.is_armed(Armed::StopCapture, at(t0, 1)));
        // Arming restarts the window.
        assert!(c.is_armed(Armed::StopCapture, at(t0, 4)));
        assert!(!c.is_armed(Armed::StopCapture, at(t0, 5)));
    }

    #[test]
    fn the_safe_button_disarms() {
        let t0 = Instant::now();
        let mut c = Confirm::default();
        c.arm(Armed::StopOrphan, t0);
        c.disarm();
        assert!(!c.is_armed(Armed::StopOrphan, at(t0, 1)));
    }

    /// Feeds 1 %/s for `secs` seconds starting from 0.
    fn steady(tl: &mut TimeLeft, base: Instant, secs: u64) {
        for s in 0..=secs {
            tl.record(at(base, s), s as f32 / 100.0);
        }
    }

    #[test]
    fn nothing_is_shown_until_enough_time_and_progress_have_passed() {
        let t0 = Instant::now();
        let mut tl = TimeLeft::default();
        assert_eq!(tl.remaining(false), Remaining::Unknown);
        steady(&mut tl, t0, 9);
        assert_eq!(tl.remaining(false), Remaining::Unknown, "under 10 s");
        steady(&mut tl, t0, 10);
        assert_ne!(tl.remaining(false), Remaining::Unknown);
    }

    #[test]
    fn a_steady_rate_gives_the_time_left() {
        let t0 = Instant::now();
        let mut tl = TimeLeft::default();
        steady(&mut tl, t0, 20); // 20 %, 1 %/s
        assert_eq!(tl.remaining(false), Remaining::Secs(80));
    }

    #[test]
    fn paused_says_paused_and_forgets_nothing() {
        let t0 = Instant::now();
        let mut tl = TimeLeft::default();
        steady(&mut tl, t0, 20);
        assert_eq!(tl.remaining(true), Remaining::Paused);
        assert_eq!(tl.remaining(false), Remaining::Secs(80));
    }

    #[test]
    fn progress_going_backwards_starts_again() {
        let t0 = Instant::now();
        let mut tl = TimeLeft::default();
        steady(&mut tl, t0, 20);
        tl.record(at(t0, 21), 0.0);
        assert_eq!(tl.remaining(false), Remaining::Unknown);
    }

    #[test]
    fn an_erratic_rate_shows_nothing() {
        let t0 = Instant::now();
        let mut tl = TimeLeft::default();
        // Nothing for 15 s, then a 20 % jump: the two halves disagree wildly.
        for s in 0..15 {
            tl.record(at(t0, s), 0.0);
        }
        for s in 15..=20 {
            tl.record(at(t0, s), (s - 14) as f32 * 0.04);
        }
        assert_eq!(tl.remaining(false), Remaining::Unknown);
    }

    #[test]
    fn a_job_that_stalls_shows_nothing_and_reset_clears_it() {
        let t0 = Instant::now();
        let mut tl = TimeLeft::default();
        for s in 0..=30 {
            tl.record(at(t0, s), 0.5);
        }
        assert_eq!(tl.remaining(false), Remaining::Unknown);
        steady(&mut tl, at(t0, 40), 20);
        tl.reset();
        assert_eq!(tl.remaining(false), Remaining::Unknown);
    }

    #[test]
    fn capture_progress_is_elapsed_over_elapsed_plus_remaining() {
        // 1:12:44 elapsed with 1:17:16 left stops at 2:30:00, 48.5 % through.
        let (total, frac) = capture_progress(4364, Some(4636)).unwrap();
        assert_eq!(hms(total), "02:30:00");
        assert!((frac - 0.4849).abs() < 0.001);
        assert_eq!(capture_progress(10, None), None, "no stop time set");
        assert_eq!(capture_progress(0, Some(0)), None);
        assert_eq!(capture_progress(100, Some(0)).unwrap().1, 1.0);
    }

    #[test]
    fn hms_is_zero_padded() {
        assert_eq!(hms(0), "00:00:00");
        assert_eq!(hms(3725), "01:02:05");
        assert_eq!(hms(11 * 3600 + 5), "11:00:05");
    }
}
