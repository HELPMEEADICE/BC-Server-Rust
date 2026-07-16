use anyhow::Result;

/// Hash password with bcrypt cost 10 after uppercasing (Node compatibility).
pub fn hash_password(password: &str) -> Result<String> {
    let upper = password.to_uppercase();
    let hash = bcrypt::hash(upper, 10)?;
    Ok(hash)
}

/// Verify password against bcrypt hash (uppercase before compare).
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let upper = password.to_uppercase();
    Ok(bcrypt::verify(upper, hash)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercase_roundtrip() {
        let h = hash_password("abc123").unwrap();
        assert!(verify_password("abc123", &h).unwrap());
        assert!(verify_password("ABC123", &h).unwrap());
        assert!(!verify_password("wrong", &h).unwrap());
    }
}
