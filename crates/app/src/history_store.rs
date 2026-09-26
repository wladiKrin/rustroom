//! A photo's undo history as text, for the catalog's `photo_history` table (M9.7).
//!
//! ⚠ The catalog stores the body opaquely; this is the one place that knows its shape. Why the
//! catalog and not the sidecar is written down in `catalog`'s `db/history.rs`.

use crate::history::History;
use edit_model::Edits;
use serde::{Deserialize, Serialize};

/// The most undo steps kept per photo, and the most redo steps. The newest are kept.
///
/// ⚠ Bounded because every step is a whole `Edits` — 3.4 KB for a median sidecar, ~44 KB with
/// brush masks — and a photo worked on for an afternoon can accumulate hundreds.
pub const CAP: usize = 100;

/// ⚠ `version` so a future shape can refuse an old body instead of misreading it; `decode` drops
/// anything it does not recognise, which costs only the history, never the edits.
#[derive(Serialize, Deserialize)]
struct Doc {
    version: u32,
    undo: Vec<Edits>,
    current: Edits,
    redo: Vec<Edits>,
}

const VERSION: u32 = 1;

/// Whether `history` holds anything worth storing — anything to undo or redo.
pub fn worth_keeping(history: &History<Edits>) -> bool {
    history.can_undo() || history.can_redo()
}

/// `history` as text, its undo and redo sides each cut to the newest [`CAP`] steps.
pub fn encode(history: &History<Edits>) -> String {
    let newest = |v: &[Edits]| v[v.len().saturating_sub(CAP)..].to_vec();
    let doc = Doc {
        version: VERSION,
        undo: newest(history.undo_steps()),
        current: history.get().clone(),
        // ⚠ The next redo is LAST, so "newest" — nearest the present — is the tail here too.
        redo: newest(history.redo_steps()),
    };
    serde_json::to_string(&doc).expect("Edits always serialises")
}

/// The history in `text`, or `None` for anything unreadable or of another version.
pub fn decode(text: &str) -> Option<History<Edits>> {
    let doc: Doc = serde_json::from_str(text).ok()?;
    (doc.version == VERSION).then(|| History::from_parts(doc.undo, doc.current, doc.redo))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ev: f32) -> Edits {
        Edits {
            exposure: ev,
            ..Default::default()
        }
    }

    /// Ten steps, three undone: seven to undo, three to redo, and the present in between.
    fn worked() -> History<Edits> {
        let mut h = History::new(at(0.0));
        for i in 1..=10 {
            h.push(at(i as f32 / 10.0));
        }
        for _ in 0..3 {
            h.undo();
        }
        h
    }

    #[test]
    fn a_history_round_trips_with_both_sides_and_the_present() {
        let h = worked();
        let back = decode(&encode(&h)).expect("decodes");
        assert_eq!(back.get(), h.get());
        assert_eq!(back.undo_steps(), h.undo_steps());
        assert_eq!(back.redo_steps(), h.redo_steps());
        // ⚠ And it behaves: an undo lands where the original's would.
        let (mut a, mut b) = (h, back);
        assert!(a.undo() && b.undo());
        assert_eq!(a.get(), b.get());
        assert!(a.redo() && b.redo() && a.redo() && b.redo());
        assert_eq!(a.get(), b.get());
    }

    /// ⚠ The NEWEST steps survive the cap — the oldest are what is dropped — on both sides.
    #[test]
    fn the_cap_keeps_the_newest_steps() {
        let mut h = History::new(at(0.0));
        for i in 1..=(CAP + 30) {
            h.push(at(i as f32));
        }
        let back = decode(&encode(&h)).unwrap();
        assert_eq!(back.undo_depth(), CAP);
        assert_eq!(back.undo_steps().last(), h.undo_steps().last());
        assert_eq!(back.undo_steps()[0], at(30.0), "the 30 oldest are dropped");
        assert_eq!(back.get(), h.get());

        let mut r = h;
        for _ in 0..(CAP + 20) {
            r.undo();
        }
        let back = decode(&encode(&r)).unwrap();
        assert_eq!(back.redo_depth(), CAP);
        assert_eq!(
            back.redo_steps().last(),
            r.redo_steps().last(),
            "the next redo is kept"
        );
    }

    #[test]
    fn garbage_and_another_version_decode_to_nothing() {
        assert!(decode("not json").is_none());
        let other = encode(&worked()).replace("\"version\":1", "\"version\":2");
        assert!(decode(&other).is_none());
    }

    #[test]
    fn nothing_to_undo_or_redo_is_not_worth_keeping() {
        assert!(!worth_keeping(&History::new(at(0.0))));
        assert!(worth_keeping(&worked()));
    }
}
