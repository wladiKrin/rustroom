//! The tethered session: one thread that owns the camera, downloads each new file into a folder and
//! reports it. The app talks to it only through [`Session`]'s channels.

use crate::hold::Hold;
use crate::name::{free_name, part_of};
use crate::{Camera, CameraFile, Opener};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::Duration;

/// How long one wait for a camera event lasts — the longest a Shoot or a Stop waits to be read.
const POLL: Duration = Duration::from_millis(200);

/// What the session tells the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The camera is open; its model.
    Connected(String),
    /// A shot is downloaded, complete, at this path.
    Landed(PathBuf),
    /// A sentence for the status row; the session goes on.
    Failed(String),
    /// The session is over: why, or `None` after a Stop. ⚠ A thread gone without one reads the same
    /// (the receiver disconnects) — the app treats that as `Ended(None)`.
    Ended(Option<String>),
}

enum Command {
    Shoot,
    Stop,
}

/// A running session.
pub struct Session {
    commands: Sender<Command>,
    events: Receiver<Event>,
}

impl Session {
    /// Open the camera on a new thread and download every new file into `dir` (which must exist).
    pub fn start(open: Opener, dir: PathBuf) -> Self {
        Self::start_holding(open, dir, Hold::none())
    }

    /// [`Session::start`] holding `hold` from before the camera opens until the session ends — a real
    /// session's [`Hold::photo_service`].
    pub fn start_holding(open: Opener, dir: PathBuf, hold: Hold) -> Self {
        let (commands, command_rx) = channel();
        let (tx, events) = channel();
        std::thread::Builder::new()
            .name("tether".into())
            .spawn(move || {
                let _hold = hold;
                run(open, &dir, &command_rx, &tx)
            })
            .expect("spawn the tether thread");
        Self { commands, events }
    }

    /// A session whose thread is gone without a last word — the panic case, for tests.
    pub fn ended() -> Self {
        let (commands, _) = channel();
        let (_, events) = channel();
        Self { commands, events }
    }

    /// Fire the shutter.
    pub fn shoot(&self) {
        let _ = self.commands.send(Command::Shoot);
    }

    /// End the session — after any download already running completes.
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }
}

fn run(open: Opener, dir: &Path, commands: &Receiver<Command>, tx: &Sender<Event>) {
    let mut cam = match open() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Event::Ended(Some(e)));
            return;
        }
    };
    let _ = tx.send(Event::Connected(cam.name()));
    let mut stems = HashMap::new();
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Shoot) => match cam.shoot() {
                    Ok(files) => {
                        for f in files {
                            land(cam.as_mut(), &f, dir, &mut stems, tx);
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Failed(format!("The camera did not shoot: {e}")));
                    }
                },
                // ⚠ The app dropping its handle is a Stop too, or the thread would hold the camera.
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                    let _ = tx.send(Event::Ended(None));
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        match cam.wait(POLL) {
            Ok(Some(f)) => land(cam.as_mut(), &f, dir, &mut stems, tx),
            Ok(None) => {}
            Err(e) => {
                let _ = tx.send(Event::Ended(Some(e)));
                return;
            }
        }
    }
}

/// Download `f` beside its final name and rename it only when complete — a pulled cable leaves no
/// half photo under a photo's name.
fn land(
    cam: &mut dyn Camera,
    f: &CameraFile,
    dir: &Path,
    stems: &mut HashMap<String, String>,
    tx: &Sender<Event>,
) {
    let dest = free_name(dir, &f.name, stems);
    let part = part_of(&dest);
    let done = cam
        .fetch(f, &part)
        .and_then(|()| std::fs::rename(&part, &dest).map_err(|e| e.to_string()));
    let _ = tx.send(match done {
        Ok(()) => Event::Landed(dest),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Event::Failed(format!("Could not download {}: {e}", f.name))
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{unopenable, Fake, Step};
    use std::sync::atomic::Ordering;

    /// The 120 s ceiling against a hang — never a time budget.
    const LONG: Duration = Duration::from_secs(120);

    fn next(s: &Session) -> Event {
        s.events().recv_timeout(LONG).expect("an event")
    }

    fn jpg(name: &str, b: u8) -> Step {
        Step::File(name.into(), vec![b])
    }

    fn files_in(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn two_shots_land_under_their_own_names() {
        let d = test_dir::TempDir::new("tether-two");
        let s = Session::start(
            Fake::new("Fujifilm X-T4", vec![jpg("A.JPG", 1), jpg("B.JPG", 2)]).opener(),
            d.path().to_path_buf(),
        );
        assert_eq!(next(&s), Event::Connected("Fujifilm X-T4".into()));
        assert_eq!(next(&s), Event::Landed(d.join("A.JPG")));
        assert_eq!(next(&s), Event::Landed(d.join("B.JPG")));
        assert_eq!(std::fs::read(d.join("B.JPG")).unwrap(), vec![2]);
        s.stop();
        assert_eq!(next(&s), Event::Ended(None));
    }

    #[test]
    fn a_taken_name_steps_to_dash_two_and_leaves_no_part_file() {
        let d = test_dir::TempDir::new("tether-taken");
        std::fs::write(d.join("A.JPG"), b"earlier").unwrap();
        let s = Session::start(
            Fake::new("X", vec![jpg("A.JPG", 7)]).opener(),
            d.path().into(),
        );
        next(&s);
        assert_eq!(next(&s), Event::Landed(d.join("A-2.JPG")));
        assert_eq!(
            std::fs::read(d.join("A.JPG")).unwrap(),
            b"earlier",
            "overwritten"
        );
        assert_eq!(files_in(d.path()), ["A-2.JPG", "A.JPG"]);
        s.stop();
    }

    #[test]
    fn shoot_reaches_the_camera_and_its_files_land() {
        let d = test_dir::TempDir::new("tether-shoot");
        let fake = Fake::new("X", vec![]);
        let shots = fake.shots.clone();
        let s = Session::start(fake.opener(), d.path().into());
        next(&s);
        s.shoot();
        assert_eq!(next(&s), Event::Landed(d.join("SHOT1.JPG")));
        assert_eq!(shots.load(Ordering::SeqCst), 1);
        s.stop();
        assert_eq!(next(&s), Event::Ended(None));
    }

    /// Review focus 2: a download that breaks leaves nothing behind, and the session goes on.
    #[test]
    fn a_fetch_error_is_failed_and_the_session_goes_on() {
        let d = test_dir::TempDir::new("tether-broken");
        let s = Session::start(
            Fake::new("X", vec![Step::Broken("A.RAF".into()), jpg("B.JPG", 1)]).opener(),
            d.path().into(),
        );
        next(&s);
        match next(&s) {
            Event::Failed(m) => assert!(m.starts_with("Could not download A.RAF"), "{m}"),
            e => panic!("{e:?}"),
        }
        assert_eq!(next(&s), Event::Landed(d.join("B.JPG")));
        assert_eq!(files_in(d.path()), ["B.JPG"], "a part file was left");
        s.stop();
    }

    #[test]
    fn a_wait_error_ends_the_session_with_its_sentence() {
        let d = test_dir::TempDir::new("tether-pulled");
        let s = Session::start(
            Fake::new(
                "X",
                vec![Step::WaitError("The camera was disconnected".into())],
            )
            .opener(),
            d.path().into(),
        );
        next(&s);
        assert_eq!(
            next(&s),
            Event::Ended(Some("The camera was disconnected".into()))
        );
    }

    #[test]
    fn a_camera_that_never_opens_ends_at_once() {
        let d = test_dir::TempDir::new("tether-unopenable");
        let s = Session::start(unopenable("No camera found"), d.path().into());
        assert_eq!(next(&s), Event::Ended(Some("No camera found".into())));
    }

    #[test]
    fn a_dropped_handle_stops_the_thread() {
        let d = test_dir::TempDir::new("tether-dropped");
        let s = Session::start(Fake::new("X", vec![]).opener(), d.path().into());
        next(&s);
        let Session { commands, events } = s;
        drop(commands);
        assert_eq!(events.recv_timeout(LONG), Ok(Event::Ended(None)));
    }

    /// Asked for in use: macOS's camera service held the camera, so Start failed. The hold acts BEFORE the
    /// camera is opened, and lets go when the session ends.
    #[test]
    fn a_session_holds_the_service_off_from_before_the_open_until_its_end() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        let d = test_dir::TempDir::new("tether-hold");
        let acted = Arc::new(AtomicUsize::new(0));
        let (a, seen) = (acted.clone(), acted.clone());
        let hold = crate::hold::Hold::every(Duration::from_millis(5), move || {
            a.fetch_add(1, Ordering::SeqCst);
        });
        let open: Opener = Box::new(move || {
            assert!(
                seen.load(Ordering::SeqCst) > 0,
                "the camera opened before the hold acted"
            );
            Fake::new("X", vec![]).opener()()
        });
        let s = Session::start_holding(open, d.path().into(), hold);
        assert_eq!(next(&s), Event::Connected("X".into()));
        // The test's handle and the hold thread's — the opener's is spent.
        assert_eq!(
            Arc::strong_count(&acted),
            2,
            "the hold ended before the session did"
        );
        s.stop();
        assert_eq!(next(&s), Event::Ended(None));
        // The hold's thread owns the closure's clone; it is gone once the hold is dropped.
        let t = std::time::Instant::now();
        while Arc::strong_count(&acted) > 1 {
            assert!(t.elapsed() < LONG, "the hold outlived its session");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_session_thread_gone_without_a_word_disconnects() {
        let s = Session::ended();
        assert!(s.events().recv_timeout(LONG).is_err());
    }
}
