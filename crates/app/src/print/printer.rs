//! The system's printers, through CUPS's command-line tools — `lpstat` lists them, `lp` prints — which Linux
//! and macOS both have. ⚠ Run through a [`Runner`], so no test ever reaches a real printer.

use super::layout::Paper;
use std::path::Path;

/// Runs a program, answering whether it succeeded and what it printed (stdout, then stderr).
pub trait Runner {
    fn run(&self, prog: &str, args: &[String]) -> std::io::Result<(bool, String)>;
}

/// The real one.
pub struct System;

impl Runner for System {
    fn run(&self, prog: &str, args: &[String]) -> std::io::Result<(bool, String)> {
        let out = std::process::Command::new(prog).args(args).output()?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((out.status.success(), text))
    }
}

/// The printers CUPS knows, by name — none when `lpstat` is missing or fails (no CUPS, no printer).
pub fn printers(r: &dyn Runner) -> Vec<String> {
    match r.run("lpstat", &["-e".to_string()]) {
        Ok((true, out)) => out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.contains(char::is_whitespace))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// `lp`'s arguments: the printer, the copies, the paper, and the file.
pub fn lp_args(printer: &str, copies: u16, paper: Paper, file: &Path) -> Vec<String> {
    vec![
        "-d".into(),
        printer.into(),
        "-n".into(),
        copies.max(1).to_string(),
        "-o".into(),
        format!("media={}", paper.media()),
        "--".into(),
        file.to_string_lossy().into_owned(),
    ]
}

/// Send `file` to `printer`; an error says what `lp` said.
pub fn send(
    r: &dyn Runner,
    printer: &str,
    copies: u16,
    paper: Paper,
    file: &Path,
) -> Result<(), String> {
    match r.run("lp", &lp_args(printer, copies, paper, file)) {
        Ok((true, _)) => Ok(()),
        Ok((false, out)) => Err(format!("the printer refused it: {}", out.trim())),
        Err(e) => Err(format!("could not run lp: {e}")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A constructed CUPS: answers each program with a fixed reply and records what it was asked.
    pub(crate) struct Fake {
        pub(crate) lpstat: Option<(bool, String)>,
        pub(crate) lp: (bool, String),
        pub(crate) asked: RefCell<Vec<(String, Vec<String>)>>,
    }

    impl Runner for Fake {
        fn run(&self, prog: &str, args: &[String]) -> std::io::Result<(bool, String)> {
            self.asked
                .borrow_mut()
                .push((prog.to_owned(), args.to_vec()));
            match prog {
                "lpstat" => self
                    .lpstat
                    .clone()
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound)),
                _ => Ok(self.lp.clone()),
            }
        }
    }

    fn fake(lpstat: Option<(bool, &str)>) -> Fake {
        Fake {
            lpstat: lpstat.map(|(ok, s)| (ok, s.to_owned())),
            lp: (true, "request id is Canon-7 (1 file(s))".into()),
            asked: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn the_printers_are_lpstats_lines() {
        let f = fake(Some((true, "Canon_G620\nOffice_Laser\n\n")));
        assert_eq!(printers(&f), vec!["Canon_G620", "Office_Laser"]);
        assert_eq!(
            f.asked.borrow()[0],
            ("lpstat".into(), vec!["-e".to_string()])
        );
    }

    #[test]
    fn no_cups_or_a_failing_lpstat_is_no_printer() {
        assert!(printers(&fake(None)).is_empty());
        // What `lpstat -e` says with CUPS running and nothing set up.
        assert!(printers(&fake(Some((false, "lpstat: No destinations added.")))).is_empty());
    }

    #[test]
    fn lp_is_asked_for_the_printer_copies_paper_and_file() {
        let f = fake(Some((true, "")));
        send(&f, "Canon_G620", 2, Paper::In4x6, Path::new("/x/print.pdf")).unwrap();
        let asked = f.asked.borrow();
        assert_eq!(asked[0].0, "lp");
        assert_eq!(
            asked[0].1,
            [
                "-d",
                "Canon_G620",
                "-n",
                "2",
                "-o",
                "media=na_index-4x6_4x6in",
                "--",
                "/x/print.pdf"
            ]
        );
    }

    #[test]
    fn a_refusing_lp_is_an_error_carrying_what_it_said() {
        let mut f = fake(Some((true, "")));
        f.lp = (false, "lp: The printer or class does not exist.\n".into());
        let e = send(&f, "Gone", 1, Paper::A4, Path::new("p.pdf")).unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
    }
}
