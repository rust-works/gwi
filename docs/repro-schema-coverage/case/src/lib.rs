//! Keeps the generated fallback unused during successful registration.

fn schema() -> Result<bool, &'static str> {
    Ok(true)
}

/// Supplies the source span for the generated metadata identifier.
#[span_macro::metadata("x")]
pub fn handler() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_accepts_the_schema() {
        assert!(handler_metadata().1);
    }
}
