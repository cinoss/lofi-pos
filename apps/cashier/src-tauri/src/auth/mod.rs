//! Authentication primitives: PIN hashing (Argon2id), HMAC-signed bearer
//! tokens, and a high-level `AuthService` orchestrating login and token
//! verification against the staff table.

pub mod pin;
pub mod service;
pub mod token;

pub use service::AuthService;
