# qplan

Compile typed query expressions (`qexpr`) into execution-friendly query plans.

This crate is intentionally narrow today: it supports compiling a subset of `QExpr`
into a conjunctive plan (`AND` of terms/phrases/near constraints). Unsupported operators
return explicit errors so downstream systems can choose a fallback.

## Usage

```toml
[dependencies]
qplan = { git = "https://github.com/arclabs561/qplan" }
```

Example:

```rust
use qexpr::{Phrase, QExpr, Term};
use qplan::compile_conjunctive;

let q = QExpr::And(vec![
    QExpr::Term(Term::new("alpha")),
    QExpr::Phrase(Phrase::new(vec![Term::new("new"), Term::new("york")])),
]);

let p = compile_conjunctive(&q).unwrap();
assert!(!p.bag_terms.is_empty());
```

## Development

```bash
cargo test
```
