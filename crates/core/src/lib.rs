pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;

    #[rstest]
    #[case(2, 2, 4)]
    #[case(0, 0, 0)]
    fn test_add(#[case] a: u64, #[case] b: u64, #[case] expected: u64) {
        assert_eq!(add(a, b), expected);
    }
}
