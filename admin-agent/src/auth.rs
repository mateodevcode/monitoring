use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    extract::State,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
    body::Body,
    http::Request,
};
use jsonwebtoken::{encode, decode, Header, EncodingKey, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use totp_rs::{Algorithm, TOTP, Secret};
use std::sync::Arc;
use std::env;

#[derive(Clone)]
pub struct AuthState {
    pub jwt_secret: String,
    pub jwt_refresh_secret: String,
    pub admin_username: String,
    pub admin_password_hash: String,
    pub admin_totp_secret: String,
}

impl AuthState {
    pub fn from_env() -> Self {
        Self {
            jwt_secret: env::var("ADMIN_JWT_SECRET").expect("falta ADMIN_JWT_SECRET"),
            jwt_refresh_secret: env::var("ADMIN_JWT_REFRESH_SECRET").expect("falta ADMIN_JWT_REFRESH_SECRET"),
            admin_username: env::var("ADMIN_USERNAME").expect("falta ADMIN_USERNAME"),
            admin_password_hash: env::var("ADMIN_PASSWORD_HASH").expect("falta ADMIN_PASSWORD_HASH"),
            admin_totp_secret: env::var("ADMIN_TOTP_SECRET").expect("falta ADMIN_TOTP_SECRET"),
        }
    }
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub otp_code: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    token_type: String, // "access" o "refresh"
}

fn make_token(username: &str, secret: &str, minutes: i64, token_type: &str) -> String {
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::minutes(minutes))
        .unwrap()
        .timestamp() as usize;

    let claims = Claims {
        sub: username.to_string(),
        exp,
        token_type: token_type.to_string(),
    };

    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("error generando token")
}

pub async fn login(
    State(auth): State<Arc<AuthState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    // 1. Usuario único
    if payload.username != auth.admin_username {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 2. Password (Argon2)
    let parsed_hash = PasswordHash::new(&auth.admin_password_hash)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if Argon2::default()
        .verify_password(payload.password.as_bytes(), &parsed_hash)
        .is_err()
    {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 3. OTP (TOTP)
    let secret = Secret::Encoded(auth.admin_totp_secret.clone());
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret.to_bytes().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        None,
        "admin".to_string(),
    ).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !totp.check_current(&payload.otp_code).unwrap_or(false) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Todo válido -> emitir tokens
    let access_token = make_token(&payload.username, &auth.jwt_secret, 15, "access");
    let refresh_token = make_token(&payload.username, &auth.jwt_refresh_secret, 60 * 24 * 7, "refresh");

    // TODO: guardar refresh_token en threats.db para poder revocarlo (ver nota abajo)

    Ok(Json(LoginResponse { access_token, refresh_token }))
}

// Middleware que protege TODAS las rutas de control (exec, kill, files, etc.)
pub async fn require_auth(
    State(auth): State<Arc<AuthState>>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let decoded = decode::<Claims>(
        token,
        &DecodingKey::from_secret(auth.jwt_secret.as_bytes()),
        &Validation::default(),
    ).map_err(|_| StatusCode::UNAUTHORIZED)?;

    if decoded.claims.token_type != "access" {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}