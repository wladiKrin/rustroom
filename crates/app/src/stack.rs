//! Grouping same-stem files — a RAW and its JPEG — into one row.
//!
//! ⚠⚠ **A view grouping and nothing more.** Edits, sidecars, marks and the catalog are untouched:
//! every file keeps its own everything. Shared *edits* were never on the table and should not be
//! revisited — a RAW needs demosaic and white balance while a JPEG is already rendered, so one
//! `Edits` cannot mean the same thing on both. Shared *marks* were offered and declined.
//!
//! ⚠⚠ **This runs in ONE place, and three bugs are why.** The grid, the filmstrip, `selection`,
//! `view_region` and `format_filter` all read the same `Vec<PhotoEntry>`; `entries`' own doc records
//! that this path "has produced three sparse-index bugs" and `format_filter`'s that "a wrong answer
//! here is a photo that silently is not there". Grouping in the grid alone would make the filmstrip
//! and the selection disagree with it. So [`group`] returns a `Vec<PhotoEntry>` and every consumer
//! is untouched.
//!
//! ⚠ Pure and `egui`-free, like `format_filter` and `view_region`, for the same reason: the index
//! bookkeeping is where an off-by-one hides.

use crate::entries::PhotoEntry;
use crate::format_filter::FormatFilter;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What identifies a stack: the containing folder and the upper-cased file stem.
///
/// ⚠⚠ **The folder is part of the key, and the stem alone is safe here — which it was NOT in the
/// Immich stacking migration one day earlier.** There, `DSCF0236` named two different photographs
/// because the camera's counter had wrapped across a 16,000-asset library, and pairing needed
/// `(stem, capture time)`. **Within one folder the filesystem enforces uniqueness**: `DSCF0236.JPG`
/// can exist only once.
///
/// ⚠ The residual risk is two wrapped rolls merged into one folder. Accepted, because this is
/// **view-only**: a wrong pairing is visually confusing and one click from being undone, not a
/// photograph hidden from a timeline. A different bar, deliberately.
pub type StackKey = (PathBuf, String);

/// Which stack a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stack {
    /// How many files share this stem. Always ≥ 2 — a lone file is `stack: None`, never a stack of
    /// one, so the badge never reads "1".
    pub count: usize,
    /// True on **every** member of an expanded stack; false on the single row of a collapsed one.
    pub expanded: bool,
}

/// Which member a collapsed stack shows and opens.
///
/// ⚠⚠ **Append variants, never insert** — serialised into `Settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum StackPrimary {
    /// The edited member; ⚠ **if several are edited, the RAW**; if none, the RAW.
    ///
    /// ⚠ The user's default, chosen knowing the consequence: a cell **changes which file it
    /// represents** when the non-RAW member is edited. That only happens if the JPEG is edited,
    /// which is rarer and is arguably what you want to see afterwards.
    #[default]
    EditedElseRaw,
    /// Always the RAW, edits ignored.
    Raw,
    /// Always the JPEG, edits ignored.
    Jpeg,
}

impl StackPrimary {
    pub const ALL: [StackPrimary; 3] = [
        StackPrimary::EditedElseRaw,
        StackPrimary::Raw,
        StackPrimary::Jpeg,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StackPrimary::EditedElseRaw => "Edited, else RAW",
            StackPrimary::Raw => "RAW",
            StackPrimary::Jpeg => "JPEG",
        }
    }
}

/// How the browser groups same-stem files.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Stacking {
    /// Whether stacking happens at all.
    ///
    /// ⚠⚠ **Defaults to `true`, which is the `gpu_tail` trap in a new place.** `Settings` carries a
    /// container-level `#[serde(default)]`, so a missing field takes whatever `Default` says — and
    /// `true` **halves the grid on first launch** for an existing install. That is the behaviour the
    /// user asked for, so it stands, and `stacking_is_on_by_default` pins it against a casual flip
    /// in either direction.
    pub enabled: bool,
    /// Which formats take part. A file whose format is absent is never a stack member.
    ///
    /// ⚠⚠ **`FormatFilter` is a set of *hidden* formats**, so `is_visible` here means "takes part".
    /// Reused rather than reinvented because it already collapses `jpg`/`jpeg` and `tif`/`tiff` into
    /// one format each — which stacking genuinely needs, since a folder mixing `.JPG` and `.jpeg`
    /// must not be two formats.
    ///
    /// ⚠ Empty by default: **every** supported format takes part, which is what makes this "more
    /// general than just raw and jpeg".
    pub formats: FormatFilter,
    pub primary: StackPrimary,
}

impl Stacking {
    /// Whether a file's format takes part in stacking.
    fn participates(&self, path: &Path) -> bool {
        self.formats.is_visible(path)
    }
}

/// The stack key for a path, or `None` when it has no stem.
pub fn key_of(path: &Path) -> Option<StackKey> {
    let stem = path.file_stem()?.to_string_lossy().to_uppercase();
    let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    Some((dir, stem))
}

/// Where a member sits in the running for primary. Lower wins.
fn rank(path: &Path, primary: StackPrimary) -> u8 {
    let raw = catalog::is_raw(path);
    let jpeg = matches!(
        path.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref(),
        Some("jpg") | Some("jpeg")
    );
    match primary {
        StackPrimary::Jpeg if jpeg => 0,
        StackPrimary::Jpeg if raw => 1,
        _ if raw => 0,
        _ if jpeg => 1,
        _ => 2,
    }
}

/// Group `entries` into stacks, collapsing all but the primary of each.
///
/// ⚠⚠ **Order is preserved.** The list arrives sorted by `catalog::sort`, and collapsing must not
/// reorder it — so a group is emitted at the position of its **first** member rather than collected
/// into a map and flushed at the end. That is also what makes the output deterministic.
///
/// ⚠ **Copy-0 rows only.** A virtual copy is a version the user deliberately made, and folding it
/// inside a stack would hide their own work, so a copy is always its own row — stacked sibling or
/// not.
pub fn group(
    entries: Vec<PhotoEntry>,
    s: &Stacking,
    expanded: &HashSet<StackKey>,
) -> Vec<PhotoEntry> {
    if !s.enabled {
        return entries;
    }

    // Which rows are candidates, and how many share each key.
    let mut members: HashMap<StackKey, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if e.id.copy != 0 || !s.participates(&e.id.path) {
            continue;
        }
        if let Some(k) = key_of(&e.id.path) {
            members.entry(k).or_default().push(i);
        }
    }
    members.retain(|_, v| v.len() > 1);

    let mut emitted: HashSet<StackKey> = HashSet::new();
    let mut out: Vec<PhotoEntry> = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        let k = (e.id.copy == 0)
            .then(|| key_of(&e.id.path))
            .flatten()
            .filter(|k| members.contains_key(k));
        let Some(k) = k else {
            // Not a stack member: a copy, an excluded format, or a file with no sibling.
            let mut e = e.clone();
            e.stack = None;
            out.push(e);
            continue;
        };
        if !emitted.insert(k.clone()) {
            continue; // already handled at the group's first member
        }
        let idx = &members[&k];
        let count = idx.len();
        if expanded.contains(&k) {
            for &j in idx {
                let mut m = entries[j].clone();
                m.stack = Some(Stack {
                    count,
                    expanded: true,
                });
                out.push(m);
            }
        } else {
            let best = *idx
                .iter()
                .min_by_key(|&&j| {
                    let m = &entries[j];
                    let edited = match s.primary {
                        // ⚠ Edits only decide under `EditedElseRaw`; the other two say "always".
                        StackPrimary::EditedElseRaw => {
                            u8::from(m.edits == edit_model::Edits::default())
                        }
                        _ => 0,
                    };
                    // ⚠ `file_name` last, so the answer is deterministic when the rest ties.
                    (edited, rank(&m.id.path, s.primary), m.file_name.clone())
                })
                .unwrap_or(&i);
            let mut m = entries[best].clone();
            m.stack = Some(Stack {
                count,
                expanded: false,
            });
            out.push(m);
        }
    }
    out
}

/// Every stack key in `entries`, for "Expand all".
///
/// ⚠ Derived the same way [`group`] derives them, so the two cannot disagree about what a stack is.
pub fn keys(entries: &[PhotoEntry], s: &Stacking) -> HashSet<StackKey> {
    if !s.enabled {
        return HashSet::new();
    }
    let mut counts: HashMap<StackKey, usize> = HashMap::new();
    for e in entries {
        if e.id.copy != 0 || !s.participates(&e.id.path) {
            continue;
        }
        if let Some(k) = key_of(&e.id.path) {
            *counts.entry(k).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(k, _)| k)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo_id::PhotoId;

    fn entry(path: &str, copy: u32, edited: bool) -> PhotoEntry {
        let p = PathBuf::from(path);
        PhotoEntry {
            file_name: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            id: PhotoId::new(p, copy),
            label: None,
            edits: if edited {
                edit_model::Edits {
                    exposure: 1.0,
                    ..Default::default()
                }
            } else {
                edit_model::Edits::default()
            },
            stack: None,
        }
    }

    fn on() -> Stacking {
        Stacking {
            enabled: true,
            ..Default::default()
        }
    }

    fn names(v: &[PhotoEntry]) -> Vec<String> {
        v.iter().map(|e| e.file_name.clone()).collect()
    }

    /// ⚠⚠ **The sample assertion first.** "Collapsed to one row" passes vacuously against an input
    /// that never held a pair, so the input's length is asserted before the output's.
    #[test]
    fn a_raw_and_its_jpeg_collapse_to_one_row() {
        let input = vec![entry("/f/a.JPG", 0, false), entry("/f/a.RAF", 0, false)];
        assert_eq!(input.len(), 2, "the fixture must hold a pair");

        let out = group(input, &on(), &HashSet::new());
        assert_eq!(out.len(), 1, "the pair must collapse: {:?}", names(&out));
        assert_eq!(out[0].file_name, "a.RAF", "the RAW is primary by default");
        assert_eq!(
            out[0].stack,
            Some(Stack {
                count: 2,
                expanded: false
            })
        );
    }

    #[test]
    fn an_expanded_stack_yields_every_member() {
        let input = vec![entry("/f/a.JPG", 0, false), entry("/f/a.RAF", 0, false)];
        let mut ex = HashSet::new();
        ex.insert(key_of(Path::new("/f/a.RAF")).unwrap());

        let out = group(input, &on(), &ex);
        assert_eq!(out.len(), 2, "both members must be shown");
        assert!(
            out.iter().all(|e| e.stack
                == Some(Stack {
                    count: 2,
                    expanded: true
                })),
            "every member carries the expanded stack"
        );
    }

    /// ⚠ A lone file is `None`, never a stack of one — or the badge would read "1".
    #[test]
    fn a_file_with_no_sibling_is_not_a_stack() {
        let out = group(
            vec![entry("/f/lonely.RAF", 0, false)],
            &on(),
            &HashSet::new(),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stack, None);
    }

    /// ⚠ Off means **unchanged**, length included — the control for every test above.
    #[test]
    fn disabled_stacking_returns_the_input_untouched() {
        let input = vec![entry("/f/a.JPG", 0, false), entry("/f/a.RAF", 0, false)];
        let off = Stacking {
            enabled: false,
            ..Default::default()
        };
        let out = group(input.clone(), &off, &HashSet::new());
        assert_eq!(out, input);
        assert!(keys(&input, &off).is_empty());
    }

    /// ⚠ A format excluded from stacking is never a member — which is what makes the rule "more
    /// general than just raw and jpeg".
    #[test]
    fn an_excluded_format_never_joins_a_stack() {
        let mut s = on();
        s.formats.toggle("jpg");
        let out = group(
            vec![entry("/f/a.JPG", 0, false), entry("/f/a.RAF", 0, false)],
            &s,
            &HashSet::new(),
        );
        assert_eq!(
            out.len(),
            2,
            "neither may be folded away: {:?}",
            names(&out)
        );
        assert!(out.iter().all(|e| e.stack.is_none()));
    }

    /// ⚠⚠ **The tiebreak the design had to invent**: with both members edited, `EditedElseRaw` must
    /// fall back to the RAW rather than pick whichever came first.
    #[test]
    fn the_primary_follows_the_setting_including_the_both_edited_tiebreak() {
        let pair = |raw_edited, jpg_edited| {
            vec![
                entry("/f/a.JPG", 0, jpg_edited),
                entry("/f/a.RAF", 0, raw_edited),
            ]
        };
        let first =
            |s: &Stacking, v: Vec<PhotoEntry>| group(v, s, &HashSet::new())[0].file_name.clone();

        let edited_else_raw = on();
        assert_eq!(first(&edited_else_raw, pair(false, false)), "a.RAF");
        assert_eq!(
            first(&edited_else_raw, pair(false, true)),
            "a.JPG",
            "an edited JPEG beside an untouched RAW must win"
        );
        assert_eq!(
            first(&edited_else_raw, pair(true, true)),
            "a.RAF",
            "both edited must fall back to the RAW"
        );

        // ⚠ The other two ignore edits entirely, which is the discriminating half.
        let always_raw = Stacking {
            primary: StackPrimary::Raw,
            ..on()
        };
        assert_eq!(first(&always_raw, pair(false, true)), "a.RAF");
        let always_jpeg = Stacking {
            primary: StackPrimary::Jpeg,
            ..on()
        };
        assert_eq!(first(&always_jpeg, pair(true, false)), "a.JPG");
    }

    /// ⚠ A virtual copy is a version the user made; folding it into a stack would hide their work.
    #[test]
    fn virtual_copies_are_never_folded_into_a_stack() {
        let input = vec![
            entry("/f/a.JPG", 0, false),
            entry("/f/a.RAF", 0, false),
            entry("/f/a.RAF", 1, false),
        ];
        let out = group(input, &on(), &HashSet::new());
        assert_eq!(out.len(), 2, "the stack and the copy: {:?}", names(&out));
        assert_eq!(out.iter().filter(|e| e.stack.is_some()).count(), 1);
        let copy = out.iter().find(|e| e.id.copy == 1).expect("the copy row");
        assert_eq!(copy.stack, None);
    }

    #[test]
    fn the_stem_match_ignores_case() {
        let out = group(
            vec![entry("/f/A.JPG", 0, false), entry("/f/a.raf", 0, false)],
            &on(),
            &HashSet::new(),
        );
        assert_eq!(out.len(), 1, "case must not split a pair");
    }

    /// ⚠ The folder is part of the key, so two imports holding the same stem stay apart.
    #[test]
    fn the_same_stem_in_two_folders_does_not_stack() {
        let out = group(
            vec![entry("/one/a.RAF", 0, false), entry("/two/a.RAF", 0, false)],
            &on(),
            &HashSet::new(),
        );
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| e.stack.is_none()));
    }

    /// ⚠⚠ **Order is preserved**, because the list arrives sorted and collapsing must not reorder
    /// it. A map-then-flush implementation passes every test above and fails this one.
    #[test]
    fn collapsing_keeps_the_incoming_order() {
        let input = vec![
            entry("/f/aaa.RAF", 0, false),
            entry("/f/bbb.JPG", 0, false),
            entry("/f/bbb.RAF", 0, false),
            entry("/f/ccc.RAF", 0, false),
        ];
        let out = group(input, &on(), &HashSet::new());
        assert_eq!(
            names(&out),
            vec!["aaa.RAF", "bbb.RAF", "ccc.RAF"],
            "the collapsed row must sit where its group began"
        );
    }

    #[test]
    fn keys_names_every_stack_and_nothing_else() {
        let input = vec![
            entry("/f/a.JPG", 0, false),
            entry("/f/a.RAF", 0, false),
            entry("/f/lonely.RAF", 0, false),
        ];
        let k = keys(&input, &on());
        assert_eq!(k.len(), 1, "one stack, not the lone file: {k:?}");
        assert!(k.contains(&key_of(Path::new("/f/a.RAF")).unwrap()));
    }
}
