//! Sending Rustroom's marks to Immich (M10.15d) — what a send changes, how it is grouped into
//! requests, how it is undone, and the words the confirmation shows. Every decision is here, and
//! none of it touches a window, a server or a disk.
//!
//! ⚠⚠ **Never remove** (the author's rule): a photo with no rating or no flag in Rustroom leaves
//! Immich's as it is. A send can add or change a mark in Immich, never clear one — only an undo
//! puts back a value it replaced, `null` included.
//!
//! The mapping mirrors `immich_catalog::server_marks` (M10.15c), the read half: ★1–5 ↔ rating,
//! pick ↔ favourite, reject ↔ rating −1.

use catalog::db::{Flag, Marks};

/// What an asset has in Immich: stars (`None` unrated, −1 rejected) and the heart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServerMarks {
    pub rating: Option<i32>,
    pub favourite: bool,
}

/// One selected photo: its asset, its name, its Rustroom marks and what Immich has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub asset_id: String,
    pub name: String,
    pub mine: Marks,
    pub server: ServerMarks,
}

/// One photo a send changes, from `before` to `after`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub asset_id: String,
    pub name: String,
    pub before: ServerMarks,
    pub after: ServerMarks,
}

/// One `PUT /assets`: these ids, and only the fields being set (`Some(None)` is an explicit unrated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub ids: Vec<String>,
    pub rating: Option<Option<i32>>,
    pub favourite: Option<bool>,
}

/// What an asset has in Immich, from its DTO (M10.15c's fields).
pub fn server_marks_of(a: &immich::Asset) -> ServerMarks {
    ServerMarks {
        rating: a.exif_info.as_ref().and_then(|e| e.rating),
        favourite: a.is_favorite,
    }
}

/// What Immich should have after a send — the mapping, under **never remove**: a reject sends −1
/// (stars and a reject too — Immich cannot hold both, and the reject is the more deliberate mark);
/// else stars 1–5 send themselves; else Immich keeps its rating. A pick adds the heart; nothing here
/// ever takes one away.
pub fn wanted(mine: Marks, server: ServerMarks) -> ServerMarks {
    let rating = if mine.flag == Flag::Reject {
        Some(-1)
    } else if (1..=5).contains(&mine.rating) {
        Some(mine.rating as i32)
    } else {
        server.rating
    };
    ServerMarks {
        rating,
        favourite: mine.flag == Flag::Pick || server.favourite,
    }
}

/// The photos a send would change, in the selection's order — those whose [`wanted`] differs.
pub fn plan(candidates: &[Candidate]) -> Vec<Change> {
    candidates
        .iter()
        .filter_map(|c| {
            let after = wanted(c.mine, c.server);
            (after != c.server).then(|| Change {
                asset_id: c.asset_id.clone(),
                name: c.name.clone(),
                before: c.server,
                after,
            })
        })
        .collect()
}

/// Whether a change replaces a value Immich already had — stars over stars — rather than adding to an
/// empty one. The confirmation calls these out.
pub fn replaces(c: &Change) -> bool {
    c.before.rating.is_some() && c.before.rating != c.after.rating
}

/// One request per distinct delta, in first-seen order; a field is present only where it changes.
pub fn groups(changes: &[Change]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for c in changes {
        let rating = (c.after.rating != c.before.rating).then_some(c.after.rating);
        let favourite = (c.after.favourite != c.before.favourite).then_some(c.after.favourite);
        match out
            .iter_mut()
            .find(|g| g.rating == rating && g.favourite == favourite)
        {
            Some(g) => g.ids.push(c.asset_id.clone()),
            None => out.push(Group {
                ids: vec![c.asset_id.clone()],
                rating,
                favourite,
            }),
        }
    }
    out
}

/// The requests that put back what a send replaced — **both** fields, to the prior values, `null`
/// included: an undo is the one place a mark in Immich is cleared.
pub fn undo_groups(prior: &[(String, ServerMarks)]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for (id, m) in prior {
        let (rating, favourite) = (Some(m.rating), Some(m.favourite));
        match out
            .iter_mut()
            .find(|g| g.rating == rating && g.favourite == favourite)
        {
            Some(g) => g.ids.push(id.clone()),
            None => out.push(Group {
                ids: vec![id.clone()],
                rating,
                favourite,
            }),
        }
    }
    out
}

fn stars(r: Option<i32>) -> String {
    match r {
        None => "unrated".to_string(),
        Some(-1) => "rejected".to_string(),
        Some(n) => format!("★{n}"),
    }
}

/// One confirmation line: `IMG_1234.jpg  ★3 → ★5  ·  favourite`.
pub fn line(c: &Change) -> String {
    let mut parts = Vec::new();
    if c.after.rating != c.before.rating {
        parts.push(format!(
            "{} → {}",
            stars(c.before.rating),
            stars(c.after.rating)
        ));
    }
    if c.after.favourite && !c.before.favourite {
        parts.push("favourite".to_string());
    }
    format!("{}  {}", c.name, parts.join("  ·  "))
}

fn count(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// The confirmation's count line: `3 photos: 2 ratings, 1 favourite — 1 replaces what Immich had`.
pub fn summary(changes: &[Change]) -> String {
    let ratings = changes
        .iter()
        .filter(|c| c.after.rating != c.before.rating)
        .count();
    let favourites = changes
        .iter()
        .filter(|c| c.after.favourite && !c.before.favourite)
        .count();
    let mut kinds = Vec::new();
    if ratings > 0 {
        kinds.push(count(ratings, "rating"));
    }
    if favourites > 0 {
        kinds.push(count(favourites, "favourite"));
    }
    let mut s = format!("{}: {}", count(changes.len(), "photo"), kinds.join(", "));
    let replaced = changes.iter().filter(|c| replaces(c)).count();
    if replaced > 0 {
        s.push_str(&format!(" — {replaced} replaces what Immich had"));
    }
    s
}

/// The Immich assets a selection names, each once, in order — a virtual copy shares its original's
/// cache path, and Immich has one asset for both.
pub fn selected_asset_ids(paths: &[std::path::PathBuf]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    paths
        .iter()
        .filter_map(|p| crate::immich_source::asset_id_of(p))
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(rating: u8, flag: Flag) -> Marks {
        Marks {
            rating,
            flag,
            label: 0,
        }
    }

    fn server(rating: Option<i32>, favourite: bool) -> ServerMarks {
        ServerMarks { rating, favourite }
    }

    fn change(id: &str, before: ServerMarks, after: ServerMarks) -> Change {
        Change {
            asset_id: id.into(),
            name: format!("IMG_{id}.jpg"),
            before,
            after,
        }
    }

    #[test]
    fn the_mapping_never_removes() {
        let fav3 = server(Some(3), true);
        assert_eq!(wanted(marks(0, Flag::None), fav3), fav3, "no opinion here");
        assert_eq!(wanted(marks(5, Flag::None), fav3).rating, Some(5));
        let picked = wanted(marks(0, Flag::Pick), server(Some(2), false));
        assert_eq!(
            picked,
            server(Some(2), true),
            "a pick adds the heart, keeps the stars"
        );
        let rejected = wanted(marks(0, Flag::Reject), server(Some(4), true));
        assert_eq!(rejected, server(Some(-1), true), "a reject keeps the heart");
        assert_eq!(
            wanted(marks(4, Flag::Reject), server(None, false)).rating,
            Some(-1),
            "stars and a reject send the reject"
        );
        assert!(
            wanted(marks(0, Flag::None), server(None, true)).favourite,
            "unflagged never un-favourites"
        );
        assert_eq!(
            wanted(marks(0, Flag::None), server(Some(2), false)).rating,
            Some(2),
            "unrated never clears the stars"
        );
    }

    #[test]
    fn a_plan_holds_only_what_changes_in_order() {
        let c = |id: &str, mine, srv| Candidate {
            asset_id: id.into(),
            name: format!("IMG_{id}.jpg"),
            mine,
            server: srv,
        };
        let p = plan(&[
            c("a", marks(5, Flag::None), server(Some(3), false)),
            c("b", marks(3, Flag::None), server(Some(3), false)),
            c("c", marks(0, Flag::Pick), server(None, false)),
        ]);
        let ids: Vec<&str> = p.iter().map(|c| c.asset_id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        assert_eq!(p[0].before, server(Some(3), false));
        assert_eq!(p[0].after, server(Some(5), false));
    }

    #[test]
    fn replacing_is_changing_a_value_immich_had() {
        assert!(replaces(&change(
            "1",
            server(Some(3), false),
            server(Some(5), false)
        )));
        assert!(!replaces(&change(
            "1",
            server(None, false),
            server(Some(5), false)
        )));
        assert!(!replaces(&change(
            "1",
            server(None, false),
            server(None, true)
        )));
    }

    #[test]
    fn identical_deltas_share_a_group_and_different_ones_split() {
        let five = |id| change(id, server(None, false), server(Some(5), false));
        let g = groups(&[five("a"), five("b")]);
        assert_eq!(
            g,
            vec![Group {
                ids: vec!["a".into(), "b".into()],
                rating: Some(Some(5)),
                favourite: None
            }]
        );
        let heart = change("c", server(None, false), server(None, true));
        let both = change("d", server(None, false), server(Some(2), true));
        let g = groups(&[five("a"), heart, both]);
        assert_eq!(g.len(), 3);
        assert_eq!((g[1].rating, g[1].favourite), (None, Some(true)));
        assert_eq!((g[2].rating, g[2].favourite), (Some(Some(2)), Some(true)));
    }

    #[test]
    fn undo_groups_restore_both_fields_null_included() {
        let g = undo_groups(&[
            ("a".to_string(), server(None, false)),
            ("b".to_string(), server(Some(3), true)),
        ]);
        assert_eq!(
            g,
            vec![
                Group {
                    ids: vec!["a".into()],
                    rating: Some(None),
                    favourite: Some(false)
                },
                Group {
                    ids: vec!["b".into()],
                    rating: Some(Some(3)),
                    favourite: Some(true)
                },
            ]
        );
    }

    #[test]
    fn a_line_says_what_changes() {
        assert_eq!(
            line(&change("1", server(Some(3), false), server(Some(5), false))),
            "IMG_1.jpg  ★3 → ★5"
        );
        assert_eq!(
            line(&change("2", server(None, false), server(Some(4), false))),
            "IMG_2.jpg  unrated → ★4"
        );
        assert_eq!(
            line(&change("3", server(None, false), server(None, true))),
            "IMG_3.jpg  favourite"
        );
        assert_eq!(
            line(&change(
                "4",
                server(Some(2), false),
                server(Some(-1), false)
            )),
            "IMG_4.jpg  ★2 → rejected"
        );
        assert_eq!(
            line(&change("5", server(None, false), server(Some(5), true))),
            "IMG_5.jpg  unrated → ★5  ·  favourite"
        );
    }

    #[test]
    fn the_summary_counts_and_says_what_it_replaces() {
        let c = [
            change("1", server(Some(3), false), server(Some(5), false)),
            change("2", server(None, false), server(Some(4), false)),
            change("3", server(None, false), server(None, true)),
        ];
        assert_eq!(
            summary(&c),
            "3 photos: 2 ratings, 1 favourite — 1 replaces what Immich had"
        );
        assert_eq!(summary(&c[1..2]), "1 photo: 1 rating");
    }

    /// A virtual copy shares its original's path — one asset, counted once, in selection order.
    #[test]
    fn a_copy_counts_as_its_original_once() {
        let p = |n: &str| std::path::PathBuf::from(format!("/cache/server/{n}.jpg"));
        assert_eq!(
            selected_asset_ids(&[p("a"), p("b"), p("a")]),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn an_assets_marks_are_read_from_its_dto() {
        let mut a: immich::Asset = serde_json::from_value(serde_json::json!({
            "id": "a", "type": "IMAGE", "originalFileName": "a.jpg", "originalMimeType": null,
            "width": 1, "height": 1, "thumbhash": null,
            "fileModifiedAt": "2025-01-01T00:00:00.000Z",
            "updatedAt": "2025-01-01T00:00:00.000Z"
        }))
        .unwrap();
        assert_eq!(server_marks_of(&a), server(None, false));
        a.is_favorite = true;
        a.exif_info = Some(immich::ExifInfo { rating: Some(4) });
        assert_eq!(server_marks_of(&a), server(Some(4), true));
    }
}
