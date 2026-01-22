pub struct Ignore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ignore() {
        let _ignore = Ignore;
    }
}
