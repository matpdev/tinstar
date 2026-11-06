//! The example project is the demo and the reference: every planted violation must be found,
//! and nothing else.

use std::path::{Path, PathBuf};

use tinstar_core::Lawbook;

fn shop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/shop")
        .canonicalize()
        .unwrap()
}

#[test]
fn every_law_passes_its_examples() {
    let (book, errors) = Lawbook::load(&shop());
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(book.laws.len(), 9);
    for law in &book.laws {
        let r = law.test_examples();
        assert!(
            r.failures.is_empty(),
            "{} failed: {:?}",
            r.law,
            r.failures.iter().map(|f| &f.reason).collect::<Vec<_>>()
        );
    }
}

#[test]
fn finds_exactly_the_planted_violations() {
    let (book, _) = Lawbook::load(&shop());
    let mut found: Vec<String> = book
        .check_project(None)
        .iter()
        .map(|d| match d.span {
            Some(s) => format!("{} {}:{}", d.law, d.path.display(), s.line),
            None => format!("{} {}", d.law, d.path.display()),
        })
        .collect();
    found.sort();
    let mut expected = vec![
        "no-committed-env .env",
        "gha-pinned-actions .github/workflows/ci.yml:7",
        "k8s-resource-limits deploy/api.yaml:10",
        "k8s-pinned-images deploy/api.yaml:11",
        "migration-names migrations/AddEmail.sql",
        "no-float-money src/cart.ts:11",
        "no-console-log src/cart.ts:12",
        "feature-has-test src/features/catalog",
        "payments-only-in-gateway src/features/checkout/index.ts:4",
    ];
    expected.sort();
    assert_eq!(found, expected);
}

#[test]
fn fixes_and_in_memory_sources() {
    let (book, _) = Lawbook::load(&shop());
    // What an editor sends: unsaved text for a path.
    let d = book.check_source(
        Path::new("src/new.ts"),
        "console.log(a, b);\nconst price: number = 1;\n",
    );
    let laws: Vec<&str> = d.iter().map(|d| d.law.as_str()).collect();
    assert_eq!(laws, ["no-console-log", "no-float-money"]);
    let fix = d[0].fix.as_ref().expect("console.log has a fix");
    assert_eq!(fix.replacement, "logger.info(a, b)");
    assert_eq!((fix.start, fix.end), (0, 17));
    // Outside `files`, nothing applies.
    assert!(
        book.check_source(Path::new("scripts/x.ts"), "console.log(1)")
            .is_empty()
    );
}
