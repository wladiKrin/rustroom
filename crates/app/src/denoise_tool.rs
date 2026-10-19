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

/// The Detail block's line, or `None` with nothing to say. `progress` is tiles done of total.
pub fn status(f: &Facts, progress: Option<(usize, usize)>) -> Option<String> {
    if !f.active || f.has_result {
        return None;
    }
    if f.running {
        return Some(match progress {
            Some((done, total)) => format!("Denoising… {} %", done * 100 / total.max(1)),
            None => "Denoising…".to_string(),
        });
    }
    if f.failed {
        return Some("Denoising failed — the status bar says why".to_string());
    }
    match next(f) {
        Next::NeedModel => Some("The AI denoise model is not downloaded".to_string()),
        Next::WaitForDecode => Some("Waiting for the full-resolution decode…".to_string()),
        _ => None,
    }
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
            status(&half, None).as_deref(),
            Some("Waiting for the full-resolution decode…")
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
        assert!(status(&no_model, None).is_some_and(|s| s.contains("model")));
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
    fn the_line_says_what_is_happening() {
        let running = Facts {
            running: true,
            ..wanted()
        };
        assert_eq!(
            status(&running, Some((37, 100))).as_deref(),
            Some("Denoising… 37 %")
        );
        assert_eq!(status(&running, None).as_deref(), Some("Denoising…"));
        let failed = Facts {
            failed: true,
            ..wanted()
        };
        assert!(status(&failed, None).is_some_and(|s| s.starts_with("Denoising failed")));
        let done = Facts {
            has_result: true,
            ..wanted()
        };
        assert_eq!(status(&done, None), None);
        let off = Facts {
            active: false,
            ..wanted()
        };
        assert_eq!(status(&off, None), None);
    }
}
