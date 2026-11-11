//! The learning pass's decisions: which photos teach which look, when a look is due, and one look's fit.
//!
//! ⚠ The render is INJECTED: what a look is fitted on must be exactly what the app renders before applying it —
//! the camera profile the user chose included — and only the app knows that. Tests hand in a stand-in.

use super::fit::{self, Pair};
use super::fuji::JpegLook;
use super::store::{Learned, Store};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A group is learned once it has this many plain photos.
pub const MIN_PHOTOS: usize = 8;
/// …and from at most this many, spread across it.
pub const MAX_PHOTOS: usize = 40;
/// Pairs per fit, at most — beyond it the fit costs time and gains nothing measurable.
const MAX_PAIRS: usize = 200_000;
/// The long edge photos are compared at.
pub const EDGE: usize = 384;

/// A catalogued Fuji RAF and what its JPEG was rendered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    /// The camera, as the catalog names it.
    pub body: String,
    pub look: JpegLook,
}

/// A look to learn: its key, the photos chosen, and how many plain photos the group holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub body: String,
    pub film: String,
    pub photos: Vec<PathBuf>,
    pub total: usize,
}

/// The looks due: every body-and-film group with at least [`MIN_PHOTOS`] plain photos that has no look yet, or
/// has grown by half since its look was fitted.
pub fn plan(candidates: &[Candidate], learned: &[Learned]) -> Vec<Group> {
    let mut groups: BTreeMap<(&str, &str), Vec<&Path>> = BTreeMap::new();
    for c in candidates.iter().filter(|c| c.look.plain) {
        groups
            .entry((c.body.as_str(), c.look.film.as_str()))
            .or_default()
            .push(&c.path);
    }
    groups
        .into_iter()
        .filter(|(_, p)| p.len() >= MIN_PHOTOS)
        .filter(|((body, film), p)| {
            learned
                .iter()
                .find(|l| l.body == *body && l.film == *film)
                .is_none_or(|l| p.len() * 2 >= l.group * 3)
        })
        .map(|((body, film), mut p)| {
            p.sort();
            Group {
                body: body.to_owned(),
                film: film.to_owned(),
                total: p.len(),
                photos: spread(&p, MAX_PHOTOS),
            }
        })
        .collect()
}

/// At most `n` of `paths`, evenly spaced — a group's photos span shoots, and its first forty would be one.
pub fn spread(paths: &[&Path], n: usize) -> Vec<PathBuf> {
    if paths.len() <= n {
        return paths.iter().map(|p| p.to_path_buf()).collect();
    }
    (0..n)
        .map(|i| paths[i * paths.len() / n].to_path_buf())
        .collect()
}

/// A rendering of one photo: encoded sRGB samples at `w` × `h`.
pub type Rendered = (Vec<[f32; 3]>, usize, usize);

/// The camera's own JPEG of `path` at `w` × `h`, encoded sRGB samples — the target a look is fitted to.
pub fn camera_jpeg(path: &Path, w: usize, h: usize) -> Option<Vec<[f32; 3]>> {
    let t = raw_decode::extract_preview(path).ok()?;
    let img = image::RgbaImage::from_raw(t.width as u32, t.height as u32, t.rgba)?;
    let img = image::imageops::resize(
        &img,
        w as u32,
        h as u32,
        image::imageops::FilterType::Triangle,
    );
    Some(
        img.pixels()
            .map(|p| [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0))
            .collect(),
    )
}

/// Fit one group's look and save it: each photo rendered by `render` and compared with what `target` says the
/// camera made of it. `None` when no photo yields a pair (every render failed).
pub fn learn(
    group: &Group,
    render: &dyn Fn(&Path) -> Option<Rendered>,
    target: &dyn Fn(&Path, usize, usize) -> Option<Vec<[f32; 3]>>,
    store: &Store,
    now: i64,
) -> std::io::Result<Option<Learned>> {
    let mut pairs: Vec<Pair> = Vec::new();
    let mut used = 0;
    for p in &group.photos {
        let Some((ours, w, h)) = render(p) else {
            continue;
        };
        let Some(theirs) = target(p, w, h) else {
            continue;
        };
        let s = fit::samples(&ours, &theirs, w, h);
        if !s.is_empty() {
            used += 1;
            pairs.extend(s);
        }
    }
    if pairs.is_empty() {
        return Ok(None);
    }
    // ⚠ Thinned evenly, never truncated: the last photos must count as much as the first.
    let step = pairs.len().div_ceil(MAX_PAIRS);
    let pairs: Vec<Pair> = pairs.into_iter().step_by(step).collect();
    let lattice = fit::fit(&pairs);
    store
        .save(&group.body, &group.film, &lattice, used, group.total, now)
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(i: usize, film: &str, plain: bool) -> Candidate {
        Candidate {
            path: PathBuf::from(format!("/p/{film}-{i:03}.RAF")),
            body: "FUJIFILM X-T4".into(),
            look: JpegLook {
                film: film.into(),
                plain,
            },
        }
    }

    fn learned(film: &str, group: usize) -> Learned {
        Learned {
            body: "FUJIFILM X-T4".into(),
            film: film.into(),
            file: "x.cube".into(),
            photos: group.min(MAX_PHOTOS),
            group,
            at: 0,
        }
    }

    #[test]
    fn a_group_is_due_with_enough_plain_photos_and_spread_across_them() {
        let mut c: Vec<Candidate> = (0..100).map(|i| cand(i, "Classic Chrome", true)).collect();
        c.extend((0..7).map(|i| cand(i, "Provia", true)));
        c.extend((0..20).map(|i| cand(i, "Velvia", false)));
        let p = plan(&c, &[]);
        assert_eq!(p.len(), 1, "Provia has 7, Velvia's are not plain: {p:?}");
        assert_eq!(
            (p[0].film.as_str(), p[0].total, p[0].photos.len()),
            ("Classic Chrome", 100, MAX_PHOTOS)
        );
        assert!(
            p[0].photos
                .last()
                .unwrap()
                .ends_with("Classic Chrome-097.RAF"),
            "spread to the end"
        );
    }

    #[test]
    fn a_learned_look_is_due_again_only_when_its_group_has_grown_by_half() {
        let c: Vec<Candidate> = (0..14).map(|i| cand(i, "Eterna", true)).collect();
        assert!(plan(&c, &[learned("Eterna", 10)]).is_empty());
        assert_eq!(plan(&c, &[learned("Eterna", 9)]).len(), 1);
    }

    #[test]
    fn learning_fits_what_the_render_and_the_camera_disagree_on() {
        let dir = test_dir::TempDir::new("camera-look-learn");
        let store = Store::new(dir.path().to_path_buf());
        let group = Group {
            body: "FUJIFILM X-T4".into(),
            film: "Classic Chrome".into(),
            photos: (0..3)
                .map(|i| PathBuf::from(format!("/p/{i}.RAF")))
                .collect(),
            total: 9,
        };
        // Constructed: our render is a smooth gradient; "the camera" made it warmer. One photo fails to render.
        // ⚠ Large enough that the gradient is flat by `fit::samples`' edge test — at 40 × 30 every pixel was an edge.
        let (w, h) = (200, 150);
        let render = |p: &Path| {
            (!p.ends_with("2.RAF")).then(|| {
                let px: Vec<[f32; 3]> = (0..w * h)
                    .map(|i| [(i % w) as f32 / w as f32, (i / w) as f32 / h as f32, 0.5])
                    .collect();
                (px, w, h)
            })
        };
        let warm = |p: [f32; 3]| [(p[0] + 0.1).min(1.0), p[1], (p[2] - 0.1).max(0.0)];
        let target = |p: &Path, w: usize, h: usize| {
            render(p).map(|(px, ..)| px.into_iter().map(warm).take(w * h).collect())
        };
        let l = learn(&group, &render, &target, &store, 5)
            .unwrap()
            .expect("learned");
        assert_eq!((l.photos, l.group, l.at), (2, 9, 5));
        let cube = store.cube("FUJIFILM X-T4", "Classic Chrome").unwrap();
        let got = cube.lookup([0.5, 0.5, 0.5]);
        assert!(
            (got[0] - 0.6).abs() < 0.03 && (got[2] - 0.4).abs() < 0.03,
            "{got:?}"
        );
    }
}
