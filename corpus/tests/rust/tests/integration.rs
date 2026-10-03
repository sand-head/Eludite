//! An integration test of the corpus package (a test target of its own).

#[test]
fn adds_from_outside() {
    assert_eq!(corpus_tests::add(1, 1), 2);
}
