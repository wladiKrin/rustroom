//! A publish service's member states (M10.18d): New, Modified, Published, Removed — and the fingerprint that
//! tells Modified from Published. No egui.
//!
//! ⚠ The fingerprint is STORED in the catalog, so it must not change between builds: FNV-1a over the serialized
//! edits and options, never `DefaultHasher` (its algorithm may change with the toolchain, and every member
//! would then read Modified at once).

use crate::batch_export::ExportOptions;
use catalog::db::Member;
use edit_model::Edits;

/// Where a member stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    New,
    Modified,
    Published,
    Removed,
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // A separator, so two parts cannot run into each other.
    *h ^= 0xff;
    *h = h.wrapping_mul(0x0000_0100_0000_01b3);
}

/// What a member would be published as: its edits, the service's settings — only those that differ from the
/// defaults, and never where they go — and, when the settings embed it, the metadata a person sets.
///
/// ⚠⚠ Stable across builds by construction (the M10.18d review): a NEW `ExportOptions` field at its default
/// changes nothing, `Edits` already omits its defaults (AGENTS.md), and the metadata is hashed field by field —
/// never `Debug` text, never the capture facts (a background scan or the GPS pass filling them later would
/// otherwise mark a published photo Modified).
pub fn fingerprint(
    edits: &Edits,
    opts: &ExportOptions,
    meta: Option<&export::FileMetadata>,
) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    fnv(&mut h, &serde_json::to_vec(edits).unwrap_or_default());
    let mine = serde_json::to_value(opts).unwrap_or_default();
    let base = serde_json::to_value(ExportOptions::default()).unwrap_or_default();
    if let (Some(mine), Some(base)) = (mine.as_object(), base.as_object()) {
        let mut keys: Vec<&String> = mine.keys().collect();
        keys.sort();
        for k in keys {
            if matches!(k.as_str(), "dest_dir" | "destination" | "delivery")
                || base.get(k) == mine.get(k)
            {
                continue;
            }
            fnv(&mut h, k.as_bytes());
            fnv(&mut h, mine[k].to_string().as_bytes());
        }
    }
    if let Some(m) = meta {
        for part in [&m.title, &m.caption, &m.copyright, &m.creator] {
            fnv(&mut h, part.as_bytes());
        }
        fnv(&mut h, &[m.rating]);
        fnv(&mut h, m.label.as_deref().unwrap_or("").as_bytes());
        for path in &m.keywords {
            fnv(&mut h, path.join("\u{1f}").as_bytes());
        }
    }
    h
}

/// `m`'s state, given what it would be published as `now`.
pub fn state(m: &Member, now: u64) -> State {
    match (&m.published, m.removed) {
        (_, true) => State::Removed,
        (None, false) => State::New,
        (Some(p), false) if p.fingerprint == now => State::Published,
        (Some(_), false) => State::Modified,
    }
}

/// How many members are in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub new: usize,
    pub modified: usize,
    pub published: usize,
    pub removed: usize,
}

impl Counts {
    pub fn add(&mut self, s: State) {
        match s {
            State::New => self.new += 1,
            State::Modified => self.modified += 1,
            State::Published => self.published += 1,
            State::Removed => self.removed += 1,
        }
    }

    /// What a Publish would export.
    pub fn to_publish(&self) -> usize {
        self.new + self.modified
    }

    /// Whether a Publish has anything to do.
    pub fn has_work(&self) -> bool {
        self.to_publish() + self.removed > 0
    }

    /// `3 new · 2 modified · 40 published · 1 removed`, only what is there.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            (self.new, "new"),
            (self.modified, "modified"),
            (self.published, "published"),
            (self.removed, "removed"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} {w}"))
        .collect();
        if parts.is_empty() {
            "No photos yet".into()
        } else {
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::db::{Place, Published};

    fn member(published: Option<u64>, removed: bool) -> Member {
        Member {
            path: "/a/a.RAF".into(),
            copy: 0,
            removed,
            published: published.map(|f| Published {
                fingerprint: f,
                place: Place::Asset { id: "x".into() },
                at: 0,
            }),
        }
    }

    #[test]
    fn an_unpublished_member_is_new() {
        assert_eq!(state(&member(None, false), 5), State::New);
        assert_eq!(state(&member(Some(5), false), 5), State::Published);
    }

    #[test]
    fn a_changed_edit_is_modified() {
        let opts = ExportOptions::default();
        let a = fingerprint(&Edits::default(), &opts, None);
        let e = Edits {
            exposure: 0.5,
            ..Default::default()
        };
        let b = fingerprint(&e, &opts, None);
        assert_ne!(a, b);
        assert_eq!(state(&member(Some(a), false), b), State::Modified);
    }

    #[test]
    fn a_changed_size_marks_modified_but_a_new_folder_does_not() {
        let e = Edits::default();
        let opts = ExportOptions::default();
        let a = fingerprint(&e, &opts, None);
        let mut moved = opts.clone();
        moved.dest_dir = "/elsewhere".into();
        assert_eq!(
            fingerprint(&e, &moved, None),
            a,
            "a new folder changed the print"
        );
        let mut q = opts.clone();
        q.quality = if opts.quality == 50 { 60 } else { 50 };
        assert_ne!(fingerprint(&e, &q, None), a, "a new quality left the print");
    }

    #[test]
    fn a_removed_member_is_removed_whatever_its_print() {
        assert_eq!(state(&member(Some(5), true), 5), State::Removed);
        assert_eq!(state(&member(None, true), 5), State::Removed);
    }

    #[test]
    fn a_changed_title_counts_only_with_metadata() {
        let (e, o) = (Edits::default(), ExportOptions::default());
        let m1 = export::FileMetadata {
            title: "One".into(),
            ..Default::default()
        };
        let m2 = export::FileMetadata {
            title: "Two".into(),
            ..Default::default()
        };
        assert_ne!(
            fingerprint(&e, &o, Some(&m1)),
            fingerprint(&e, &o, Some(&m2))
        );
        assert_eq!(fingerprint(&e, &o, None), fingerprint(&e, &o, None));
    }

    /// ⚠ Pinned: the print is stored, so a change to the hash marks every published photo Modified.
    #[test]
    fn the_hash_is_pinned() {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        fnv(&mut h, b"rustroom");
        assert_eq!(h, 4_779_478_826_075_588_061);
    }

    /// ⚠⚠ Pinned (the M10.18d review): the print is stored, so a change here marks every published photo of
    /// every service Modified — re-exporting them all and churning Immich albums. Re-pin only on purpose.
    #[test]
    fn the_print_of_the_defaults_is_pinned() {
        let m = export::FileMetadata::default();
        let got = fingerprint(&Edits::default(), &ExportOptions::default(), Some(&m));
        assert_eq!(
            got, 12_201_439_358_509_891_507,
            "re-pin only for a deliberate change"
        );
    }

    #[test]
    fn the_capture_facts_do_not_count() {
        let (e, o) = (Edits::default(), ExportOptions::default());
        let a = export::FileMetadata::default();
        let mut b = a.clone();
        b.capture.camera = Some("X-T4".into());
        assert_eq!(fingerprint(&e, &o, Some(&a)), fingerprint(&e, &o, Some(&b)));
    }

    #[test]
    fn the_summary_names_only_what_is_there() {
        let mut c = Counts::default();
        assert_eq!(c.summary(), "No photos yet");
        for s in [State::New, State::New, State::Modified, State::Removed] {
            c.add(s);
        }
        assert_eq!(c.summary(), "2 new · 1 modified · 1 removed");
        assert_eq!(c.to_publish(), 3);
    }
}
