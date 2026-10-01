//! Keyword suggestions and autocomplete (M10.8a): which keywords a selection probably wants, and which
//! keywords a typed name means.
//!
//! ⚠ No egui — the `keyword_tool` pattern: the rail and the right-click menu draw these, the rules are
//! tested here without a window.
//!
//! **Suggestions** weigh two kinds of evidence, Lightroom's model (the author's choice, 2026-09-30):
//! the keywords on photos taken **on the same capture day** as the selection, and the keywords that
//! **usually go with** the ones the selection already has. "Recently used" is deliberately absent —
//! the catalog keeps no assignment times.

use catalog::db::{Catalog, DayCounts, KeywordRow};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// How many suggestions the rail and the menu show.
pub const LIMIT: usize = 5;

/// How many matches the field's list shows.
pub const COMPLETE_LIMIT: usize = 8;

/// Everything `rank` weighs, read from the catalog by `suggest`.
pub struct Evidence<'a> {
    /// The whole tree; `count` is a keyword's usage.
    pub keywords: &'a [KeywordRow],
    /// How many of the selected files carry each keyword.
    pub on_selection: &'a HashMap<i64, usize>,
    /// How many distinct files are selected.
    pub files: usize,
    /// The photos taken on the selection's capture day(s), and their keywords.
    pub day: &'a DayCounts,
    /// `(j, k)` → files carrying both, for each `j` on the selection.
    pub companions: &'a HashMap<(i64, i64), usize>,
}

/// The suggested keyword ids, best first, at most `limit`.
///
/// A keyword scores the share of the same-day photos that carry it, plus its best companion rate
/// `P(k | j)` over the keywords `j` already on the selection. ⚠ **Excluded:** a keyword already on
/// every selected file, every ancestor of one (the XMP path carries parents already), and anything
/// that scores nothing. A keyword on *some* selected files stays — tagging makes the selection
/// uniform. Ties go to the more-used keyword, then the name.
pub fn rank(e: &Evidence, limit: usize) -> Vec<i64> {
    let parent: HashMap<i64, Option<i64>> =
        e.keywords.iter().map(|k| (k.id, k.parent_id)).collect();
    let usage: HashMap<i64, i64> = e.keywords.iter().map(|k| (k.id, k.count)).collect();
    let mut excluded: HashSet<i64> = HashSet::new();
    for (&k, &n) in e.on_selection {
        if e.files > 0 && n == e.files {
            excluded.insert(k);
            let mut up = parent.get(&k).copied().flatten();
            while let Some(p) = up {
                if !excluded.insert(p) {
                    break;
                }
                up = parent.get(&p).copied().flatten();
            }
        }
    }
    let mut scored: Vec<(f32, i64, &KeywordRow)> = e
        .keywords
        .iter()
        .filter(|k| !excluded.contains(&k.id))
        .filter_map(|k| {
            let day = match e.day.neighbours {
                0 => 0.0,
                n => e.day.counts.get(&k.id).copied().unwrap_or(0) as f32 / n as f32,
            };
            let companion = e
                .on_selection
                .keys()
                .filter_map(|j| {
                    let both = *e.companions.get(&(*j, k.id))?;
                    let of_j = *usage.get(j)?;
                    (of_j > 0).then(|| both as f32 / of_j as f32)
                })
                .fold(0.0, f32::max);
            let score = day + companion;
            (score > 0.0).then_some((score, k.count, k))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.2.name.to_lowercase().cmp(&b.2.name.to_lowercase()))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, k)| k.id)
        .collect()
}

/// The suggestions for `files` — **distinct** files, since keywords are per file — read from the
/// catalog and ranked.
pub fn suggest(cat: &Catalog, files: &[PathBuf], limit: usize) -> Vec<KeywordRow> {
    if files.is_empty() {
        return Vec::new();
    }
    let keywords = cat.keywords().unwrap_or_default();
    let on_selection = cat.keyword_counts(files).unwrap_or_default();
    let day = cat.day_keyword_counts(files).unwrap_or_default();
    let mine: Vec<i64> = on_selection.keys().copied().collect();
    let companions = cat.companion_counts(&mine).unwrap_or_default();
    let evidence = Evidence {
        keywords: &keywords,
        on_selection: &on_selection,
        files: files.len(),
        day: &day,
        companions: &companions,
    };
    rank(&evidence, limit)
        .into_iter()
        .filter_map(|id| keywords.iter().find(|k| k.id == id).cloned())
        .collect()
}

/// One autocomplete match: the keyword and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub id: i64,
    pub name: String,
    /// The parent chain, root first (`Places › Europe`); empty for a root.
    pub path: String,
}

/// The keywords `typed` means, case-insensitively: an exact name first, then names starting with it,
/// then names containing it; within each, by name, then path (two `Berlin`s in different branches).
/// At most `limit`.
pub fn complete(typed: &str, kws: &[KeywordRow], limit: usize) -> Vec<Match> {
    let t = typed.trim().to_lowercase();
    if t.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, (String, String), &KeywordRow)> = kws
        .iter()
        .filter_map(|k| {
            let n = k.name.to_lowercase();
            let rank = if n == t {
                0
            } else if n.starts_with(&t) {
                1
            } else if n.contains(&t) {
                2
            } else {
                return None;
            };
            Some((rank, (n, parent_path(k.id, kws).to_lowercase()), k))
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    hits.into_iter()
        .take(limit)
        .map(|(_, _, k)| Match {
            id: k.id,
            name: k.name.clone(),
            path: parent_path(k.id, kws),
        })
        .collect()
}

/// `id`'s parent chain, root first, joined by ` › ` — empty for a root.
pub fn parent_path(id: i64, kws: &[KeywordRow]) -> String {
    let by_id: HashMap<i64, &KeywordRow> = kws.iter().map(|k| (k.id, k)).collect();
    let mut chain = Vec::new();
    let mut up = by_id.get(&id).and_then(|k| k.parent_id);
    while let Some(p) = up {
        let Some(k) = by_id.get(&p) else { break };
        chain.push(k.name.as_str());
        up = k.parent_id;
    }
    chain.reverse();
    chain.join(" › ")
}

/// `suggest`, remembered until the selection or the catalog changes.
///
/// ⚠⚠ Keyed on [`Catalog::change_count`], not on a list of the places that tag: every write from
/// any route — present or future — moves it, so nothing can leave the suggestions stale. The rail
/// redraws at 60 Hz and asks every frame; this is what makes that free.
#[derive(Default)]
pub struct Cache {
    key: Option<(Vec<PathBuf>, u64)>,
    list: Vec<KeywordRow>,
    /// How many times `suggest` ran — what the test counts.
    computed: usize,
}

impl Cache {
    /// The suggestions for `files` (distinct files).
    pub fn get(&mut self, cat: &Catalog, files: &[PathBuf]) -> &[KeywordRow] {
        let key = (files.to_vec(), cat.change_count());
        if self.key.as_ref() != Some(&key) {
            self.list = suggest(cat, files, LIMIT);
            self.key = Some(key);
            self.computed += 1;
        }
        &self.list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(id: i64, name: &str, parent: Option<i64>, count: i64) -> KeywordRow {
        KeywordRow {
            id,
            name: name.to_string(),
            parent_id: parent,
            depth: 0,
            count,
        }
    }

    /// Places › Europe › {Berlin, Paris}; Genre › Street; Night; Cats; Berlinale.
    fn tree() -> Vec<KeywordRow> {
        vec![
            kw(1, "Places", None, 0),
            kw(2, "Europe", Some(1), 0),
            kw(3, "Berlin", Some(2), 10),
            kw(4, "Paris", Some(2), 4),
            kw(5, "Genre", None, 0),
            kw(6, "Street", Some(5), 8),
            kw(7, "Night", None, 6),
            kw(8, "Cats", None, 20),
            kw(9, "Berlinale", None, 2),
        ]
    }

    fn ranked(
        on: &[(i64, usize)],
        files: usize,
        day: (usize, &[(i64, usize)]),
        comp: &[((i64, i64), usize)],
    ) -> Vec<i64> {
        let kws = tree();
        let on: HashMap<i64, usize> = on.iter().copied().collect();
        let day = DayCounts {
            neighbours: day.0,
            counts: day.1.iter().copied().collect(),
        };
        let comp: HashMap<(i64, i64), usize> = comp.iter().copied().collect();
        rank(
            &Evidence {
                keywords: &kws,
                on_selection: &on,
                files,
                day: &day,
                companions: &comp,
            },
            LIMIT,
        )
    }

    /// Berlin is on both selected files; ten photos share their day, six of them Street and two
    /// Night; Paris goes with Berlin on three of Berlin's ten files.
    ///
    /// ⚠⚠ Berlin, Europe and Places carry evidence of their own — four and five of the day's photos,
    /// two of Berlin's files — so only the exclusions keep them out. Without it the zero-score
    /// filter dropped them anyway, and turning the ancestor exclusion off left this green.
    #[test]
    fn the_same_day_and_companions_are_suggested_and_the_rest_is_not() {
        let got = ranked(
            &[(3, 2)],
            2,
            (10, &[(6, 6), (7, 2), (3, 4), (2, 5)]),
            &[((3, 4), 3), ((3, 1), 2)],
        );
        // ⚠ The sample first: a ranking that returned nothing would pass every exclusion below.
        assert!(!got.is_empty(), "no suggestion at all");
        assert_eq!(got, vec![6, 4, 7], "Street 0.6, Paris 0.3, Night 0.2");
        assert!(!got.contains(&3), "Berlin is on every selected file");
        assert!(
            !got.contains(&2) && !got.contains(&1),
            "Berlin's ancestors are in its path"
        );
        assert!(!got.contains(&8), "Cats has no evidence");
    }

    #[test]
    fn a_keyword_on_some_of_the_selection_is_still_suggested() {
        let got = ranked(&[(3, 1)], 2, (4, &[(3, 2)]), &[]);
        assert_eq!(got, vec![3], "tagging the rest makes the selection uniform");
    }

    #[test]
    fn ties_go_to_the_more_used_then_the_name() {
        // Street (8) and Night (6) score alike: the more used first.
        assert_eq!(ranked(&[], 1, (2, &[(6, 1), (7, 1)]), &[]), vec![6, 7]);
        // Paris (4) and an equally used keyword: by name.
        let kws = vec![kw(4, "Paris", None, 4), kw(10, "Oslo", None, 4)];
        let day = DayCounts {
            neighbours: 2,
            counts: [(4, 1), (10, 1)].into_iter().collect(),
        };
        let none = HashMap::new();
        let got = rank(
            &Evidence {
                keywords: &kws,
                on_selection: &none,
                files: 1,
                day: &day,
                companions: &HashMap::new(),
            },
            LIMIT,
        );
        assert_eq!(got, vec![10, 4], "Oslo before Paris");
    }

    #[test]
    fn at_most_the_limit() {
        let all: Vec<(i64, usize)> = (1..=9).map(|k| (k, 1)).collect();
        assert_eq!(ranked(&[], 1, (9, &all), &[]).len(), LIMIT);
    }

    #[test]
    fn completion_puts_exact_then_prefix_then_substring() {
        let kws = tree();
        let names = |t: &str| -> Vec<String> {
            complete(t, &kws, COMPLETE_LIMIT)
                .into_iter()
                .map(|m| m.name)
                .collect()
        };
        assert_eq!(names("berlin"), vec!["Berlin", "Berlinale"]);
        assert_eq!(
            names("BER"),
            vec!["Berlin", "Berlinale"],
            "case-insensitive"
        );
        assert_eq!(names("rli"), vec!["Berlin", "Berlinale"]);
        assert_eq!(names("ar"), vec!["Paris"]);
        assert!(names("  ").is_empty());
        let m = &complete("paris", &kws, COMPLETE_LIMIT)[0];
        assert_eq!(m.path, "Places › Europe");
        assert_eq!(parent_path(7, &kws), "", "a root has no path");
    }

    /// The cache runs `suggest` once per selection and catalog state — a redraw asks again and gets
    /// the remembered list; a write moves the change count and recomputes.
    #[test]
    fn the_cache_recomputes_on_a_write_and_not_on_a_redraw() {
        let cat = Catalog::open_in_memory().unwrap();
        let entries: Vec<catalog::CatalogEntry> = ["a.raf", "b.raf"]
            .iter()
            .map(|n| catalog::CatalogEntry {
                path: PathBuf::from("/p").join(n),
                file_name: n.to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect();
        cat.sync_folder(std::path::Path::new("/p"), &entries)
            .unwrap();
        let berlin = cat.create_keyword("Berlin", None).unwrap();
        let street = cat.create_keyword("Street", None).unwrap();
        let (a, b) = (PathBuf::from("/p/a.raf"), PathBuf::from("/p/b.raf"));
        cat.assign_keyword(berlin, &b).unwrap();
        cat.assign_keyword(street, &b).unwrap();
        cat.assign_keyword(berlin, &a).unwrap();
        let mut cache = Cache::default();
        let first: Vec<i64> = cache
            .get(&cat, std::slice::from_ref(&a))
            .iter()
            .map(|k| k.id)
            .collect();
        assert_eq!(first, vec![street], "Street goes with Berlin");
        let _ = cache.get(&cat, std::slice::from_ref(&a));
        assert_eq!(cache.computed, 1, "a redraw recomputed");
        cat.assign_keyword(street, &a).unwrap();
        assert!(
            cache.get(&cat, &[a]).is_empty(),
            "Street is on the photo now"
        );
        assert_eq!(cache.computed, 2, "a write did not recompute");
    }
}
