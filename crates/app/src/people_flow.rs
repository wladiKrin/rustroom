//! Find people's job (M10.16c3): a press starts the search in the background once the models are downloaded and
//! the photo has loaded, and the people it finds land on the session, the small ones pre-ticked. The wiring only —
//! every decision is `rustroom_app::people_tool`'s.
//!
//! ⚠ Pumped by Develop's body, `dust_flow`'s reason: a search that lands while the Retouch panel is closed still
//! lands.

use crate::RustroomApp;
use core_image::ImageBuf;
use eframe::egui;
use rustroom_app::people_tool::{self, Search, Wait};
use std::path::PathBuf;
use std::sync::Arc;

/// Below this long side the decode is the stand-in shown while the photo loads.
const LOADED: usize = 1500;
/// How often the panel redraws while a search runs or a press waits.
const REDRAW: std::time::Duration = std::time::Duration::from_millis(50);
/// The long side the people are found at — the tiles' own.
const INPUT_LONG: usize = ai_mask::people::TILED_LONG as usize;

/// The upright image the search reads: the decode's positive, sRGB-encoded — the AI masks' own input
/// (`ai_worker::model_input`), so a film negative is searched as the photograph it holds.
pub(crate) fn people_input(
    buffer: &ImageBuf,
    turns: u8,
    negative: &edit_model::Negative,
) -> image::RgbImage {
    rustroom_app::ai_worker::model_input(buffer, INPUT_LONG, turns, negative)
}

impl RustroomApp {
    /// Where Find people's models are — the AI flow's folder, a test's own when it set one.
    pub(crate) fn people_models(&self) -> Option<PathBuf> {
        match &self.ai.dirs {
            Some((_, models)) => Some(models.clone()),
            None => rustroom_app::ai_cache::models_dir(),
        }
    }

    /// Once per Develop frame: a press waits for the models and the photo, then searches; a search that lands
    /// shows its people. Finding people edits nothing — Remove N people is the panel's one step.
    pub(crate) fn pump_people(&mut self, ctx: &egui::Context) {
        let models = self.people_models();
        let missing: Vec<ai_mask::ModelId> = std::iter::once(ai_mask::ModelId::YoloxM)
            .chain(ai_mask::registry::CLICK_MODELS)
            .filter(|m| {
                !models
                    .as_deref()
                    .is_some_and(|d| ai_mask::download::present(&m.spec(), d))
            })
            .collect();
        let offering = self.ai.download.is_some() || self.modal == crate::modals::Modal::AiDownload;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        if std::mem::take(&mut s.people.pressed) {
            s.people.said = None;
            s.people.people.clear();
            s.people.ticked.clear();
            s.people.waiting = Some(Wait::Photo);
            if !missing.is_empty() {
                s.people.waiting = Some(Wait::Models);
                self.ai.proposed = missing.clone();
                self.modal = crate::modals::Modal::AiDownload;
                return;
            }
        }
        if s.people.waiting == Some(Wait::Models) {
            if missing.is_empty() {
                s.people.waiting = Some(Wait::Photo);
            } else if !offering {
                // ⚠ The dialog closed without them — a press must not wait for ever.
                s.people.waiting = None;
                s.people.said = Some("Find people needs its models".to_string());
            }
        }
        let loaded = !s.buffer_is_ghost && s.buffer.width().max(s.buffer.height()) >= LOADED;
        if s.people.waiting == Some(Wait::Photo) && loaded {
            if let Some(models) = models {
                let turns = s.edits.geometry.orientation.rotate.turns();
                let img = Arc::new(people_input(&s.buffer, turns, &s.edits.negative));
                s.people.waiting = None;
                s.people.search = Some(Search::spawn(img, turns, people_tool::real_finder(models)));
            }
        }
        if s.people.waiting.is_some() {
            ctx.request_repaint_after(REDRAW);
        }
        let Some(search) = &s.people.search else {
            return;
        };
        let Some(got) = search.poll() else {
            ctx.request_repaint_after(REDRAW);
            return;
        };
        s.people.search = None;
        // ⚠ Final review: the search reads the decode, where people already removed still stand.
        let got = got.map(|found| {
            let n = found.len();
            (n, people_tool::not_removed(found, &s.edits.removals))
        });
        match got {
            Ok((0, _)) => s.people.said = Some(people_tool::summary(0, 0)),
            Ok((_, people)) if people.is_empty() => {
                s.people.said = Some("No new people found".to_string());
            }
            Ok((_, people)) => {
                s.people.ticked = people_tool::pre_ticked(&people);
                s.people.people = people;
            }
            Err(e) => s.people.said = Some(format!("Could not find people: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustroom_app::people_tool::{Person, Search, Wait};

    /// An app with a photo open and its own empty models folder.
    fn app_with_models(tag: &str) -> (RustroomApp, egui::Context, PathBuf) {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        let d = test_dir::path(&format!("people-flow-{tag}"));
        let models = d.join("models");
        std::fs::create_dir_all(&models).unwrap();
        app.ai.dirs = Some((d.join("cache"), models.clone()));
        (app, ctx, models)
    }

    fn person(tall: f32) -> Person {
        Person {
            bbox: [0.1, 0.1, 0.2, 0.1 + tall],
            score: 0.9,
            tall,
            area: Arc::new(rustroom_app::remove_tool::Area {
                rect: [0.0; 4],
                side: 1,
                alpha: vec![0.0],
            }),
            outline: Vec::new(),
        }
    }

    fn people(app: &RustroomApp) -> &rustroom_app::people_tool::Run {
        &app.session.as_ref().unwrap().people
    }

    /// Review Focus 3: a press with the models missing opens the download dialog and waits; closing it without
    /// them ends the wait, said.
    #[test]
    fn a_cancelled_download_ends_the_wait() {
        let (mut app, ctx, _) = app_with_models("cancel");
        app.session.as_mut().unwrap().people.pressed = true;
        app.pump_people(&ctx);
        assert!(
            app.modal == crate::modals::Modal::AiDownload,
            "the dialog opened"
        );
        assert!(app.ai.proposed.contains(&ai_mask::ModelId::YoloxM));
        assert_eq!(people(&app).waiting, Some(Wait::Models));
        app.pump_people(&ctx);
        assert_eq!(
            people(&app).waiting,
            Some(Wait::Models),
            "waits while the dialog is open"
        );
        app.close_ai_download();
        app.pump_people(&ctx);
        assert!(people(&app).waiting.is_none() && people(&app).search.is_none());
        assert_eq!(
            people(&app).said.as_deref(),
            Some("Find people needs its models")
        );
    }

    /// With the models there and the photo loaded, a press starts the search — here over stand-in model files,
    /// so it ends in its failure, said.
    #[test]
    fn a_press_with_models_and_photo_starts_the_search() {
        let (mut app, ctx, models) = app_with_models("start");
        for m in std::iter::once(ai_mask::ModelId::YoloxM).chain(ai_mask::registry::CLICK_MODELS) {
            std::fs::File::create(models.join(m.spec().file))
                .unwrap()
                .set_len(m.spec().bytes)
                .unwrap();
        }
        app.session.as_mut().unwrap().buffer = Arc::new(core_image::ImageBuf::from_vec(
            1600,
            1200,
            vec![0.2; 1600 * 1200 * 3],
        ));
        app.session.as_mut().unwrap().people.pressed = true;
        app.pump_people(&ctx);
        assert!(
            app.modal == crate::modals::Modal::None,
            "no dialog: the models are there"
        );
        assert!(people(&app).search.is_some(), "the search started");
        let start = std::time::Instant::now();
        while people(&app).search.is_some() {
            app.pump_people(&ctx);
            assert!(start.elapsed().as_secs() < 60, "the search never landed");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let said = people(&app).said.clone().unwrap_or_default();
        assert!(said.starts_with("Could not find people"), "{said}");
    }

    /// The photo still loading: the press waits for it.
    #[test]
    fn a_press_waits_for_the_photo() {
        let (mut app, ctx, models) = app_with_models("photo");
        for m in std::iter::once(ai_mask::ModelId::YoloxM).chain(ai_mask::registry::CLICK_MODELS) {
            std::fs::File::create(models.join(m.spec().file))
                .unwrap()
                .set_len(m.spec().bytes)
                .unwrap();
        }
        app.session.as_mut().unwrap().people.pressed = true;
        app.pump_people(&ctx);
        assert_eq!(people(&app).waiting, Some(Wait::Photo));
        assert!(people(&app).search.is_none());
    }

    /// Review Focus 2: the people are found on the positive of a film negative.
    #[test]
    fn the_search_reads_the_positive() {
        let buffer = core_image::ImageBuf::from_vec(64, 48, vec![0.05; 64 * 48 * 3]);
        let mut neg = edit_model::Negative::default();
        let plain = people_input(&buffer, 0, &neg);
        neg.on = true;
        neg.base = [0.5, 0.4, 0.3];
        assert!(neg.is_active(), "the fixture: an active negative");
        let pos = people_input(&buffer, 0, &neg);
        assert_ne!(
            plain.as_raw(),
            pos.as_raw(),
            "the negative's positive, not its raw tones"
        );
    }

    /// A search that lands: its people shown, the small ones pre-ticked.
    #[test]
    fn a_search_lands_its_people_pre_ticked() {
        let (mut app, ctx, _) = app_with_models("land");
        let (search, tx) = Search::answered_by(0.5);
        app.session.as_mut().unwrap().people.search = Some(search);
        tx.send(Ok(vec![person(0.62), person(0.10), person(0.20)]))
            .unwrap();
        app.pump_people(&ctx);
        let p = people(&app);
        assert!(p.search.is_none());
        assert_eq!(p.people.len(), 3);
        assert_eq!(p.ticked, vec![false, true, true]);
        assert!(
            p.said.is_none(),
            "the summary is drawn live from the people"
        );
    }

    /// Everyone found was already removed: said so, nothing shown.
    #[test]
    fn people_already_removed_are_said_not_shown() {
        let (mut app, ctx, _) = app_with_models("again");
        let found = person(0.20);
        app.session.as_mut().unwrap().edits.removals = vec![edit_model::Removal {
            points: Vec::new(),
            radius: 0.0,
            detect: false,
            variant: 0,
            opacity: 100.0,
            person: Some(found.bbox),
        }];
        let (search, tx) = Search::answered_by(0.5);
        app.session.as_mut().unwrap().people.search = Some(search);
        tx.send(Ok(vec![found])).unwrap();
        app.pump_people(&ctx);
        assert!(people(&app).people.is_empty());
        assert_eq!(people(&app).said.as_deref(), Some("No new people found"));
    }

    /// Nobody found: said, and nothing shown.
    #[test]
    fn nobody_found_is_said() {
        let (mut app, ctx, _) = app_with_models("nobody");
        let (search, tx) = Search::answered_by(0.5);
        app.session.as_mut().unwrap().people.search = Some(search);
        tx.send(Ok(Vec::new())).unwrap();
        app.pump_people(&ctx);
        assert_eq!(people(&app).said.as_deref(), Some("No people found"));
        assert!(people(&app).people.is_empty());
    }
}
