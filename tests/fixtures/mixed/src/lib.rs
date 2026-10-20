/// ```
/// assert_eq!(mixed::two(), 2);
/// ```
pub fn two() -> u32 {
    let unused = 1;
    2
}

#[cfg(test)]
mod t {
    #[test]
    fn ok() {}

    #[test]
    fn bad() {
        println!("captured before the panic");
        panic!("boom");
    }

    #[test]
    #[ignore = "slow"]
    fn ign() {}
}
