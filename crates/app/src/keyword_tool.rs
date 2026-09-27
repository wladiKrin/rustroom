//! The Keywords panel's rules (M9.8): what a keyword may be called, and where one may be dropped.
//!
//! ⚠ No egui — the `defringe_tool` / `mask_tool` pattern: the panel supplies the gesture, the rules
//! are tested here without a window.

use catalog::db::KeywordRow;

/// Why a name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadName {
    Empty,
    /// `|` separates the levels of a keyword path in XMP's `lr:hierarchicalSubject`, so a name
    /// containing it would come back from the sidecar as two keywords.
    Separator,
}

impl BadName {
    pub fn message(self) -> &'static str {
        match self {
            BadName::Empty => "A keyword needs a name",
            BadName::Separator => "A keyword cannot contain “|” — XMP uses it between levels",
        }
    }
}

/// `typed`, trimmed, if it can be a keyword's name.
pub fn valid_name(typed: &str) -> Result<String, BadName> {
    let name = typed.trim();
    if name.is_empty() {
        Err(BadName::Empty)
    } else if name.contains('|') {
        Err(BadName::Separator)
    } else {
        Ok(name.to_string())
    }
}

/// Whether `dragged` may be dropped under `target` (`None`: the top level).
///
/// ⚠ Refused: onto itself, onto anything beneath it (a loop — the catalog refuses it too, this
/// says so before the drop), and onto its current parent, which would change nothing.
pub fn can_drop(tree: &[KeywordRow], dragged: i64, target: Option<i64>) -> bool {
    let parent_of = |id: i64| tree.iter().find(|k| k.id == id).and_then(|k| k.parent_id);
    if parent_of(dragged) == target && tree.iter().any(|k| k.id == dragged) {
        return false;
    }
    let Some(mut t) = target else {
        return true;
    };
    loop {
        if t == dragged {
            return false;
        }
        match parent_of(t) {
            Some(p) => t = p,
            None => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, parent: Option<i64>) -> KeywordRow {
        KeywordRow {
            id,
            name: format!("k{id}"),
            parent_id: parent,
            depth: 0,
            count: 0,
        }
    }

    /// Europe(1) > France(2) > Paris(3); Asia(4).
    fn tree() -> Vec<KeywordRow> {
        vec![row(1, None), row(2, Some(1)), row(3, Some(2)), row(4, None)]
    }

    #[test]
    fn names_are_trimmed_and_the_separator_and_empty_are_refused() {
        assert_eq!(valid_name("  Paris "), Ok("Paris".to_string()));
        assert_eq!(valid_name("   "), Err(BadName::Empty));
        assert_eq!(valid_name("Europe|France"), Err(BadName::Separator));
    }

    #[test]
    fn a_keyword_cannot_go_under_itself_or_its_descendants_or_where_it_is() {
        let t = tree();
        assert!(!can_drop(&t, 1, Some(1)), "onto itself");
        assert!(!can_drop(&t, 1, Some(3)), "under its grandchild");
        assert!(
            !can_drop(&t, 3, Some(2)),
            "onto its own parent changes nothing"
        );
        assert!(
            !can_drop(&t, 1, None),
            "a root to the top level changes nothing"
        );
        assert!(can_drop(&t, 3, Some(4)), "across to another branch");
        assert!(can_drop(&t, 3, None), "up to the top level");
        assert!(can_drop(&t, 4, Some(3)), "a root under a leaf elsewhere");
    }
}
