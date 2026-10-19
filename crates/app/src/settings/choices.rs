//! The closed choices: theme, accent, interface scale, the drag bindings and drag quality.

use serde::{Deserialize, Serialize};

/// Which theme to render in.
///
/// ⚠ A mirror of `egui::ThemePreference`, which `CLAUDE.md` warns against on the grounds that
/// "a mapping that exists twice is one that will disagree" — the reason `Demosaic` lives in
/// `raw-decode` rather than being duplicated. Accepted here because the direction is forced:
/// this crate's **lib** must not depend on egui, since that separation is what keeps
/// `history`, `selection` and the rest testable without a windowing system. The mitigation is
/// that `theme.rs` maps it with three exhaustive match arms, so a new variant is a compile
/// error rather than a silent gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
            ThemeChoice::System => "Follow system",
        }
    }
}

/// The colour used for *active* state — slider fill and handle, the open section, the primary
/// button.
///
/// ⚠ `Neutral` is not an absence: it is a grey, so "no accent" is a real choice rather than a
/// discarded one. Its cost is that active state falls back to lightness alone, which is
/// genuinely harder to read.
///
/// ⚠ **Orange and Sage were deliberately absent until 2026-09-29**, and the objection still
/// stands: a warm accent sits on the skin-tone and colour-temperature axis that Temp and Tint
/// exist to judge, and green is the complement the eye adapts against. They are offered because
/// the author asked for more choice (`docs/DESIGN.md` §4.2); the **default stays Steel blue**, and
/// Orange's hover in Settings says why a warm accent can colour a white-balance judgement.
///
/// ⚠ Serialised by variant name, so a settings file from before the new variants loads unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Accent {
    Neutral,
    #[default]
    SteelBlue,
    Teal,
    Violet,
    Orange,
    Sage,
    Rose,
}

impl Accent {
    /// The order the Settings swatches show them: the default first, Neutral last.
    pub const ALL: [Accent; 7] = [
        Accent::SteelBlue,
        Accent::Teal,
        Accent::Violet,
        Accent::Orange,
        Accent::Sage,
        Accent::Rose,
        Accent::Neutral,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Accent::Neutral => "Neutral",
            Accent::SteelBlue => "Steel blue",
            Accent::Teal => "Teal",
            Accent::Violet => "Violet",
            Accent::Orange => "Orange",
            Accent::Sage => "Sage",
            Accent::Rose => "Rose",
        }
    }
}

/// How large the interface is drawn, as egui's zoom factor.
///
/// ⚠ An enum, not an `f32`: `Settings` derives `Eq`, and three steps are what a person can tell
/// apart. Nothing in Settings could make the interface bigger before this (A-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UiScale {
    #[default]
    Normal,
    Larger,
    Largest,
}

impl UiScale {
    pub const ALL: [UiScale; 3] = [Self::Normal, Self::Larger, Self::Largest];

    pub fn factor(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Larger => 1.25,
            Self::Largest => 1.5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "100 %",
            Self::Larger => "125 %",
            Self::Largest => "150 %",
        }
    }
}

/// How a *drag* pans the canvas.
///
/// ⚠ Scroll and two-finger swipe always pan and are deliberately **not** configurable — they are
/// the universal route that works on every device, and making them optional would let a user
/// lock themselves out of panning entirely. Only the extra *drag* gesture is a setting.
///
/// ⚠ A closed three-value enum, not a keybinding system. The risk in this area is ballooning
/// into general rebinding; three values already cover every device. A mouse button is not a
/// modifier, which is why the set spans both kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PanDrag {
    /// Middle-button drag. ⚠ Unavailable on an Apple Magic Mouse, which has no middle button —
    /// those users pan by swiping, which always works.
    #[default]
    Middle,
    /// Right-button drag. Free: nothing on the canvas uses secondary click or a context menu.
    Secondary,
    /// Hold Space and left-drag. The only option needing no extra hardware, and the
    /// Photoshop/Figma convention.
    SpaceLeft,
}

impl PanDrag {
    /// Every variant, for the settings dropdown and for table-driven tests.
    pub const ALL: [PanDrag; 3] = [PanDrag::Middle, PanDrag::Secondary, PanDrag::SpaceLeft];

    /// How to describe this binding in the UI.
    ///
    /// ⚠ The toolbar hint must name the *configured* binding rather than saying "middle-drag"
    /// literally, or it becomes a lie the moment the setting changes. Same shape as
    /// `Mask::label()`.
    pub fn label(self) -> &'static str {
        match self {
            PanDrag::Middle => "middle-drag",
            PanDrag::Secondary => "right-drag",
            PanDrag::SpaceLeft => "Space+drag",
        }
    }
}

/// Which held mouse button makes a brush stroke erase, for the duration of the gesture.
///
/// ⚠⚠ **The pan binding always wins.** [`PanDrag`] is a deliberate choice about how to move around
/// the photograph, where this is a convenience — so a button bound to panning is never also an
/// erase, and the gesture is simply unavailable rather than both firing. That is the precedence
/// `tool_drag` already applies by refusing middle and secondary drags outright, so the pan gesture
/// cannot drag crop rectangles and mask handles as it passes over them.
///
/// ⚠ `Off` earns its place: an accidental right-drag erases part of a mask, and someone who finds
/// that destructive needs a way to turn it off that is not "bind panning to the same button",
/// which is exactly the kind of accident a setting should prevent rather than require.
///
/// ⚠ A closed enum, not a keybinding system — [`PanDrag`]'s reasoning, for its reason. A modifier
/// key was considered and left out: it would need its own keyboard-focus gating, which
/// [`PanDrag::SpaceLeft`] already documents as a trap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EraseDrag {
    /// Right-button drag. The default: nothing else on the canvas uses secondary click.
    #[default]
    Secondary,
    /// Middle-button drag. ⚠ Unavailable on an Apple Magic Mouse, which has no middle button.
    Middle,
    /// No erase gesture at all; the Erase checkbox is the only route.
    Off,
}

/// What a drag inside the tone curve widget does in its **Parametric** mode (M5.1c).
///
/// ⚠ The widget is in the panel, not on the canvas, so no pan binding competes with it — which is
/// why this is two values and not `EraseDrag`'s three. `Off` is for someone who finds an accidental
/// drag over the curve destructive: the four sliders remain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ParametricDrag {
    /// A vertical left-drag adjusts the region under the pointer, as Lightroom's does.
    #[default]
    Left,
    /// No drag gesture; the sliders are the only route.
    Off,
}

impl ParametricDrag {
    /// Every variant, for the settings dropdown and for table-driven tests.
    pub const ALL: [ParametricDrag; 2] = [ParametricDrag::Left, ParametricDrag::Off];

    /// How to describe this binding in the UI. See [`PanDrag::label`] for why it is named here.
    pub fn label(self) -> &'static str {
        match self {
            ParametricDrag::Left => "drag on the curve",
            ParametricDrag::Off => "off",
        }
    }
}

/// What reorders the photos of an unfurled stack in the Library grid (M10.8c, after use).
///
/// ⚠ A drag that starts anywhere else always draws the selection rectangle; this decides only what a
/// drag that starts **on a member of an unfurled stack** does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StackDrag {
    /// A plain drag moves the photo — the author's choice.
    #[default]
    Drag,
    /// Only a drag with ⌥ held moves it; a plain one draws the selection rectangle as anywhere else.
    AltDrag,
}

impl StackDrag {
    /// Every variant, for the settings dropdown and for table-driven tests.
    pub const ALL: [StackDrag; 2] = [StackDrag::Drag, StackDrag::AltDrag];

    /// How to describe this binding in the UI. See [`PanDrag::label`] for why it is named here.
    pub fn label(self) -> &'static str {
        match self {
            StackDrag::Drag => "drag a photo",
            StackDrag::AltDrag => "⌥ + drag a photo",
        }
    }

    /// Whether a drag with ⌥ held (`alt`) or not reorders.
    pub fn reorders(self, alt: bool) -> bool {
        match self {
            StackDrag::Drag => true,
            StackDrag::AltDrag => alt,
        }
    }
}

/// A mouse button, named here so this module needs no `egui` dependency.
///
/// ⚠ `crates/app/src/lib.rs` has no egui by rule — that separation is what keeps these types
/// testable without a window. The bin maps these onto `egui::PointerButton`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragButton {
    Secondary,
    Middle,
}

impl EraseDrag {
    /// Every variant, for the settings dropdown and for table-driven tests.
    pub const ALL: [EraseDrag; 3] = [EraseDrag::Secondary, EraseDrag::Middle, EraseDrag::Off];

    /// How to describe this binding in the UI.
    ///
    /// ⚠ Named rather than spelled out at the call site, so a hint cannot go stale the moment the
    /// setting changes — [`PanDrag::label`]'s reason.
    pub fn label(self) -> &'static str {
        match self {
            EraseDrag::Secondary => "right-drag",
            EraseDrag::Middle => "middle-drag",
            EraseDrag::Off => "off",
        }
    }

    /// The button this binds to, or `None` when the gesture is off **or when panning has already
    /// claimed that button**.
    ///
    /// ⚠⚠ One function decides both, so "the pan binding wins" is written once. Two places
    /// deciding it is how a build appears in which a right-drag both pans and erases.
    pub fn button(self, pan: PanDrag) -> Option<DragButton> {
        let want = match self {
            EraseDrag::Secondary => DragButton::Secondary,
            EraseDrag::Middle => DragButton::Middle,
            EraseDrag::Off => return None,
        };
        let taken = match pan {
            PanDrag::Middle => Some(DragButton::Middle),
            PanDrag::Secondary => Some(DragButton::Secondary),
            PanDrag::SpaceLeft => None,
        };
        (Some(want) != taken).then_some(want)
    }
}

/// How much resolution a live gesture may trade for responsiveness.
///
/// ⚠⚠ **`Full` is zero rungs dropped, and that is the whole reason this is a setting rather than
/// a constant.** When the GPU port lands and the tail runs 5–15× faster, the feature is switched
/// off from Settings rather than removed from the code — **the setting is the deletion
/// mechanism.** It also lets a slower or faster machine pick its own point on the trade, which a
/// hardcoded rung cannot.
///
/// ⚠ `ALL`, `label` and `rungs_dropped` live **beside the type**, the rule `EDIT_GROUP_NAMES`,
/// `Tool::ALL` and `FlagSet::ALL` already follow, so the Settings UI cannot invent a second
/// ordering or a second mapping.
///
/// ⚠ `Fastest` differs from `Faster` **only when zoomed**: fitted, a two-rung drop from 3200
/// clamps at `fit_cap` and lands where one rung already did. Recorded so it is not later filed
/// as a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DragQuality {
    /// Render a drag at the same rung as an idle view — the behaviour before this existed.
    Full,
    #[default]
    Faster,
    Fastest,
}

impl DragQuality {
    pub const ALL: [DragQuality; 3] =
        [DragQuality::Full, DragQuality::Faster, DragQuality::Fastest];

    pub fn label(self) -> &'static str {
        match self {
            DragQuality::Full => "Full quality",
            DragQuality::Faster => "Faster",
            DragQuality::Fastest => "Fastest",
        }
    }

    /// How many rungs of the render ladder a live gesture drops.
    pub fn rungs_dropped(self) -> u32 {
        match self {
            DragQuality::Full => 0,
            DragQuality::Faster => 1,
            DragQuality::Fastest => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    /// ⚠⚠ **The pan binding always wins, decided in one place.** Two places deciding it is how a
    /// build appears in which a right-drag both pans the photograph and erases into it.
    #[test]
    fn the_pan_binding_takes_the_button_from_the_erase_gesture() {
        assert_eq!(
            EraseDrag::Secondary.button(PanDrag::Middle),
            Some(DragButton::Secondary)
        );
        assert_eq!(
            EraseDrag::Secondary.button(PanDrag::SpaceLeft),
            Some(DragButton::Secondary)
        );
        assert_eq!(
            EraseDrag::Secondary.button(PanDrag::Secondary),
            None,
            "right-drag is bound to panning, so it must not also erase"
        );
        assert_eq!(
            EraseDrag::Middle.button(PanDrag::Middle),
            None,
            "middle-drag is bound to panning, so it must not also erase"
        );
        assert_eq!(
            EraseDrag::Middle.button(PanDrag::Secondary),
            Some(DragButton::Middle)
        );
    }

    /// ⚠ `Off` is off under every pan binding — a test of the conflict alone would pass against a
    /// version that only ever disabled the gesture by collision.
    #[test]
    fn the_erase_gesture_can_be_switched_off_outright() {
        for pan in PanDrag::ALL {
            assert_eq!(EraseDrag::Off.button(pan), None, "with pan {pan:?}");
        }
    }

    #[test]
    fn drag_quality_maps_to_rungs() {
        // ⚠⚠ `Full` must be exactly zero. It is the off switch: when the GPU port lands, the
        // feature is disabled from Settings rather than removed from the code, and
        // `rung_below` returns its input unchanged at n == 0.
        assert_eq!(DragQuality::Full.rungs_dropped(), 0);
        assert_eq!(DragQuality::Faster.rungs_dropped(), 1);
        assert_eq!(DragQuality::Fastest.rungs_dropped(), 2);
        // Every variant is in ALL, so the Settings combo cannot silently omit one.
        assert_eq!(DragQuality::ALL.len(), 3);
        for q in DragQuality::ALL {
            assert!(!q.label().is_empty(), "{q:?} has no label");
        }
    }

    #[test]
    fn every_accent_and_theme_is_in_its_table() {
        // The dialog and the colour lookup both iterate `ALL`; a variant missing from it is
        // invisible in the UI rather than a compile error, so it is worth an explicit check.
        assert_eq!(Accent::ALL.len(), 7);
        assert_eq!(ThemeChoice::ALL.len(), 3);
        for a in Accent::ALL {
            assert!(!a.label().is_empty());
        }
        for t in ThemeChoice::ALL {
            assert!(!t.label().is_empty());
        }
    }

    #[test]
    fn every_pan_drag_has_a_label() {
        // A table over every variant so a fourth cannot ship showing nothing in the toolbar
        // hint. The same shape `every_demosaic_tier_has_a_cost_hint` and `Mask::label` use.
        for b in PanDrag::ALL {
            assert!(!b.label().is_empty(), "{b:?} has no label");
        }
    }

    #[test]
    fn the_pan_drag_labels_are_distinct() {
        // Two variants sharing a label would make the hint ambiguous about which is active.
        for (i, a) in PanDrag::ALL.iter().enumerate() {
            for b in &PanDrag::ALL[i + 1..] {
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
            }
        }
    }

    #[test]
    fn the_default_pan_drag_is_the_middle_button() {
        assert_eq!(PanDrag::default(), PanDrag::Middle);
        assert_eq!(Settings::default().pan_drag, PanDrag::Middle);
    }

    #[test]
    fn a_stack_reorders_on_a_plain_drag_or_only_with_option() {
        assert!(StackDrag::Drag.reorders(false) && StackDrag::Drag.reorders(true));
        assert!(!StackDrag::AltDrag.reorders(false) && StackDrag::AltDrag.reorders(true));
        assert_eq!(StackDrag::default(), StackDrag::Drag, "the author's choice");
    }
}

/// AI denoise's network (M10.16b6): **FFDNet** by default — ten times lighter than DRUNet, the author's
/// pick at the noise map × 2 — or **DRUNet**, kept in Settings, which holds a little more fine detail.
///
/// ⚠ Every place that names the network reads it from here: the job's factory, the download offer, export,
/// the cache key and the rendered preview's key — or one network's pixels pass for the other's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum DenoiseNetwork {
    #[default]
    Ffdnet,
    Drunet,
}

impl DenoiseNetwork {
    pub const ALL: [DenoiseNetwork; 2] = [DenoiseNetwork::Ffdnet, DenoiseNetwork::Drunet];

    pub fn label(self) -> &'static str {
        match self {
            DenoiseNetwork::Ffdnet => "FFDNet",
            DenoiseNetwork::Drunet => "DRUNet",
        }
    }

    /// Its model file.
    pub fn model(self) -> ai_mask::ModelId {
        match self {
            DenoiseNetwork::Ffdnet => ai_mask::ModelId::FfdnetColor,
            DenoiseNetwork::Drunet => ai_mask::ModelId::DrunetColor,
        }
    }

    /// Its results' recipe in the cache key — DRUNet's unchanged since M10.16b3, so its results stay valid.
    pub fn recipe(self) -> &'static str {
        match self {
            DenoiseNetwork::Ffdnet => "denoise:v3-ffdnet",
            DenoiseNetwork::Drunet => "denoise:v2",
        }
    }
}
