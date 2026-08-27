//! The flat form a rule is edited in, and its conversion to and from [`Expr`].
//!
//! ⚠⚠ **A row is a *kind* plus one-or-more values**, not a single criterion — because
//! `rule_from_current` maps a star set to `Any([Is, Is, …])`, so **every rule saved from the
//! current view is nested**. The real one saved during plan 1's testing is
//! `{"All":[{"Any":[{"Is":{"Rating":…}}]}]}`, and a one-criterion-per-row editor would refuse to
//! open its own output.
//!
//! ⚠ The form expresses a strict **subset** of `Expr`. A rule with nesting it cannot represent is
//! **refused**, never flattened lossily: silently rewriting someone's rule is worse than declining
//! to edit it, and the loss would be invisible until the collection stopped matching.

use crate::rule::{Criterion, Expr};

/// How the rows combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Match {
    #[default]
    All,
    Any,
    /// `Not(Any([…]))` — "none of the following".
    None,
}

impl Match {
    pub const ALL: [Match; 3] = [Match::All, Match::Any, Match::None];

    pub fn label(self) -> &'static str {
        match self {
            Match::All => "all",
            Match::Any => "any",
            Match::None => "none",
        }
    }
}

/// One row: a criterion kind, with one or more values of that kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub values: Vec<Criterion>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Form {
    pub match_: Match,
    pub rows: Vec<Row>,
}

impl Form {
    /// Build the expression this form describes.
    pub fn to_expr(&self) -> Expr {
        let parts: Vec<Expr> = self
            .rows
            .iter()
            .filter(|r| !r.values.is_empty())
            .map(|r| {
                // ⚠ A single value is a bare `Is`, not a one-element `Any`: it keeps the stored
                // JSON readable, and keeps a hand-written `{"Is":…}` editable.
                if r.values.len() == 1 {
                    Expr::Is(r.values[0].clone())
                } else {
                    Expr::Any(r.values.iter().cloned().map(Expr::Is).collect())
                }
            })
            .collect();
        match self.match_ {
            Match::All => Expr::All(parts),
            Match::Any => Expr::Any(parts),
            Match::None => Expr::Not(Box::new(Expr::Any(parts))),
        }
    }

    /// Read an expression into the form, or `None` if it uses nesting the form cannot express.
    ///
    /// ⚠⚠ **`None` means "decline to edit", never "start from empty".** An empty form that
    /// silently replaced a working rule on Save is exactly the data loss this refusal exists to
    /// prevent.
    pub fn from_expr(e: &Expr) -> Option<Self> {
        let (match_, parts) = match e {
            Expr::All(v) => (Match::All, v),
            Expr::Any(v) => (Match::Any, v),
            Expr::Not(inner) => match inner.as_ref() {
                Expr::Any(v) => (Match::None, v),
                _ => return None,
            },
            Expr::Is(_) => return None,
        };
        let mut rows = Vec::new();
        for part in parts {
            rows.push(Row {
                values: row_values(part)?,
            });
        }
        Some(Form { match_, rows })
    }
}

/// The values of one row, or `None` if this child is not a row shape.
///
/// ⚠ A multi-value row must hold **one kind**: `Any([Rating, Text])` is nesting the form cannot
/// show, and splitting it silently into two rows would change what the rule means.
fn row_values(e: &Expr) -> Option<Vec<Criterion>> {
    match e {
        Expr::Is(c) => Some(vec![c.clone()]),
        Expr::Any(v) => {
            let mut out = Vec::new();
            for child in v {
                match child {
                    Expr::Is(c) => out.push(c.clone()),
                    _ => return None,
                }
            }
            let first = out.first()?.kind();
            if out.iter().any(|c| c.kind() != first) {
                return None;
            }
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::Cmp;

    fn rating(v: u8) -> Criterion {
        Criterion::Rating {
            op: Cmp::Exactly,
            value: v,
        }
    }

    #[test]
    fn a_rule_saved_from_the_current_view_can_be_edited() {
        // ⚠⚠ The finding this model exists for. `rule_from_current` maps a star **set** to
        // `Any([…])`, so its output is nested — this is the real rule saved during plan 1's
        // testing, and a one-criterion-per-row editor would refuse to open it.
        let saved = Expr::All(vec![Expr::Any(vec![Expr::Is(rating(3))])]);
        let form = Form::from_expr(&saved).expect("plan 1's own output must be editable");
        assert_eq!(form.match_, Match::All);
        assert_eq!(form.rows.len(), 1);
        assert_eq!(form.rows[0].values, vec![rating(3)]);
    }

    #[test]
    fn a_form_round_trips_through_an_expression() {
        let form = Form {
            match_: Match::Any,
            rows: vec![
                Row {
                    values: vec![rating(4), rating(5)],
                },
                Row {
                    values: vec![Criterion::Text("paris".into())],
                },
            ],
        };
        let back = Form::from_expr(&form.to_expr()).expect("must round trip");
        assert_eq!(back, form);
    }

    #[test]
    fn none_becomes_a_negated_any_and_back() {
        let form = Form {
            match_: Match::None,
            rows: vec![Row {
                values: vec![rating(1)],
            }],
        };
        let e = form.to_expr();
        assert!(matches!(e, Expr::Not(_)), "None must negate: {e:?}");
        assert_eq!(Form::from_expr(&e).unwrap(), form);
    }

    #[test]
    fn a_single_value_row_is_a_bare_is_rather_than_a_one_element_any() {
        // ⚠ Keeps the stored JSON readable, and keeps a hand-written `{"Is":…}` editable.
        let form = Form {
            match_: Match::All,
            rows: vec![Row {
                values: vec![rating(3)],
            }],
        };
        assert_eq!(form.to_expr(), Expr::All(vec![Expr::Is(rating(3))]));
    }

    #[test]
    fn nesting_the_form_cannot_express_is_refused_rather_than_flattened() {
        // ⚠⚠ Silently rewriting someone's rule is worse than declining to edit it: the loss would
        // be invisible until the collection stopped matching what it used to.
        let deep = Expr::All(vec![Expr::Any(vec![Expr::All(vec![Expr::Is(rating(3))])])]);
        assert!(Form::from_expr(&deep).is_none());

        let negated_inside = Expr::All(vec![Expr::Not(Box::new(Expr::Is(rating(3))))]);
        assert!(Form::from_expr(&negated_inside).is_none());

        // A bare criterion at the top level is not a form either — there is no combinator to show.
        assert!(Form::from_expr(&Expr::Is(rating(3))).is_none());
    }

    #[test]
    fn an_empty_form_is_all_of_nothing() {
        // ⚠ Which matches **everything**, per `Expr`'s identity elements — the state a new rule
        // starts in, and it must not be an error.
        assert_eq!(Form::default().to_expr(), Expr::All(vec![]));
        assert_eq!(
            Form::from_expr(&Expr::All(vec![])).unwrap(),
            Form::default()
        );
    }

    #[test]
    fn a_row_of_mixed_kinds_is_refused() {
        // ⚠ A row is "one kind, several values", so `Any([Rating, Text])` is not a row — it is
        // nesting the form cannot show, and splitting it silently into two rows would change what
        // the rule means (an OR would become an AND).
        let mixed = Expr::All(vec![Expr::Any(vec![
            Expr::Is(rating(3)),
            Expr::Is(Criterion::Text("x".into())),
        ])]);
        assert!(Form::from_expr(&mixed).is_none());
    }

    #[test]
    fn an_empty_row_is_dropped_rather_than_emitted() {
        // ⚠ A row whose values were all removed must not become `Any([])`, which matches
        // **nothing** and would silently empty the collection.
        let form = Form {
            match_: Match::All,
            rows: vec![
                Row { values: vec![] },
                Row {
                    values: vec![rating(3)],
                },
            ],
        };
        assert_eq!(form.to_expr(), Expr::All(vec![Expr::Is(rating(3))]));
    }
}
