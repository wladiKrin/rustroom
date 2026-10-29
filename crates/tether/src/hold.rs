//! Keeping the system's own camera service off the camera for a session (M10.18a, asked for in use).
//!
//! ⚠ On macOS `ptpcamerad` claims every camera the moment it is plugged in, so libgphoto2's claim fails
//! ("Another program is using the camera"), and launchd restarts it within a second of being killed. A
//! [`Hold`] therefore kills it once before the camera is opened and again every [`PERIOD`] until it is
//! dropped — the session's end — after which launchd brings it back on its own. Off macOS it does nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// How often the service is pushed off again — under launchd's restart of about a second.
pub const PERIOD: Duration = Duration::from_millis(500);

/// A running hold; dropping it ends it.
pub struct Hold {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Hold {
    /// No hold at all.
    pub fn none() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Run `act` now, then every `period` on a thread of its own until dropped.
    pub fn every(period: Duration, act: impl Fn() + Send + 'static) -> Self {
        act();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("tether-hold".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    std::thread::park_timeout(period);
                    if !s.load(Ordering::Relaxed) {
                        act();
                    }
                }
            })
            .expect("spawn the tether hold");
        Self {
            stop,
            thread: Some(thread),
        }
    }

    /// The hold a real session takes: macOS's camera service kept off the camera; elsewhere none.
    pub fn photo_service() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::every(PERIOD, push_off_photo_service)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::none()
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

/// Kill the camera service — `ptpcamerad` since macOS 10.15, `PTPCamera` before. It runs as the user, so no
/// password; a name with no such process is a quiet failure.
#[cfg(target_os = "macos")]
fn push_off_photo_service() {
    for name in ["ptpcamerad", "PTPCamera"] {
        let _ = std::process::Command::new("/usr/bin/killall")
            .args(["-9", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    const LONG: Duration = Duration::from_secs(120);

    #[test]
    fn a_hold_acts_at_once_then_again_until_dropped() {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let hold = Hold::every(Duration::from_millis(5), move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(
            n.load(Ordering::SeqCst),
            1,
            "not run before the camera opens"
        );
        let t = Instant::now();
        while n.load(Ordering::SeqCst) < 3 {
            assert!(t.elapsed() < LONG, "the hold never acted again");
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(hold);
        let after = n.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            n.load(Ordering::SeqCst),
            after,
            "the hold went on after it was dropped"
        );
    }

    #[test]
    fn dropping_a_slow_hold_does_not_wait_out_its_period() {
        let hold = Hold::every(Duration::from_secs(60), || {});
        let t = Instant::now();
        drop(hold);
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "the drop waited for the period"
        );
    }
}
