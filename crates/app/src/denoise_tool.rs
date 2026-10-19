//! AI denoise's decisions (M10.16b): what the flow does next for the open photo, and what the Detail
//! block says — plain facts in, no egui (the `lens_blur_tool` pattern).

/// What is known about the open photo this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    /// Amount above 0.
    pub active: bool,
    /// Its result is held.
    pub has_result: bool,
    /// The cache holds its result — loaded without the model or the full decode.
    pub cached: bool,
    /// The session's buffer is the full-resolution decode (never the half-size one, never a ghost).
    pub full_decode: bool,
    pub model_present: bool,
    /// A job for this photo is running.
    pub running: bool,
    /// The last job for this photo failed.
    pub failed: bool,
}

/// What to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// Load it from the cache.
    Load,
    /// Start the job.
    Run,
    /// Offer the model's download.
    NeedModel,
    /// Wait for the full decode.
    WaitForDecode,
}

/// ⚠ A cached result needs neither the model nor the full decode — it is loaded at once, and shown once
/// the full decode is the session's. The model is offered before the decode lands: it is a download.
pub fn next(f: &Facts) -> Next {
    if !f.active || f.has_result || f.running || f.failed {
        Next::Nothing
    } else if f.cached {
        Next::Load
    } else if !f.model_present {
        Next::NeedModel
    } else if !f.full_decode {
        Next::WaitForDecode
    } else {
        Next::Run
    }
}

/// What the Detail block shows under Amount, or `None` with nothing to say. `progress` is tiles done of
/// total.
pub fn line(f: &Facts, progress: Option<(usize, usize)>) -> Option<Line> {
    if !f.active || f.has_result {
        return None;
    }
    if f.running {
        // ⚠ M10.16b6: a quiet line, never a percentage — and nothing before the job counts tiles.
        let (done, total) = progress?;
        return Some(Line::Progress(done as f32 / total.max(1) as f32));
    }
    if f.failed {
        return Some(Line::Text(
            "Denoising failed — the status bar says why".to_string(),
        ));
    }
    match next(f) {
        Next::NeedModel => Some(Line::Text(
            "The AI denoise model is not downloaded".to_string(),
        )),
        Next::WaitForDecode => Some(Line::Text(
            "Waiting for the full-resolution decode…".to_string(),
        )),
        _ => None,
    }
}

/// What the Detail block shows under Amount.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// The whole-photo job's tiles done, 0…1 — the "Full denoise" line (M10.16b6).
    Progress(f32),
    /// Waiting, failed or missing its model.
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted() -> Facts {
        Facts {
            active: true,
            full_decode: true,
            model_present: true,
            ..Facts::default()
        }
    }

    #[test]
    fn nothing_while_off_or_done() {
        let off = Facts {
            active: false,
            ..wanted()
        };
        assert_eq!(next(&off), Next::Nothing);
        let done = Facts {
            has_result: true,
            ..wanted()
        };
        assert_eq!(next(&done), Next::Nothing);
        assert_eq!(next(&wanted()), Next::Run, "positive control");
    }

    /// A cached result needs neither the model nor the full decode to be loaded.
    #[test]
    fn the_cache_is_read_before_anything_else() {
        let cached = Facts {
            cached: true,
            model_present: false,
            full_decode: false,
            ..wanted()
        };
        assert_eq!(next(&cached), Next::Load);
    }

    /// Review Focus 1: never a half-size or ghost buffer denoised as if it were the full photo.
    #[test]
    fn no_job_before_the_full_decode() {
        let half = Facts {
            full_decode: false,
            ..wanted()
        };
        assert_eq!(next(&half), Next::WaitForDecode);
        assert_eq!(
            line(&half, None),
            Some(Line::Text(
                "Waiting for the full-resolution decode…".to_string()
            ))
        );
    }

    #[test]
    fn a_missing_model_is_offered_and_a_running_or_failed_job_is_not_restarted() {
        let no_model = Facts {
            model_present: false,
            ..wanted()
        };
        assert_eq!(next(&no_model), Next::NeedModel);
        let no_model_half = Facts {
            full_decode: false,
            ..no_model
        };
        assert_eq!(
            next(&no_model_half),
            Next::NeedModel,
            "offered before the decode lands"
        );
        assert!(matches!(line(&no_model, None), Some(Line::Text(t)) if t.contains("model")));
        let running = Facts {
            running: true,
            ..wanted()
        };
        assert_eq!(next(&running), Next::Nothing);
        let failed = Facts {
            failed: true,
            ..wanted()
        };
        assert_eq!(
            next(&failed),
            Next::Nothing,
            "a failure is not retried every frame"
        );
    }

    #[test]
    /// M10.16b6: a running job is a fraction for the quiet line — no percentage anywhere — and nothing until
    /// it counts tiles; every other state its text.
    fn a_running_job_is_a_fraction_without_a_percentage() {
        let running = Facts {
            running: true,
            ..wanted()
        };
        assert_eq!(line(&running, Some((3, 12))), Some(Line::Progress(0.25)));
        assert_eq!(line(&running, None), None);
        let failed = Facts {
            failed: true,
            ..wanted()
        };
        assert!(
            matches!(line(&failed, None), Some(Line::Text(t)) if t.starts_with("Denoising failed"))
        );
        let done = Facts {
            has_result: true,
            ..wanted()
        };
        assert_eq!(line(&done, None), None);
        let off = Facts {
            active: false,
            ..wanted()
        };
        assert_eq!(line(&off, None), None);
        for f in [
            failed,
            Facts {
                full_decode: false,
                ..wanted()
            },
            Facts {
                model_present: false,
                ..wanted()
            },
        ] {
            if let Some(Line::Text(t)) = line(&f, Some((1, 2))) {
                assert!(!t.contains('%'), "{t}");
            }
        }
    }
}
