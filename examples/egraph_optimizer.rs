//! E-graph query optimization via equality saturation (egg).
//!
//! Demonstrates how a `QExpr`-style boolean IR can be optimized using e-graphs.
//! We define a small query algebra as an `egg::Language`, write algebraic rewrite
//! rules (idempotency, double negation, distribution, identity), then extract the
//! smallest equivalent expression.
//!
//! This is a standalone sketch -- it does not depend on `qexpr` types at runtime,
//! but mirrors the same operator vocabulary (And, Or, Not, Term).
//!
//! Reference: Willsey et al., "egg: Fast and Extensible Equality Saturation", POPL 2021.

use egg::{define_language, rewrite as rw, *};

// ---------------------------------------------------------------------------
// 1. Define the query IR as an egg Language
// ---------------------------------------------------------------------------

define_language! {
    enum QueryIR {
        "and" = And([Id; 2]),  // conjunction
        "or"  = Or([Id; 2]),   // disjunction
        "not" = Not([Id; 1]),  // negation
        Symbol(Symbol),        // leaf term
    }
}

// ---------------------------------------------------------------------------
// 2. Rewrite rules (algebraic identities on boolean queries)
// ---------------------------------------------------------------------------

fn rules() -> Vec<Rewrite<QueryIR, ()>> {
    let mut rules = vec![
        // Idempotency
        rw!("and-idem"; "(and ?x ?x)" => "?x"),
        rw!("or-idem";  "(or ?x ?x)"  => "?x"),
        // Double negation elimination
        rw!("not-not"; "(not (not ?x))" => "?x"),
        // Identity elements  ("true" absorbs And, "false" absorbs Or)
        rw!("and-true"; "(and ?x true)" => "?x"),
        rw!("or-false"; "(or ?x false)" => "?x"),
        // Annihilation
        rw!("and-false"; "(and ?x false)" => "false"),
        rw!("or-true";   "(or ?x true)"  => "true"),
        // Commutativity
        rw!("and-comm"; "(and ?x ?y)" => "(and ?y ?x)"),
        rw!("or-comm";  "(or ?x ?y)"  => "(or ?y ?x)"),
        // Absorption: and(x, or(x, y)) -> x
        rw!("and-absorb"; "(and ?x (or ?x ?y))" => "?x"),
        rw!("or-absorb";  "(or ?x (and ?x ?y))" => "?x"),
    ];

    // Associativity (bidirectional)
    rules.extend(
        vec![
            rw!("and-assoc"; "(and ?x (and ?y ?z))" <=> "(and (and ?x ?y) ?z)"),
            rw!("or-assoc";  "(or ?x (or ?y ?z))"   <=> "(or (or ?x ?y) ?z)"),
        ]
        .concat(),
    );

    // Distribution (bidirectional -- lets the optimizer try both forms)
    rules.extend(
        vec![rw!("and-dist-or";
                "(and ?x (or ?y ?z))" <=> "(or (and ?x ?y) (and ?x ?z))")]
        .concat(),
    );

    rules
}

// ---------------------------------------------------------------------------
// 3. Cost function: prefer fewer nodes, penalise Or (conjunctive plans are cheaper)
// ---------------------------------------------------------------------------

struct ConjunctiveCost;

impl CostFunction<QueryIR> for ConjunctiveCost {
    type Cost = usize;

    fn cost<C>(&mut self, enode: &QueryIR, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        let op_cost = match enode {
            QueryIR::And(_) => 1,
            QueryIR::Or(_) => 3, // penalise disjunction
            QueryIR::Not(_) => 2,
            QueryIR::Symbol(_) => 1,
        };
        enode.fold(op_cost, |sum, id| sum + costs(id))
    }
}

// ---------------------------------------------------------------------------
// 4. Run it
// ---------------------------------------------------------------------------

fn main() {
    // Input: and(a, and(a, or(b, false)))
    // Expected simplification chain:
    //   or(b, false) -> b          (or-false)
    //   and(a, a)    -> a          (idempotent, after assoc)
    //   result: and(a, b)
    let input = "(and a (and a (or b false)))";
    let start: RecExpr<QueryIR> = input.parse().expect("parse input");

    let runner = Runner::default()
        .with_expr(&start)
        .with_iter_limit(20)
        .with_node_limit(10_000)
        .run(&rules());

    let extractor = Extractor::new(&runner.egraph, ConjunctiveCost);
    let (best_cost, best_expr) = extractor.find_best(runner.roots[0]);

    println!("before: {input}");
    println!("after:  {best_expr}");
    println!("cost:   {best_cost}");
    println!(
        "stopped after {} iterations ({:?})",
        runner.iterations.len(),
        runner.stop_reason.as_ref().unwrap()
    );

    // A second example: double negation + identity
    let input2 = "(and (not (not x)) true)";
    let start2: RecExpr<QueryIR> = input2.parse().expect("parse input2");

    let runner2 = Runner::default()
        .with_expr(&start2)
        .with_iter_limit(20)
        .with_node_limit(10_000)
        .run(&rules());

    let extractor2 = Extractor::new(&runner2.egraph, ConjunctiveCost);
    let (cost2, expr2) = extractor2.find_best(runner2.roots[0]);

    println!();
    println!("before: {input2}");
    println!("after:  {expr2}");
    println!("cost:   {cost2}");

    // Verify expectations
    assert_eq!(best_expr.to_string(), "(and a b)");
    assert_eq!(expr2.to_string(), "x");
}
