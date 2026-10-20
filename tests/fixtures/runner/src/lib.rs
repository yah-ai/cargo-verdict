/// ```
/// assert_eq!(runner::two(), 2);
/// ```
///
/// ```
/// assert_eq!(runner::two() + 1, 3);
/// ```
pub fn two() -> u32 {
    2
}

#[cfg(test)]
mod t {
    #[test]
    fn ok() {}

    #[test]
    fn also_ok() {}
}
