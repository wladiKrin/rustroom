//! What each command does, in one sentence (M10.9b, after use: "it might not be clear what e.g. *Make
//! top of stack* means"). The Keyboard tab shows it on hover over a command's name.
//!
//! ⚠ A lib module, exhaustive over [`Command`]: a new command is a compile error here until it is
//! explained, and `every_command_says_what_it_does` holds each sentence to the hover rule (one short
//! sentence, no full stop — `docs/DESIGN.md` §8).

use crate::menu::Command;
use crate::tool::Tool;

/// What `c` does, in one sentence.
pub fn describe(c: Command) -> String {
    use Command as C;
    let s: &str = match c {
        C::OpenFile => "Opens one photo file to edit",
        C::OpenFolder => "Opens a folder of photos in the Library",
        C::OpenRecent(_) => "Opens the folder you opened last",
        C::Import => "Copies new photos from a memory card into dated folders",
        C::Export => "Exports the selected photos with their edits",
        C::BuildPreviews => "Renders full-size previews of the selected photos ahead of time",
        C::Settings => "Opens the settings",
        C::Undo => "Undoes the last change",
        C::Redo => "Redoes the last undone change",
        C::TakeSnapshot => "Keeps this photo's current settings as a snapshot to return to",
        C::CreateVirtualCopy => "Makes a second version of the photo to edit on its own",
        C::DeleteVirtualCopy => "Deletes this virtual copy and leaves the original",
        C::CopySettings => "Copies this photo's settings",
        C::PasteSettings => "Pastes the copied settings onto the selected photos",
        C::PasteSettingsWith => "Pastes only the groups of the copied settings you choose",
        C::SyncSelection => {
            "Gives the selected photos this photo's settings, the groups you choose"
        }
        C::ResetAdjustments => "Puts this photo's adjustments back to neutral",
        C::ShowLibrary => "Shows the Library's grid of photos",
        C::ShowDevelop => "Shows the photo in Develop to edit it",
        C::ShowCompare => "Shows two photos side by side to choose between",
        C::ShowSurvey => "Shows the selected photos together to pick from",
        C::ZoomIn => "Zooms in",
        C::ZoomOut => "Zooms out",
        C::FitView => "Fits the whole photo in the view",
        C::ZoomActual => "Zooms to 100 %, one photo pixel per screen pixel",
        C::ToggleBeforeAfter => "Shows the photo before and after its edits",
        C::ToggleFilmstrip => "Shows or hides the filmstrip",
        C::RenamePhotos => "Renames the selected photos' files from a template",
        C::MergeHdr => "Merges the selected bracket of exposures into one HDR photo",
        C::MergeHdrLast => {
            "Merges the selected bracket with the last HDR options, without the dialog"
        }
        C::Stack => "Groups the selected photos into one stack",
        C::Unstack => "Takes apart the stacks the selected photos are in",
        C::MakeTopOfStack => "Makes this photo the one a folded stack shows",
        C::StackByTime(None) => "Takes apart the stacks made by capture time",
        C::StackByTime(Some(secs)) => {
            let gap = match secs {
                s if s >= 60 && s % 60 == 0 => format!("{} min", s / 60),
                s => format!("{s} s"),
            };
            return format!("Stacks photos taken within {gap} of each other");
        }
        C::TogglePanels => "Hides or shows the panels, the rail and the filmstrip",
        C::LightsOut => "Dims everything around the photo, then blacks it out",
        C::ToggleInfoOverlay => "Shows the photo's capture details over its corner",
        C::ToggleClipping => "Marks clipped highlights in red and clipped shadows in blue",
        C::ToggleMaskOverlay => "Shows every mask's overlay on the photo",
        C::CycleCompositionGuide => "Steps through the composition guides over the photo",
        C::SelectTool(Tool::Edit) => "Opens the adjustments",
        C::SelectTool(Tool::Crop) => "Opens cropping, straightening and perspective",
        C::SelectTool(Tool::Masks) => "Opens the masks, to adjust part of the photo",
        C::SelectTool(Tool::Retouch) => "Opens Retouch, to remove spots and fix red eyes",
        C::SelectTool(Tool::Presets) => "Opens the presets",
        C::RotateLeft => "Turns the photo a quarter turn to the left",
        C::RotateRight => "Turns the photo a quarter turn to the right",
        C::SavePreset => "Saves this photo's settings as a preset",
        C::KeyboardShortcuts => "Lists every keyboard shortcut",
        C::About => "Shows the version and the licence",
        C::SoftProof(None) => "Stops showing the photo as another colour space would",
        C::SoftProof(Some(space)) => {
            return format!("Shows the photo as it will look in {}", space.label());
        }
        C::GamutWarning => "Marks the colours the proof's colour space cannot hold",
        C::RenderingIntent(color::RenderingIntent::Clip) => {
            "Clips colour the screen cannot show, keeping it vivid"
        }
        C::RenderingIntent(color::RenderingIntent::RelativeColorimetric) => {
            "Moves colour the screen cannot show to its edge, keeping the hue"
        }
        C::RenderingIntent(color::RenderingIntent::Perceptual) => {
            "Eases strong colour in to fit the screen, keeping its gradation"
        }
    };
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command a shortcut can name says what it does, in one short sentence.
    #[test]
    fn every_command_says_what_it_does() {
        let entries = crate::shortcuts::entries(&[]);
        assert!(entries.len() > 50, "the sample: {}", entries.len());
        for e in entries {
            let d = describe(e.command);
            assert!(!d.is_empty(), "{} says nothing", e.label);
            if let Some(f) = crate::slider_help::sentence_fault(&d) {
                panic!("{}: {f}: {d:?}", e.label);
            }
        }
        assert_eq!(
            describe(Command::StackByTime(Some(5))),
            "Stacks photos taken within 5 s of each other"
        );
    }
}
