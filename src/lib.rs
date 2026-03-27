//! `qplan`: query planning / compilation for retrieval.
//!
//! `qexpr` describes query meaning; `qplan` compiles that meaning into a small plan that
//! execution backends can implement efficiently.
//!
//! This crate is intentionally narrow today: it only supports a conjunctive subset of `QExpr`
//! (terms, phrases, NEAR constraints, and AND trees) and returns explicit errors for the rest.

#![warn(missing_docs)]

use qexpr::{Near, Phrase, QExpr, Term};
use std::collections::HashSet;

/// Errors returned by `qplan`.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// OR is not supported by the current compilation target.
    #[error("unsupported operator: Or")]
    UnsupportedOr,
    /// NOT is not supported by the current compilation target.
    #[error("unsupported operator: Not")]
    UnsupportedNot,
    /// Field scoping is not supported without field-aware indexing.
    #[error("unsupported operator: Field")]
    UnsupportedField,
    /// A phrase or near constraint reduced to fewer than 2 terms after blank filtering.
    #[error("constraint reduced to fewer than 2 terms after blank filtering")]
    DegenerateConstraint,
}

/// A compiled conjunctive query plan.
///
/// Interpretation:
/// - `bag_terms` is a **superset** used for candidate generation.
/// - `phrases` and `nears` are **verifier constraints** (positional evaluation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConjunctivePlan {
    /// Bag-of-terms used for candidate generation.
    pub bag_terms: Vec<String>,
    /// Phrase constraints (ordered adjacent).
    pub phrases: Vec<Vec<String>>,
    /// Proximity constraints.
    pub nears: Vec<NearPlan>,
}

impl ConjunctivePlan {
    /// Return true if there are no positional constraints.
    pub fn is_bag_only(&self) -> bool {
        self.phrases.is_empty() && self.nears.is_empty()
    }
}

/// A compiled proximity constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearPlan {
    /// Terms participating in the proximity constraint.
    pub terms: Vec<String>,
    /// Window size in tokens.
    pub window: u32,
    /// If true, enforce term order.
    pub ordered: bool,
}

/// Compile a query into a conjunctive plan.
///
/// Supported subset:
/// - `Term`, `Phrase`, `Near`
/// - `And` trees over supported nodes
///
/// Unsupported today (explicit errors): `Or`, `Not`, `Field`.
pub fn compile_conjunctive(expr: &QExpr) -> Result<ConjunctivePlan, Error> {
    let mut bag: Vec<String> = Vec::new();
    let mut phrases: Vec<Vec<String>> = Vec::new();
    let mut nears: Vec<NearPlan> = Vec::new();

    fn push_term(bag: &mut Vec<String>, t: &Term) {
        if !t.is_blank() {
            bag.push(t.0.clone());
        }
    }

    fn push_phrase(
        bag: &mut Vec<String>,
        phrases: &mut Vec<Vec<String>>,
        p: &Phrase,
    ) -> Result<(), Error> {
        let mut ts: Vec<String> = Vec::new();
        for t in &p.terms {
            if !t.is_blank() {
                ts.push(t.0.clone());
                bag.push(t.0.clone());
            }
        }
        if ts.len() >= 2 {
            phrases.push(ts);
        } else {
            return Err(Error::DegenerateConstraint);
        }
        Ok(())
    }

    fn push_near(bag: &mut Vec<String>, nears: &mut Vec<NearPlan>, n: &Near) -> Result<(), Error> {
        let mut ts: Vec<String> = Vec::new();
        for t in &n.terms {
            if !t.is_blank() {
                ts.push(t.0.clone());
                bag.push(t.0.clone());
            }
        }
        if ts.len() >= 2 && n.window > 0 {
            nears.push(NearPlan {
                terms: ts,
                window: n.window,
                ordered: n.ordered,
            });
        } else {
            return Err(Error::DegenerateConstraint);
        }
        Ok(())
    }

    fn walk(
        e: &QExpr,
        bag: &mut Vec<String>,
        phrases: &mut Vec<Vec<String>>,
        nears: &mut Vec<NearPlan>,
    ) -> Result<(), Error> {
        match e {
            QExpr::Term(t) => {
                push_term(bag, t);
                Ok(())
            }
            QExpr::Phrase(p) => push_phrase(bag, phrases, p),
            QExpr::Near(n) => push_near(bag, nears, n),
            QExpr::And(xs) => {
                for x in xs {
                    walk(x, bag, phrases, nears)?;
                }
                Ok(())
            }
            QExpr::Or(_) => Err(Error::UnsupportedOr),
            QExpr::Not(_) => Err(Error::UnsupportedNot),
            QExpr::Field(_, _) => Err(Error::UnsupportedField),
        }
    }

    walk(expr, &mut bag, &mut phrases, &mut nears)?;

    // Deterministic + dedup bag terms (but keep stable order).
    let mut seen: HashSet<String> = HashSet::new();
    let mut deduped: Vec<String> = Vec::with_capacity(bag.len());
    for t in bag {
        if seen.insert(t.clone()) {
            deduped.push(t);
        }
    }

    Ok(ConjunctivePlan {
        bag_terms: deduped,
        phrases,
        nears,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use qexpr::{FieldName, QExpr, Term};

    #[test]
    fn compiles_terms_and_phrases_into_bag_plus_constraints() {
        let q = QExpr::And(vec![
            QExpr::Term(Term::new("alpha")),
            QExpr::Phrase(Phrase::new(vec![Term::new("new"), Term::new("york")])),
        ]);
        let p = compile_conjunctive(&q).unwrap();
        assert_eq!(p.phrases.len(), 1);
        assert!(p.bag_terms.contains(&"alpha".to_string()));
        assert!(p.bag_terms.contains(&"new".to_string()));
        assert!(p.bag_terms.contains(&"york".to_string()));
    }

    #[test]
    fn phrase_with_blanks_keeping_two_terms() {
        // 1 blank + 2 non-blank -> blank filtered, phrase kept with 2 terms.
        let q = QExpr::Phrase(Phrase::new(vec![
            Term::new("  "),
            Term::new("new"),
            Term::new("york"),
        ]));
        let p = compile_conjunctive(&q).unwrap();
        assert_eq!(p.phrases, vec![vec!["new".to_string(), "york".to_string()]]);
        assert!(p.bag_terms.contains(&"new".to_string()));
        assert!(p.bag_terms.contains(&"york".to_string()));
    }

    #[test]
    fn phrase_reduced_to_one_term_errors() {
        // 1 blank + 1 non-blank -> only 1 term after filtering -> DegenerateConstraint.
        let q = QExpr::Phrase(Phrase::new(vec![Term::new("  "), Term::new("solo")]));
        assert_eq!(
            compile_conjunctive(&q).unwrap_err(),
            Error::DegenerateConstraint
        );
    }

    #[test]
    fn phrase_all_blank_errors() {
        let q = QExpr::Phrase(Phrase::new(vec![Term::new("  "), Term::new("")]));
        assert_eq!(
            compile_conjunctive(&q).unwrap_err(),
            Error::DegenerateConstraint
        );
    }

    #[test]
    fn near_reduced_to_one_term_errors() {
        let q = QExpr::Near(Near::new(
            vec![Term::new("  "), Term::new("only")],
            5,
            false,
        ));
        assert_eq!(
            compile_conjunctive(&q).unwrap_err(),
            Error::DegenerateConstraint
        );
    }

    #[test]
    fn near_with_blanks_keeping_two_terms() {
        let q = QExpr::Near(Near::new(
            vec![Term::new("  "), Term::new("deep"), Term::new("learning")],
            5,
            true,
        ));
        let p = compile_conjunctive(&q).unwrap();
        assert_eq!(p.nears.len(), 1);
        assert_eq!(
            p.nears[0].terms,
            vec!["deep".to_string(), "learning".to_string()]
        );
        assert!(p.nears[0].ordered);
    }

    // -- Generators --

    fn arb_non_blank_term() -> impl Strategy<Value = Term> {
        "[a-z]{1,8}".prop_map(Term::new)
    }

    /// An And-only tree (terms, phrases, nears, and nested Ands).
    fn arb_and_only(depth: u32) -> impl Strategy<Value = QExpr> {
        let leaf = prop_oneof![
            arb_non_blank_term().prop_map(QExpr::Term),
            prop::collection::vec(arb_non_blank_term(), 2..5)
                .prop_map(|ts| QExpr::Phrase(Phrase::new(ts))),
            (
                prop::collection::vec(arb_non_blank_term(), 2..5),
                1..10u32,
                any::<bool>(),
            )
                .prop_map(|(ts, w, o)| QExpr::Near(Near::new(ts, w, o))),
        ];
        leaf.prop_recursive(depth, 32, 4, |inner| {
            prop::collection::vec(inner, 1..4).prop_map(QExpr::And)
        })
    }

    /// Collect all non-blank term strings from a QExpr tree.
    fn collect_terms(expr: &QExpr) -> Vec<String> {
        let mut out = Vec::new();
        match expr {
            QExpr::Term(t) => {
                if !t.is_blank() {
                    out.push(t.0.clone());
                }
            }
            QExpr::Phrase(p) => {
                for t in &p.terms {
                    if !t.is_blank() {
                        out.push(t.0.clone());
                    }
                }
            }
            QExpr::Near(n) => {
                for t in &n.terms {
                    if !t.is_blank() {
                        out.push(t.0.clone());
                    }
                }
            }
            QExpr::And(xs) | QExpr::Or(xs) => {
                for x in xs {
                    out.extend(collect_terms(x));
                }
            }
            QExpr::Not(x) => out.extend(collect_terms(x)),
            QExpr::Field(_, x) => out.extend(collect_terms(x)),
        }
        out
    }

    // -- Property tests --

    proptest! {
        #[test]
        fn and_only_tree_always_compiles(expr in arb_and_only(3)) {
            prop_assert!(compile_conjunctive(&expr).is_ok());
        }

        #[test]
        fn bag_terms_contains_all_unique_terms(expr in arb_and_only(3)) {
            let plan = compile_conjunctive(&expr).unwrap();
            let expected: std::collections::HashSet<String> = collect_terms(&expr).into_iter().collect();
            let actual: std::collections::HashSet<String> = plan.bag_terms.iter().cloned().collect();
            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn phrases_preserve_term_order(
            terms in prop::collection::vec(arb_non_blank_term(), 2..6),
        ) {
            let expr = QExpr::Phrase(Phrase::new(terms.clone()));
            let plan = compile_conjunctive(&expr).unwrap();
            let expected: Vec<String> = terms.iter().map(|t| t.0.clone()).collect();
            prop_assert_eq!(&plan.phrases[0], &expected);
        }

        #[test]
        fn or_always_rejected(
            children in prop::collection::vec(arb_non_blank_term().prop_map(QExpr::Term), 1..4),
        ) {
            let expr = QExpr::Or(children);
            prop_assert_eq!(compile_conjunctive(&expr).unwrap_err(), Error::UnsupportedOr);
        }

        #[test]
        fn not_always_rejected(inner in arb_non_blank_term().prop_map(QExpr::Term)) {
            let expr = QExpr::Not(Box::new(inner));
            prop_assert_eq!(compile_conjunctive(&expr).unwrap_err(), Error::UnsupportedNot);
        }

        #[test]
        fn field_always_rejected(
            name in "[a-z]{1,6}".prop_map(FieldName::new),
            inner in arb_non_blank_term().prop_map(QExpr::Term),
        ) {
            let expr = QExpr::Field(name, Box::new(inner));
            prop_assert_eq!(compile_conjunctive(&expr).unwrap_err(), Error::UnsupportedField);
        }

        #[test]
        fn compilation_is_deterministic(expr in arb_and_only(3)) {
            let a = compile_conjunctive(&expr).unwrap();
            let b = compile_conjunctive(&expr).unwrap();
            prop_assert_eq!(a, b);
        }
    }

    #[test]
    fn rejects_or_not_field() {
        let q = QExpr::Or(vec![
            QExpr::Term(Term::new("a")),
            QExpr::Term(Term::new("b")),
        ]);
        assert_eq!(compile_conjunctive(&q).unwrap_err(), Error::UnsupportedOr);

        let q = QExpr::Not(Box::new(QExpr::Term(Term::new("a"))));
        assert_eq!(compile_conjunctive(&q).unwrap_err(), Error::UnsupportedNot);

        let q = QExpr::Field(
            FieldName::new("title"),
            Box::new(QExpr::Term(Term::new("a"))),
        );
        assert_eq!(
            compile_conjunctive(&q).unwrap_err(),
            Error::UnsupportedField
        );
    }
}
