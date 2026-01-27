//! `qplan`: query planning/compilation for retrieval.
//!
//! Prime-ideal goal: isolate the “planner seam” between query meaning (`qexpr`)
//! and execution backends (postings/positional/grams/etc.).
//!
//! This crate is intentionally **honest**: it only supports a small subset of
//! `QExpr` today (conjunctive queries), and returns explicit errors for the rest.

#![warn(missing_docs)]

use qexpr::{Near, Phrase, QExpr, Term};
use std::collections::HashSet;

/// Errors returned by `qplan`.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
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

    fn push_phrase(bag: &mut Vec<String>, phrases: &mut Vec<Vec<String>>, p: &Phrase) {
        let mut ts: Vec<String> = Vec::new();
        for t in &p.terms {
            if !t.is_blank() {
                ts.push(t.0.clone());
                bag.push(t.0.clone());
            }
        }
        if ts.len() >= 2 {
            phrases.push(ts);
        }
    }

    fn push_near(bag: &mut Vec<String>, nears: &mut Vec<NearPlan>, n: &Near) {
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
        }
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
            QExpr::Phrase(p) => {
                push_phrase(bag, phrases, p);
                Ok(())
            }
            QExpr::Near(n) => {
                push_near(bag, nears, n);
                Ok(())
            }
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
