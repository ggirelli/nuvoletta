//! Core library of Nuvoletta: the encrypted vault format and note logic.

/// Version of the encrypted file format. `0` means the format is not yet defined.
pub const FORMAT_VERSION: u16 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_is_undefined_until_designed() {
        assert_eq!(FORMAT_VERSION, 0);
    }
}
