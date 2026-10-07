use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // username
    pub role: String, // "owner" | "tenant"
    pub exp: usize,
}

pub fn sign_token(secret: &str, username: &str, role: &str, ttl_hours: i64) -> anyhow::Result<String> {
    let exp = (chrono::Utc::now() + chrono::Duration::hours(ttl_hours)).timestamp() as usize;
    let claims = Claims {
        sub: username.to_string(),
        role: role.to_string(),
        exp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

pub fn verify_token(secret: &str, token: &str) -> anyhow::Result<Claims> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    Ok(data.claims)
}
