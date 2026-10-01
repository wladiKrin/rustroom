//! Words the status line shares across features.

/// "1 photo", "3 photos".
pub fn photos(n: usize) -> String {
    if n == 1 {
        "1 photo".to_string()
    } else {
        format!("{n} photos")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_singular_and_the_rest_plural() {
        assert_eq!(photos(1), "1 photo");
        assert_eq!(photos(0), "0 photos");
        assert_eq!(photos(3), "3 photos");
    }
}
