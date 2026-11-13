//! Reopening where you left off (M11.4): the Library's last source, with its photo selected.
//!
//! On the first frame [`RustroomApp::pump_place`] opens the stored [`start::Place`] when it can still be opened
//! and the setting is on; from then on it MIRRORS what the Library shows into the settings — nothing open
//! included, so a Library closed by forgetting a folder or deleting a collection is not reopened next time.
//! ⚠⚠ One derived writer, not one call at each of the ~ten sites that set `library = None` (the spec's call).

use crate::{LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::start;

/// Where the reopen stands.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum PlaceState {
    /// The first frame has not run: nothing may be written yet, or the empty Library of a launch would
    /// overwrite the place before it was read.
    #[default]
    NotTried,
    /// Opened, and waiting for the Library to show it (an Immich source lands seconds later) to select the photo.
    Pending(start::Place),
    /// Following the Library.
    Mirroring,
}

/// The bin's source as the settings hold it.
pub(crate) fn to_place(source: &LibrarySource) -> start::Source {
    use start::Source as S;
    match source.clone() {
        LibrarySource::Folder(path) => S::Folder { path },
        LibrarySource::AllPhotos => S::AllPhotos,
        LibrarySource::Smart { id, name } => S::Smart { id, name },
        LibrarySource::Periods { periods, label } => S::Periods { periods, label },
        LibrarySource::Collection { id, name } => S::Collection { id, name },
        LibrarySource::Publish { id, name } => S::Publish { id, name },
        LibrarySource::Keyword { id, name } => S::Keyword { id, name },
        LibrarySource::Person { id, name } => S::Person { id, name },
        LibrarySource::ImmichPerson { id, person, name } => S::ImmichPerson { id, person, name },
        LibrarySource::ImmichAlbum { id, name } => S::ImmichAlbum { id, name },
        LibrarySource::ImmichBucket { bucket, label } => S::ImmichBucket { bucket, label },
    }
}

/// The settings' source as the bin's.
pub(crate) fn from_place(source: &start::Source) -> LibrarySource {
    use start::Source as S;
    match source.clone() {
        S::Folder { path } => LibrarySource::Folder(path),
        S::AllPhotos => LibrarySource::AllPhotos,
        S::Smart { id, name } => LibrarySource::Smart { id, name },
        S::Periods { periods, label } => LibrarySource::Periods { periods, label },
        S::Collection { id, name } => LibrarySource::Collection { id, name },
        S::Publish { id, name } => LibrarySource::Publish { id, name },
        S::Keyword { id, name } => LibrarySource::Keyword { id, name },
        S::Person { id, name } => LibrarySource::Person { id, name },
        S::ImmichPerson { id, person, name } => LibrarySource::ImmichPerson { id, person, name },
        S::ImmichAlbum { id, name } => LibrarySource::ImmichAlbum { id, name },
        S::ImmichBucket { bucket, label } => LibrarySource::ImmichBucket { bucket, label },
    }
}

/// Whether `a` and `b` are the same source, names aside — a collection renamed since, or an Immich person whose
/// name the server spells otherwise, is still the place.
fn same_source(a: &start::Source, b: &start::Source) -> bool {
    use start::Source as S;
    match (a, b) {
        (S::Folder { path: x }, S::Folder { path: y }) => x == y,
        (S::AllPhotos, S::AllPhotos) => true,
        (S::Periods { periods: x, .. }, S::Periods { periods: y, .. }) => x == y,
        (S::Smart { id: x, .. }, S::Smart { id: y, .. })
        | (S::Collection { id: x, .. }, S::Collection { id: y, .. })
        | (S::Publish { id: x, .. }, S::Publish { id: y, .. })
        | (S::Keyword { id: x, .. }, S::Keyword { id: y, .. })
        | (S::Person { id: x, .. }, S::Person { id: y, .. }) => x == y,
        (S::ImmichPerson { id: x, .. }, S::ImmichPerson { id: y, .. })
        | (S::ImmichAlbum { id: x, .. }, S::ImmichAlbum { id: y, .. }) => x == y,
        (S::ImmichBucket { bucket: x, .. }, S::ImmichBucket { bucket: y, .. }) => x == y,
        _ => false,
    }
}

impl RustroomApp {
    /// Once a frame, before the panels: the reopen on the first, the mirror after it.
    pub(crate) fn pump_place(&mut self, ctx: &egui::Context) {
        match std::mem::take(&mut self.place_state) {
            PlaceState::NotTried => self.place_state = self.reopen_last_place(ctx),
            PlaceState::Pending(place) => self.place_state = self.land_reopen(place),
            PlaceState::Mirroring => {
                self.place_state = PlaceState::Mirroring;
                self.mirror_place();
            }
        }
    }

    /// Open the stored place, if reopening is on and it can still be opened.
    fn reopen_last_place(&mut self, ctx: &egui::Context) -> PlaceState {
        let Some(place) = self.settings.last_place.clone() else {
            return PlaceState::Mirroring;
        };
        if !self.settings.reopen_last {
            return PlaceState::Mirroring;
        }
        // ⚠ The rail has not drawn yet on the first frame, so its catalog facts are read here.
        let immich_root = self.immich_cache_root();
        self.rail_data
            .refresh(self.catalog.as_ref(), immich_root.as_deref());
        let connected = rustroom_app::immich_source::connection(&self.settings).is_some();
        if !start::reopenable(&place.source, &self.rail_data, connected, |p| p.is_dir()) {
            return PlaceState::Mirroring;
        }
        let person = |id: i64| self.rail_data.people.iter().find(|p| p.id == id).cloned();
        match from_place(&place.source) {
            LibrarySource::Folder(dir) => self.open_catalog_folder(ctx, &dir),
            LibrarySource::AllPhotos => self.open_all_photos(ctx),
            LibrarySource::Smart { id, name } => self.open_smart(ctx, id, &name),
            LibrarySource::Periods { periods, label } => self.open_periods(ctx, &periods, &label),
            LibrarySource::Collection { id, name } => self.open_collection(ctx, id, &name),
            LibrarySource::Publish { id, name } => self.open_publish(ctx, id, &name),
            LibrarySource::Keyword { id, name } => self.open_keyword(ctx, id, &name),
            LibrarySource::Person { id, .. } | LibrarySource::ImmichPerson { person: id, .. } => {
                if let Some(row) = person(id) {
                    self.open_person(ctx, &row);
                }
            }
            LibrarySource::ImmichAlbum { id, name } => self.open_immich_album_by_id(ctx, id, name),
            LibrarySource::ImmichBucket { bucket, label } => {
                self.open_immich_bucket_by_id(ctx, bucket, label)
            }
        }
        // ⚠ Landed at once for a local source; an Immich one waits for its reply.
        self.land_reopen(place)
    }

    /// Select the stored photo once the Library shows the place; give up when it shows another source, or when an
    /// Immich reply failed (nothing is loading and nothing is open — the status line says why).
    fn land_reopen(&mut self, place: start::Place) -> PlaceState {
        let Some(lib) = self.library.as_mut() else {
            return if self.immich_opening.is_some() {
                PlaceState::Pending(place)
            } else {
                PlaceState::Mirroring
            };
        };
        // ⚠ Another source while waiting means the author clicked one: that wins, and the stored photo is not
        // forced onto it.
        if !same_source(&to_place(&lib.source), &place.source) {
            return PlaceState::Mirroring;
        }
        if let Some((path, copy)) = &place.selected {
            if let Some(i) = lib
                .entries
                .iter()
                .position(|e| e.id.path == *path && e.id.copy == *copy)
            {
                lib.selection.click(i);
                lib.reveal = true;
            }
        }
        PlaceState::Mirroring
    }

    /// The place as it is now: the Library's source and its active photo.
    fn current_place(&self) -> Option<start::Place> {
        let lib = self.library.as_ref()?;
        let selected = lib
            .selection
            .active()
            .and_then(|i| lib.entries.get(i))
            .map(|e| (e.id.path.clone(), e.id.copy));
        Some(start::Place {
            source: to_place(&lib.source),
            selected,
        })
    }

    /// Store the place when the Library's source changed — or closed — since it was last stored.
    fn mirror_place(&mut self) {
        let now = self.current_place();
        let stored = self.settings.last_place.as_ref().map(|p| &p.source);
        if now.as_ref().map(|p| &p.source) == stored {
            return;
        }
        self.settings.last_place = now;
        self.save_settings();
    }

    /// What a close request does here, on every platform (`shell`): the macOS exit workaround stays there.
    pub(crate) fn on_close_requested(&mut self) {
        self.remember_place_on_close();
    }

    /// When the window closes: the place with its photo — the one open in Develop if any, since that is the one the
    /// author was on, else the Library's active photo.
    pub(crate) fn remember_place_on_close(&mut self) {
        if self.place_state != PlaceState::Mirroring {
            return;
        }
        let Some(mut place) = self.current_place() else {
            return;
        };
        if let Some(s) = &self.session {
            place.selected = Some((s.path.clone(), s.copy));
        }
        if self.settings.last_place.as_ref() != Some(&place) {
            self.settings.last_place = Some(place);
            self.save_settings();
        }
    }
}

#[cfg(test)]
#[path = "place_tests.rs"]
mod tests;
