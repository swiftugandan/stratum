use rstest::*;
use stratum_core::add;

#[fixture]
fn operands() -> (u64, u64) {
    (3, 7)
}

#[rstest]
fn test_add(operands: (u64, u64)) {
    let (a, b) = operands;
    assert_eq!(add(a, b), 10);
}

#[rstest]
#[case(0, 0, 0)]
#[case(1, 2, 3)]
#[case(100, 200, 300)]
fn test_add_parameterized(#[case] a: u64, #[case] b: u64, #[case] expected: u64) {
    assert_eq!(add(a, b), expected);
}
