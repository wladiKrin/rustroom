//! Settings files from every earlier version still load, and every field round-trips — the
//! guarantee that an upgrade never loses a user's preferences.

use super::*;
use std::path::PathBuf;

#[test]
fn settings_round_trip_carries_the_export_format() {
    let f = TempFile::new("format_rt");
    let store = SettingsStore::new(f.0.clone());
    let s = Settings {
        export: ExportOptions {
            format: export::ExportFormat::Tiff16,
            ..ExportOptions::default()
        },
        ..Settings::default()
    };
    store.save(&s).unwrap();
    assert_eq!(store.load().export.format, export::ExportFormat::Tiff16);
}

/// ⚠⚠ **`SettingsStore::load` is infallible**, so a field that fails to parse silently reverts
/// **every** setting to its default — not just this one. A settings file written before this
/// field existed must still load.
/// ⚠ M5.1c's binding, the same guard: an older file has no `parametric_drag`.
#[test]
fn a_settings_file_from_before_the_parametric_binding_still_loads() {
    let json = r#"{"pan_drag":"Secondary","erase_drag":"Middle"}"#;
    let s: Settings = serde_json::from_str(json).expect("an older settings file must load");
    assert_eq!(s.erase_drag, EraseDrag::Middle, "the stored value survived");
    assert_eq!(s.parametric_drag, ParametricDrag::Left);
}

#[test]
fn a_settings_file_from_before_the_erase_binding_still_loads() {
    let json = r#"{"pan_drag":"Secondary","theme":"Light"}"#;
    let s: Settings = serde_json::from_str(json).expect("an older settings file must load");
    assert_eq!(s.pan_drag, PanDrag::Secondary, "the stored value survived");
    assert_eq!(
        s.erase_drag,
        EraseDrag::default(),
        "the absent field takes the struct's default"
    );
    // ⚠ And with right-drag panning, the default erase binding is correctly unavailable
    // rather than fighting it — the interaction, not just the two values.
    assert_eq!(s.erase_drag.button(s.pan_drag), None);
}

#[test]
fn a_settings_file_holding_gpu_hsl_still_turns_the_gpu_on() {
    // ⚠⚠ **Renaming a serialised field is a MIGRATION, not a rename.** `Settings` carries a
    // container-level `#[serde(default)]`, so without `alias = "gpu_hsl"` an existing file
    // holding `"gpu_hsl": true` would silently fall back to `false` — the user's choice
    // discarded with nothing to notice it. The `EditGroups::effects` → `vignette` rename set
    // this precedent, and its alias test exists because the same trap was live there.
    let f = TempFile::new("gpu_hsl_alias");
    std::fs::write(&f.0, r#"{"gpu_hsl": true}"#).unwrap();
    assert!(
        SettingsStore::new(f.0.clone()).load().gpu_tail,
        "an old gpu_hsl must carry over to gpu_tail"
    );
    // ⚠ And the new name must work too — an alias that REPLACED the field name rather than
    // adding to it would pass the assertion above and break every current file.
    let g = TempFile::new("gpu_tail_name");
    std::fs::write(&g.0, r#"{"gpu_tail": true}"#).unwrap();
    assert!(SettingsStore::new(g.0.clone()).load().gpu_tail);
}

#[test]
fn a_settings_file_from_before_the_export_format_still_loads() {
    // The forgiving-load property `ExportOptions::default()` exists for.
    let f = TempFile::new("format_old");
    std::fs::write(&f.0, r#"{"export":{"quality":80}}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert_eq!(loaded.export.quality, 80, "the old field must survive");
    assert_eq!(loaded.export.format, export::ExportFormat::default());
}

/// ⚠⚠ **A settings file written before this field must get 4, not 0.** The container-level
/// `#[serde(default)]` is what makes that true; a **field**-level one would give `u32`'s own
/// default of 0, which `export_width` repairs to 1 — the feature silently absent for everyone
/// who already has a file, which is most people. This asserts the **value**, so it catches
/// either way round.
#[test]
fn a_settings_file_from_before_the_export_width_still_gets_four() {
    let s: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(s.export_concurrency, DEFAULT_EXPORT_WIDTH);
    assert_eq!(
        s.export_width(),
        4.min(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        )
    );
}

#[test]
fn the_hidden_formats_survive_a_restart() {
    // ⚠ A standing preference — "I shoot RAW+JPEG and only want the RAWs" is true of every
    // folder. Persisting it here is what stops it resetting on each source switch.
    // ⚠ This test alone was blind to the bug it was written for: it proves the *file*, and
    // `the_hidden_formats_reach_the_filter_at_startup` proves the *wiring*.
    let f = TempFile::new("hidden_formats_rt");
    let store = SettingsStore::new(f.0.clone());
    let mut s = Settings::default();
    s.hidden_formats.toggle("jpg");
    store.save(&s).unwrap();
    let loaded = store.load();
    assert!(loaded.hidden_formats.is_hidden("jpg"));
    assert!(!loaded.hidden_formats.is_hidden("raf"));
}

#[test]
fn a_settings_file_from_before_the_format_filter_still_loads() {
    // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
    // *every* setting — which is why each new field needs this.
    let f = TempFile::new("hidden_formats_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert!(
        !loaded.hidden_formats.is_active(),
        "an absent filter must default to hiding nothing"
    );
}

#[test]
fn a_settings_file_from_before_drag_quality_still_loads() {
    // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
    // *every* setting — which is why each new field needs this.
    let f = TempFile::new("drag_quality_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(
        loaded.drag_quality,
        DragQuality::Faster,
        "an absent drag_quality must default to Faster"
    );
}

#[test]
fn a_settings_file_from_before_the_composition_guide_still_loads() {
    // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
    // *every* setting — which is why each new field needs this.
    let f = TempFile::new("composition_guide_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    // ⚠ Asserted as a **value**. An overlay appearing over every photograph on upgrade is a
    // change nobody asked for, and only the value can say the default is the quiet one.
    assert_eq!(
        loaded.composition_guide,
        crate::composition_guides::CompositionGuide::Off,
        "an absent guide must default to Off, not to an overlay"
    );
}

#[test]
fn a_settings_file_from_before_the_panel_sections_still_loads() {
    // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
    // *every* setting — which is why each new field needs this.
    let f = TempFile::new("panel_sections_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(
        loaded.panel_sections,
        PanelSections::default(),
        "an absent value must give the closed default, not a half-filled struct"
    );
    assert!(
        !loaded.panel_sections.folders,
        "and the default really is closed"
    );
}

#[test]
fn a_partial_panel_sections_object_defaults_the_rest() {
    // ⚠ The container-level `#[serde(default)]`: a file naming only one section must still
    // parse — `SettingsStore::load` is infallible, so a failure here silently reverts *every*
    // setting rather than just this one.
    let f = TempFile::new("panel_sections_partial");
    std::fs::write(&f.0, r#"{"panel_sections":{"dates":true}}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(loaded.panel_sections.dates, "the named one is honoured");
    assert!(!loaded.panel_sections.folders, "the rest take the default");
    assert!(!loaded.panel_sections.smart);
}

/// A settings file from before the field loads with the curve open, as it always was.
#[test]
fn a_settings_file_from_before_the_curve_could_collapse_opens_it() {
    let f = TempFile::new("tone_curve_open_absent");
    std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "sample: the file was read");
    assert!(
        loaded.tone_curve_open,
        "an absent field must give the default, open"
    );
}

#[test]
fn settings_round_trip_carries_the_collapsed_sections() {
    let f = TempFile::new("panel_sections_rt");
    let store = SettingsStore::new(f.0.clone());
    let s = Settings {
        panel_sections: PanelSections {
            keywords: true,
            dates: true,
            ..PanelSections::default()
        },
        ..Settings::default()
    };
    store.save(&s).unwrap();
    let got = store.load();
    assert!(got.panel_sections.keywords);
    assert!(got.panel_sections.dates);
    assert!(!got.panel_sections.collections);
}

#[test]
fn settings_round_trip_carries_the_demosaic_choice() {
    let f = TempFile::new("demosaic_rt");
    let store = SettingsStore::new(f.0.clone());
    let s = Settings {
        export: ExportOptions {
            demosaic: raw_decode::Demosaic::Best,
            ..ExportOptions::default()
        },
        ..Settings::default()
    };
    store.save(&s).unwrap();
    assert_eq!(store.load().export.demosaic, raw_decode::Demosaic::Best);
}

#[test]
fn a_settings_file_from_before_the_demosaic_choice_still_loads() {
    // The same forgiving-load property that `ExportOptions::default()` exists for.
    let f = TempFile::new("demosaic_old");
    std::fs::write(
        &f.0,
        r#"{"export":{"dest_dir":"/tmp/x","quality":80,"max_edge":null}}"#,
    )
    .unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert_eq!(loaded.export.quality, 80, "the old fields must survive");
    assert_eq!(loaded.export.demosaic, raw_decode::Demosaic::default());
}

#[test]
fn a_settings_file_from_before_write_xmp_still_loads_and_it_defaults_to_on() {
    // ⚠ The forgiving-load property again, and here the *value* is what discriminates:
    // `bool`'s own default is `false`, so a field-level `#[serde(default)]` would silently
    // turn XMP writing off for every existing user while still parsing cleanly.
    let f = TempFile::new("write_xmp_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert!(
        loaded.write_xmp,
        "XMP writing must default to on for a settings file that predates it"
    );
    assert!(Settings::default().write_xmp);
}

#[test]
fn the_export_options_default_to_unset() {
    let s = Settings::default();
    assert!(s.export.dest_dir.as_os_str().is_empty());
    assert_eq!(s.export.quality, 92);
    assert_eq!(s.export.size, crate::batch_export::OutputSize::default());
}

/// M10.10b: `max_edge` became `size`, and a settings file holding the old field sizes as it did.
#[test]
fn a_settings_file_with_the_old_max_edge_keeps_its_long_edge() {
    let f = TempFile::new("max_edge_old");
    std::fs::write(&f.0, r#"{"export":{"dest_dir":"/tmp/x","max_edge":1600}}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert_eq!(
        loaded.export.size,
        crate::batch_export::OutputSize::long_edge_px(1600)
    );
}

#[test]
fn settings_round_trip_carries_the_export_options() {
    let f = TempFile::new("export_rt");
    let store = SettingsStore::new(f.0.clone());
    let s = Settings {
        // ⚠ Not the default, so the round trip actually carries it — the whole point of this
        // literal being exhaustive is that a new field lands here as a compile error and a
        // decision rather than as a silently defaulted value.
        export_concurrency: DEFAULT_EXPORT_WIDTH + 1,
        recent_folders: vec![std::path::PathBuf::from("/photos/2026-09")],
        ui_scale: UiScale::Larger,
        // Not the default, so the round trip actually carries it.
        hidden_formats: {
            let mut f = crate::format_filter::FormatFilter::default();
            f.toggle("png");
            f
        },
        // ⚠ Not the default either: `enabled: false` is the interesting value, since `true`
        // would round-trip even if the field were dropped.
        stacking: crate::stack::Stacking {
            enabled: false,
            primary: crate::stack::StackPrimary::Jpeg,
            ..Default::default()
        },
        confirm_preset_delete: false,
        confirm_export_preset_delete: false,
        // Listed explicitly rather than via `..Default::default()`: this literal being
        // exhaustive is what makes a new `Settings` field a compile error here, forcing
        // a decision about whether the round trip should carry it.
        view_demosaic: Demosaic::Better,
        gpu_tail: false,
        // Non-default, so a field re-defaulted on load cannot pass this round trip.
        resampler: Resampler::Lanczos3,
        display_space: color::OutputSpace::DisplayP3,
        canvas_intent: color::RenderingIntent::Perceptual,
        camera_profiles: std::collections::BTreeMap::new(),
        theme: ThemeChoice::System,
        accent: Accent::Teal,
        // A non-default value on purpose: it proves the field is actually persisted
        // rather than being re-defaulted on load.
        pan_drag: PanDrag::Secondary,
        erase_drag: EraseDrag::Middle,
        parametric_drag: ParametricDrag::Off,
        // Non-default, so a re-defaulted field cannot pass.
        stack_drag: StackDrag::AltDrag,
        // Both non-default, for the same reason: a field re-defaulted on load would look
        // like it round-tripped.
        library_sort: catalog::SortOrder::ModifiedDesc,
        library_cell: DEFAULT_CELL + CELL_STEP,
        survey_cell: DEFAULT_SURVEY_CELL,
        custom_aspect: DEFAULT_CUSTOM_ASPECT,
        // Likewise non-default, and inside `STRIP_HEIGHT_RANGE` so `clamped_strip_height`
        // cannot mask a field that failed to round-trip.
        snapshot_strip_height: DEFAULT_STRIP_HEIGHT + 20,
        // Non-default so a field re-defaulted on load cannot pass.
        dialog_sizes: std::collections::BTreeMap::from([("x".to_string(), [500u32, 300u32])]),
        // Non-default so a field re-defaulted on load cannot pass. ⚠ Stored *unrepaired* —
        // the repair lives only in the accessor, and this asserts it has not leaked into serde.
        panel_order: vec![PanelSection::Dates, PanelSection::Folders],
        // Non-default in both directions, so a re-defaulted field cannot pass.
        panel_sections: PanelSections {
            folders: true,
            ..PanelSections::default()
        },
        // Non-default in all three, so a re-defaulted layout cannot pass (M10.8b).
        layout: crate::layout::Layout {
            library_rail: crate::layout::Side::Right,
            edit_panel: crate::layout::Side::Left,
            filmstrip: crate::layout::StripPlace::Left,
        },
        // Non-default: it defaults to open, so `false` is what proves it round-trips.
        tone_curve_open: false,
        // Likewise non-default: the preference defaults to *on*, so persisting `false` is
        // what proves it round-trips rather than being re-defaulted.
        // Not the default, so the round trip actually carries it.
        default_rendering: edit_model::Rendering::None,
        auto_lens_correction: false,
        // Non-default so a field re-defaulted on load cannot pass.
        composition_guide: crate::composition_guides::CompositionGuide::Thirds,
        // Likewise non-default: it defaults to *on*, so persisting `false` is what proves
        // it round-trips rather than being re-defaulted.
        write_xmp: false,
        // Non-default for the same reason: `Faster` is the default, so only `Full` proves the
        // value survives a round trip rather than being re-defaulted underneath the test.
        drag_quality: DragQuality::Full,
        // Non-default for the same reason as its neighbours: the preference defaults to *on*,
        // so persisting `false` is what proves it round-trips rather than being re-defaulted.
        auto_advance: false,
        // Non-default so a field re-defaulted on load cannot pass. ⚠ The key is deliberately
        // included: it is the one field whose loss would be silent — browsing would simply
        // report "the server rejected the API key" — so the round trip must cover it.
        immich_url: "https://photos.example.com".to_string(),
        // ⚠ Not the default, so the round trip actually carries it — and `false` is the
        // interesting value here, since `true` would pass even if the field were dropped.
        immich_enabled: false,
        immich_key: "round-trip-key".to_string(),
        // ⚠ Deliberately different from `immich_key`: the round trip must prove the two
        // keys are carried as *separate* fields, not one value written twice.
        immich_upload_key: "round-trip-upload-key".to_string(),
        // Non-default so a field re-defaulted on load cannot pass.
        thumb_budget_mb: 2048,
        immich_thumb_budget_mb: 1024,
        immich_cache_limit_gb: 25,
        immich_cache_warn: false,
        denoise_cache_limit_gb: 30,
        denoise_cache_warn: false,
        // Non-default, so a field re-defaulted on load cannot pass.
        denoise_network: DenoiseNetwork::Drunet,
        preview_demosaic: Some(Demosaic::Fastest),
        preview_budget_mb: 4096,
        export: ExportOptions {
            dest_dir: std::path::PathBuf::from("/tmp/rustroom_out"),
            quality: 71,
            size: crate::batch_export::OutputSize::long_edge_px(2048),
            ..ExportOptions::default()
        },
        // Every field non-default, so a re-defaulted one cannot pass.
        // Non-default, so a re-defaulted field cannot pass.
        rename_template: "Trip-{seq:4}".into(),
        // Every field non-default, so a re-defaulted one cannot pass.
        hdr: crate::hdr_merge::HdrSettings {
            auto_align: false,
            auto_settings: false,
            deghost: photo_merge::deghost::Deghost::High,
            overlay: true,
            create_stack: true,
        },
        // Every field non-default, so a re-defaulted one cannot pass.
        panorama: crate::pano_merge::PanoSettings {
            auto_projection: false,
            projection: photo_merge::projection::Projection::Perspective,
            auto_crop: false,
            fill_edges: true,
            create_stack: true,
        },
        immich_merge_folder: Some(std::path::PathBuf::from("/merged")),
        // Non-default, with ⌥ and a removal, so a re-defaulted field cannot pass.
        shortcuts: vec![
            crate::shortcuts::Rebinding {
                command: "Stack".into(),
                shortcut: Some(crate::menu::Shortcut::cmd('J').with_alt()),
            },
            crate::shortcuts::Rebinding {
                command: "Export".into(),
                shortcut: None,
            },
        ],
        import: crate::ingest::options::ImportOptions {
            destination: std::path::PathBuf::from("/photos"),
            layout: crate::ingest::options::Layout::YearMonthDay,
            subfolder: "Trip".into(),
            backup_on: true,
            backup: std::path::PathBuf::from("/backup"),
            template: "{date}_{seq:4}".into(),
            copyright: "© Test".into(),
            creator: "Tester".into(),
            keywords: "a, b".into(),
            preset: Some("Punchy".into()),
        },
        // Non-default, so a re-defaulted field cannot pass.
        ai_subject_model: ai_mask::SubjectModel::BiRefNetLite,
    };
    store.save(&s).unwrap();
    assert_eq!(store.load(), s);
}

/// 2026-10-28: a file from before merges took Immich photos has no folder for them — the first asks.
#[test]
fn a_settings_file_from_before_immich_merges_has_no_merge_folder() {
    let s: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(s.immich_merge_folder, None);
}

/// M10.17a: a settings file from before the HDR merge opens its dialog with Lightroom's defaults.
#[test]
fn a_settings_file_from_before_panorama_merge_gets_its_defaults() {
    let s: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(s.panorama, crate::pano_merge::PanoSettings::default());
    let partial: Settings = serde_json::from_str(r#"{"panorama": {"fill_edges": true}}"#).unwrap();
    assert!(partial.panorama.fill_edges);
    assert!(
        partial.panorama.auto_crop,
        "a missing option is its default"
    );
}

#[test]
fn a_settings_file_from_before_hdr_merge_gets_lightrooms_defaults() {
    let s: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(s.hdr, crate::hdr_merge::HdrSettings::default());
    let partial: Settings = serde_json::from_str(r#"{"hdr": {"deghost": "Medium"}}"#).unwrap();
    assert_eq!(partial.hdr.deghost, photo_merge::deghost::Deghost::Medium);
    assert!(partial.hdr.auto_align, "a missing option is its default");
}

/// ⚠⚠ The trap: a field-level `#[serde(default)]` on a `u32` yields **0**, i.e. a cache that
/// evicts everything it writes. The container-level default must supply 5120.
#[test]
fn a_settings_file_from_before_the_thumbnail_budgets_loads_them_at_5120() {
    let f = TempFile::new("thumb_budget_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(loaded.thumb_budget_mb, 5120);
    assert_eq!(loaded.immich_thumb_budget_mb, 5120);
}

/// ⚠⚠ M10.15b: an old file must not get `u32`'s 0 (a warning at every launch) or `bool`'s false
/// (the warning silently off) — the container-level default must supply 10 and true.
#[test]
fn a_settings_file_from_before_the_download_limit_gets_10_gb_and_the_warning() {
    let f = TempFile::new("immich_cache_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(loaded.immich_cache_limit_gb, 10);
    assert!(loaded.immich_cache_warn);
}

/// M10.16b: the same rule for AI denoise's results — 10 GB and the warning on, never 0 and false.
#[test]
fn a_settings_file_from_before_ai_denoise_gets_10_gb_and_the_warning() {
    let f = TempFile::new("denoise_cache_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(loaded.denoise_cache_limit_gb, 10);
    assert!(loaded.denoise_cache_warn);
}

#[test]
fn the_preview_settings_round_trip_and_an_old_file_gets_their_defaults() {
    let f = TempFile::new("preview_settings");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let old = SettingsStore::new(f.0.clone()).load();
    assert_eq!(
        old.preview_demosaic, None,
        "follows the 1:1 view by default"
    );
    assert_eq!(old.preview_budget_mb, 10240);
    assert_eq!(old.preview_tier(), old.view_demosaic);
    let s = Settings {
        preview_demosaic: Some(Demosaic::Fast),
        preview_budget_mb: 2048,
        ..Default::default()
    };
    assert_eq!(s.preview_tier(), Demosaic::Fast);
    let store = SettingsStore::new(f.0.clone());
    store.save(&s).unwrap();
    let loaded = store.load();
    assert_eq!(loaded.preview_demosaic, Some(Demosaic::Fast));
    assert_eq!(loaded.preview_budget_mb, 2048);
}

#[test]
fn a_settings_file_from_before_the_immich_fields_still_loads() {
    // ⚠ Same property as its neighbours: `load()` is infallible, so a field that fails to
    // parse does not surface an error — it silently reverts *every* setting to its default.
    // The container-level `#[serde(default)]` is what fills these two, and an empty string is
    // exactly the right "no server configured".
    let f = TempFile::new("immich_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert!(loaded.immich_url.is_empty());
    assert!(loaded.immich_key.is_empty());
    assert!(
        crate::immich_source::connection(&loaded).is_none(),
        "an upgraded config must read as unconfigured, not as a half-set connection"
    );
}

#[test]
fn a_settings_file_from_before_export_options_still_loads() {
    // ⚠ The forgiving-load property, and the test most likely to be waved through as
    // obviously fine. Without `Default` on `ExportOptions` the `#[serde(default)]` on
    // `Settings` cannot fill the missing field, so every existing config would fail to
    // parse — and `load()` being infallible would turn that into *all* settings
    // silently reverting.
    let f = TempFile::new("export_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(loaded.export, ExportOptions::default());
}

#[test]
fn a_settings_file_from_before_view_demosaic_still_loads() {
    // ⚠ Same property as the test above, for the same reason: `load()` is infallible, so
    // a field that fails to parse does not surface an error — it silently reverts *every*
    // setting to its default. A new field must therefore be fillable by `#[serde(default)]`.
    let f = TempFile::new("view_demosaic_old");
    std::fs::write(
        &f.0,
        r#"{"confirm_preset_delete": false, "confirm_export_preset_delete": false}"#,
    )
    .unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old fields must survive");
    assert!(!loaded.confirm_export_preset_delete);
    assert_eq!(
        loaded.view_demosaic,
        raw_decode::Demosaic::default(),
        "and the new one must be fillable from the one place the default lives"
    );
}

#[test]
fn a_settings_file_from_before_custom_aspect_still_loads() {
    // ⚠ `load()` is infallible, so a field that fails to parse silently reverts *every*
    // setting. The container-level `#[serde(default)]` is what fills the new field.
    let f = TempFile::new("custom_aspect_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old fields must survive");
    assert_eq!(loaded.custom_aspect, DEFAULT_CUSTOM_ASPECT);
}

#[test]
fn the_view_tier_round_trips_through_a_file() {
    let f = TempFile::new("view_demosaic_rt");
    let store = SettingsStore::new(f.0.clone());
    // ⚠ Struct-update syntax, not `let mut s = Settings::default();` followed by a field
    // assignment — clippy's `field_reassign_with_default` is warn-by-default and CI runs
    // with `-D warnings`.
    let s = Settings {
        view_demosaic: raw_decode::Demosaic::Best,
        ..Default::default()
    };
    store.save(&s).unwrap();
    assert_eq!(store.load().view_demosaic, raw_decode::Demosaic::Best);
}

/// ⚠ `load()` is infallible, so an unparseable field does not surface an error — it silently
/// reverts *every* setting. A new field must be fillable by `#[serde(default)]`, which is
/// what the container-level attribute on `Settings` is for.
///
/// ⚠ The assertion is on the **value**, not merely that the load succeeded: a resampler
/// arriving as bilinear would quietly hand every existing user the old, softer kernel and
/// the feature would look like it had never shipped.
#[test]
fn a_settings_file_from_before_the_resampler_still_loads_and_gets_catmull_rom() {
    let f = TempFile::new("resampler_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(loaded.resampler, Resampler::CatmullRom);
}

#[test]
fn the_resampler_round_trips_through_a_file() {
    let f = TempFile::new("resampler_rt");
    let store = SettingsStore::new(f.0.clone());
    let s = Settings {
        resampler: Resampler::Bilinear,
        ..Default::default()
    };
    store.save(&s).unwrap();
    assert_eq!(store.load().resampler, Resampler::Bilinear);
}

/// A unique temp file path for one test, removed on drop.
struct TempFile(PathBuf);

impl TempFile {
    fn new(tag: &str) -> Self {
        Self(test_dir::path(&format!("settings-{tag}")).join("settings.json"))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

#[test]
fn an_absent_file_loads_defaults() {
    let f = TempFile::new("absent");
    assert_eq!(SettingsStore::new(f.0.clone()).load(), Settings::default());
}

#[test]
fn a_malformed_file_loads_defaults_rather_than_failing() {
    // ⚠ A corrupt settings file must never brick the app — the same forgiving posture
    // the JSON sidecars already take.
    let f = TempFile::new("malformed");
    std::fs::write(&f.0, "{ this is not json").unwrap();
    assert_eq!(SettingsStore::new(f.0.clone()).load(), Settings::default());
}

#[test]
fn save_then_load_round_trips() {
    let f = TempFile::new("roundtrip");
    let store = SettingsStore::new(f.0.clone());
    let s = Settings {
        confirm_preset_delete: false,
        ..Settings::default()
    };
    store.save(&s).unwrap();
    assert_eq!(store.load(), s);
}

#[test]
fn unknown_and_missing_fields_are_tolerated() {
    // Forward compatibility: a file written by a newer build must still load.
    let f = TempFile::new("unknown");
    std::fs::write(
        &f.0,
        r#"{"confirm_preset_delete": false, "future_flag": 7}"#,
    )
    .unwrap();
    assert!(!SettingsStore::new(f.0.clone()).load().confirm_preset_delete);

    let g = TempFile::new("emptyobj");
    std::fs::write(&g.0, "{}").unwrap();
    assert_eq!(SettingsStore::new(g.0.clone()).load(), Settings::default());
}

#[test]
fn a_settings_file_from_before_the_appearance_settings_still_loads() {
    // ⚠ The guard every new `Settings` field needs. `load()` is infallible, so a field
    // that fails to parse does not surface an error — it silently reverts *every* setting.
    let f = TempFile::new("appearance_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(loaded.theme, ThemeChoice::Dark, "dark is the default");
    assert_eq!(loaded.accent, Accent::SteelBlue);
}

#[test]
fn the_appearance_settings_round_trip() {
    let f = TempFile::new("appearance_rt");
    let store = SettingsStore::new(f.0.clone());
    let s = Settings {
        theme: ThemeChoice::Light,
        accent: Accent::Violet,
        ..Default::default()
    };
    store.save(&s).unwrap();
    let loaded = store.load();
    assert_eq!(loaded.theme, ThemeChoice::Light);
    assert_eq!(loaded.accent, Accent::Violet);
}

#[test]
fn a_settings_file_from_before_pan_drag_still_loads() {
    // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
    // reverts EVERY setting rather than erroring. That is why this test exists and is not
    // a nicety — the same trap the view-demosaic work documented.
    //
    // ⚠ `confirm_preset_delete` is written as `false` deliberately: it *defaults* to true,
    // so a whole-file parse failure is visible as it flipping back. Asserting a field whose
    // written value equalled its default would prove nothing.
    let f = TempFile::new("pan_drag_compat");
    std::fs::write(
        &f.0,
        r#"{"confirm_preset_delete":false,"confirm_export_preset_delete":false}"#,
    )
    .unwrap();
    let s = SettingsStore::new(f.0.clone()).load();
    assert_eq!(s.pan_drag, PanDrag::Middle, "the new field must default");
    assert!(
        !s.confirm_preset_delete,
        "an old field must survive — if this flipped to true, the whole file failed to parse"
    );
}

#[test]
fn a_settings_file_from_before_the_display_space_still_loads() {
    // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
    // reverts *every* setting. The container-level `#[serde(default)]` is what prevents
    // that — so what this test actually discriminates is the **value**: the default must be
    // sRGB, because a wide space arriving by default would change what every existing user
    // sees without them asking for it.
    let json = r#"{"confirm_preset_delete":true}"#;
    let s: Settings = serde_json::from_str(json).expect("older settings should load");
    assert_eq!(s.display_space, color::OutputSpace::Srgb);
    assert!(s.confirm_preset_delete);
}

#[test]
fn a_settings_file_from_before_camera_profiles_still_loads() {
    let json = r#"{"confirm_preset_delete":true}"#;
    let s: Settings = serde_json::from_str(json).expect("older settings should load");
    assert!(s.camera_profiles.is_empty());
}

#[test]
fn the_camera_profile_mapping_round_trips() {
    let mut s = Settings::default();
    s.camera_profiles
        .insert("X-T4".into(), std::path::PathBuf::from("/p/x-t4.dcp"));
    let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(
        back.camera_profiles.get("X-T4").and_then(|p| p.to_str()),
        Some("/p/x-t4.dcp")
    );
}

#[test]
fn a_settings_file_from_before_auto_lens_correction_still_loads() {
    // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
    // reverts *every* setting. `#[serde(default)]` is what stands in the way.
    let s: Settings = serde_json::from_str(r#"{"confirm_preset_delete":false}"#).unwrap();
    assert!(!s.confirm_preset_delete);
    assert!(
        s.auto_lens_correction,
        "the new field must default to on, not to false"
    );
}

/// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts *every*
/// setting. The container-level `#[serde(default)]` is what stops an older file doing that.
#[test]
fn a_settings_file_from_before_default_rendering_still_loads() {
    let f = TempFile::new("pre_rendering");
    std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
    let s = SettingsStore::new(f.0.clone()).load();
    assert!(!s.confirm_preset_delete, "the stored field was lost");
    assert_eq!(
        s.default_rendering,
        edit_model::Rendering::Camera,
        "the new field must come from Default, not from the enum's own default"
    );
}

#[test]
fn a_settings_file_from_before_the_library_view_options_still_loads() {
    // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
    // *every* setting. The container-level `#[serde(default)]` is what stops that; this pins
    // it for the two fields added here.
    let f = TempFile::new("libview_old");
    std::fs::write(
        &f.0,
        r#"{"confirm_preset_delete":false,"auto_lens_correction":false}"#,
    )
    .unwrap();
    let got = SettingsStore::new(f.0.clone()).load();
    assert!(!got.confirm_preset_delete, "the stored settings survived");
    assert!(!got.auto_lens_correction);
    assert_eq!(got.library_sort, catalog::SortOrder::NameAsc);
    assert_eq!(got.library_cell, DEFAULT_CELL);
}

/// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
/// setting. The sibling of `a_settings_file_from_before_the_panel_sections_still_loads`.
#[test]
fn a_settings_file_from_before_the_snapshot_strip_still_loads() {
    let f = TempFile::new("presnapstrip");
    std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
    let got = SettingsStore::new(f.0.clone()).load();
    assert_eq!(got.library_cell, 200, "the stored field must survive");
    assert_eq!(
        got.snapshot_strip_height, DEFAULT_STRIP_HEIGHT,
        "and an absent one must give the default, not `u32`'s zero"
    );
}

/// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
/// setting. The sibling of `a_settings_file_from_before_the_snapshot_strip_still_loads`.
#[test]
fn a_settings_file_from_before_dialog_sizes_still_loads() {
    let f = TempFile::new("predialogsizes");
    std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
    let got = SettingsStore::new(f.0.clone()).load();
    assert_eq!(got.library_cell, 200, "the stored field must survive");
    assert!(
        got.dialog_sizes.is_empty(),
        "and an absent map must read as empty, not fail the whole parse"
    );
}

/// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
/// setting. The sibling of `a_settings_file_from_before_dialog_sizes_still_loads`.
#[test]
fn a_settings_file_from_before_the_panel_order_still_loads() {
    let f = TempFile::new("prepanelorder");
    std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
    let got = SettingsStore::new(f.0.clone()).load();
    assert_eq!(got.library_cell, 200, "the stored field must survive");
    assert_eq!(
        got.panel_order(),
        PanelSection::ALL.to_vec(),
        "and an absent order must give the default"
    );
}

#[test]
fn a_settings_file_from_before_the_gpu_default_flipped_gets_the_new_default() {
    // ⚠⚠ **The guard on the 2026-09-18 flip, and it is the `auto_advance` trap again.** While
    // `gpu_tail` defaulted to `false` a field-level `#[serde(default)]` was safe, because
    // `bool`'s own default matched. Flipping the intended default to `true` without removing
    // that attribute would have loaded **`false`** for every settings file written before
    // today — the feature shipped disabled for everyone who already has one, which is most
    // people, and silently.
    //
    // ⚠ Asserts the **value**, not the mechanism, so it catches either way round.
    let f = TempFile::new("gpu_tail_absent");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert!(
        loaded.gpu_tail,
        "a settings file with no `gpu_tail` must get the intended default, not `bool`'s"
    );
}

#[test]
fn an_explicit_gpu_tail_false_is_still_honoured() {
    // ⚠ The other half: a user who switched it OFF must stay off. A default that overrode an
    // explicit choice would be worse than the wrong default.
    let f = TempFile::new("gpu_tail_off");
    std::fs::write(&f.0, r#"{"gpu_tail": false}"#).unwrap();
    assert!(!SettingsStore::new(f.0.clone()).load().gpu_tail);
}

#[test]
fn a_settings_file_from_before_auto_advance_still_loads() {
    // ⚠⚠ The forgiving-load property, and the trap `snapshot_strip_height` and `write_xmp`
    // both document: `load()` is infallible, so a field that fails to parse does not surface
    // an error — it silently reverts *every* setting. A **field-level** `#[serde(default)]`
    // would be worse still: it would use `bool`'s own default of `false` and ship the feature
    // disabled for everyone who already has a settings file, which is most people.
    //
    // ⚠ Asserts the **value**, not the mechanism, so it catches either way round.
    let f = TempFile::new("auto_advance_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert!(
        loaded.auto_advance,
        "an existing settings file must get the feature, not silently opt out of it"
    );
}

#[test]
fn the_upload_key_defaults_to_empty_and_round_trips() {
    assert!(Settings::default().immich_upload_key.is_empty());
    let s = Settings {
        immich_upload_key: "upload-key".to_string(),
        ..Settings::default()
    };
    let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.immich_upload_key, "upload-key");
}

/// ⚠ A settings file from before this field must still load — the browse key and URL survive.
#[test]
fn a_settings_file_from_before_the_upload_key_still_loads() {
    let json = r#"{"immich_url":"https://h","immich_key":"browse"}"#;
    let s: Settings = serde_json::from_str(json).unwrap();
    assert_eq!(s.immich_url, "https://h");
    assert_eq!(s.immich_key, "browse");
    assert!(s.immich_upload_key.is_empty());
}

/// ⚠⚠ **The default must be ON, or Immich vanishes on upgrade for anyone already using it** —
/// the container-level `#[serde(default)]` means a settings file written before this field takes
/// whatever `Default` says. This is the `gpu_tail` trap, pinned.
#[test]
fn the_immich_gate_defaults_to_on() {
    assert!(Settings::default().immich_enabled);
    // The upgrade case, stated as data: a file with a working Immich setup and no gate field.
    let s: Settings =
        serde_json::from_str(r#"{"immich_url":"https://h","immich_key":"k"}"#).unwrap();
    assert!(
        s.immich_enabled,
        "an existing user's Immich must survive the upgrade"
    );
}

#[test]
fn the_immich_gate_round_trips() {
    let s = Settings {
        immich_enabled: false,
        ..Settings::default()
    };
    let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert!(!back.immich_enabled);
}

/// ⚠⚠ **Stacking is ON by default, and that is the `gpu_tail` trap in a new place.** The
/// container-level `#[serde(default)]` means a settings file written before this field takes
/// whatever `Default` says — and `true` halves the grid on first launch, which is the behaviour
/// asked for. Pinned so a later tidy-up cannot flip it in either direction.
#[test]
fn stacking_is_on_by_default() {
    assert!(Settings::default().stacking.enabled);
    assert_eq!(
        Settings::default().stacking.primary,
        crate::stack::StackPrimary::EditedElseRaw
    );
    // ⚠ Every format takes part by default, which is what makes the rule general.
    assert!(!Settings::default().stacking.formats.is_active());

    let loaded: Settings = serde_json::from_str(r#"{"immich_url":"https://h"}"#).unwrap();
    assert!(
        loaded.stacking.enabled,
        "a settings file from before the field must load with stacking on"
    );
}

/// M10.16b6: a settings file from before the network choice runs FFDNet — the default for everyone.
#[test]
fn a_settings_file_from_before_b6_runs_ffdnet() {
    let f = TempFile::new("denoise_network_old");
    std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
    let loaded = SettingsStore::new(f.0.clone()).load();
    assert!(!loaded.confirm_preset_delete, "the old field must survive");
    assert_eq!(loaded.denoise_network, DenoiseNetwork::Ffdnet);
}
