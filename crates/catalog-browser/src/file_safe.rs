//! File names made from text such as addresses, for files and folders the
//! app keeps one of per team server or per library repo.

/// `text` as a file name. Every character but small letters, digits, `.`
/// and `-` is written as `_`, its code in hexadecimal and `_`, capitals
/// too. So two different texts never share a name, even where file names
/// ignore case, and every system accepts the name.
pub(crate) fn file_safe(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '-' {
                ch.to_string()
            } else {
                format!("_{:x}_", u32::from(ch))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::file_safe;

    #[test]
    fn names_differ_where_the_texts_differ_even_in_capitals() {
        assert_eq!(file_safe("https://a.b/C"), "https_3a__2f__2f_a.b_2f__43_");
        assert_ne!(file_safe("x/Acme"), file_safe("x/acme"));
        assert_ne!(file_safe("a_b"), file_safe("a-b"));
    }
}
