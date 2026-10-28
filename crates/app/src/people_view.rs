//! Library → People (M10.18c): who is in the photos, and the faces still to name.

use crate::{theme, RustroomApp};
use eframe::egui;

pub fn show(_app: &mut RustroomApp, ui: &mut egui::Ui, _ctx: &egui::Context) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.centered_and_justified(|ui| {
            theme::empty_state(ui, "No people yet.");
        });
    });
}
