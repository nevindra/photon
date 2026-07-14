//! Session authentication + user management. Password login/logout (argon2), first-run setup,
//! the boot session probe, the auth gate, and the authenticated user CRUD endpoints. Users are
//! persisted in the `UserStore` (SQLite); session cookies are signed and stateless.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::cookie::{Cookie, SameSite, SignedCookieJar};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;

/// Name of the signed session cookie. Its value is the authenticated username.
const SESSION_COOKIE: &str = "photon_session";
/// How long a session cookie stays valid (survives browser restart, not just a refresh).
const SESSION_DAYS: i64 = 7;
/// Minimum password length enforced on setup + user creation.
const MIN_PASSWORD_LEN: usize = 8;
/// Maximum username length.
const MAX_USERNAME_LEN: usize = 64;

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct SetupRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct CreateUserRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SessionResponse {
    authenticated: bool,
    username: Option<String>,
    needs_setup: bool,
}

#[derive(Serialize)]
struct UserView {
    username: String,
    created_at: i64,
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// Build the signed session cookie for `username` and add it to the jar.
fn set_session(jar: SignedCookieJar, username: &str) -> SignedCookieJar {
    let mut cookie = Cookie::new(SESSION_COOKIE, username.to_string());
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(time::Duration::days(SESSION_DAYS));
    jar.add(cookie)
}

/// Validate a new credential pair. Returns a human-readable message on failure.
fn validate_credentials(username: &str, password: &str) -> Result<(), String> {
    let u = username.trim();
    if u.is_empty() {
        return Err("username must not be empty".into());
    }
    if u.len() > MAX_USERNAME_LEN {
        return Err(format!(
            "username must be at most {MAX_USERNAME_LEN} characters"
        ));
    }
    if password.len() < MIN_PASSWORD_LEN {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    Ok(())
}

/// Hash a password with argon2id + an OS-random salt.
fn hash_password_prod(password: &str) -> Result<String, PhotonErrorHashFailed> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| PhotonErrorHashFailed)
}

/// Marker for a hashing failure (mapped to 500 by callers).
struct PhotonErrorHashFailed;

/// `GET /api/session` — the boot probe. Open (reads the cookie itself). Tells the SPA whether it
/// must onboard, whether it is authenticated, and as whom. Fixes the refresh bug: the httpOnly
/// cookie is invisible to JS, so the SPA asks the server on load.
pub(crate) async fn session(State(state): State<AppState>, jar: SignedCookieJar) -> Response {
    let needs_setup = match state.users.count().await {
        Ok(n) => n == 0,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut authenticated = false;
    let mut username = None;
    if let Some(c) = jar.get(SESSION_COOKIE) {
        let name = c.value().to_string();
        if matches!(state.users.get(&name).await, Ok(Some(_))) {
            authenticated = true;
            username = Some(name);
        }
    }
    Json(SessionResponse {
        authenticated,
        username,
        needs_setup,
    })
    .into_response()
}

/// `POST /api/setup` — first-run onboarding. Open, but only while no users exist; once any user
/// exists it returns 409. This is the boundary that keeps setup from being open signup.
pub(crate) async fn setup(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Json(body): Json<SetupRequest>,
) -> Response {
    match state.users.count().await {
        Ok(0) => {}
        Ok(_) => return error_response(StatusCode::CONFLICT, "setup already complete"),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    if let Err(msg) = validate_credentials(&body.username, &body.password) {
        return error_response(StatusCode::BAD_REQUEST, &msg);
    }
    let username = body.username.trim().to_string();
    let hash = match hash_password_prod(&body.password) {
        Ok(h) => h,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if state.users.create(&username, &hash).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    (set_session(jar, &username), Json(json!({}))).into_response()
}

/// `POST /api/login` — verify credentials against the store, then set a signed session cookie.
pub(crate) async fn login(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Json(body): Json<LoginRequest>,
) -> Response {
    let verified = match state.users.get(&body.username).await {
        Ok(Some(u)) => verify_password(&body.password, &u.password_hash),
        Ok(None) => false,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if !verified {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    (set_session(jar, &body.username), Json(json!({}))).into_response()
}

/// `POST /api/logout` — clear the session cookie. Always `204`.
pub(crate) async fn logout(jar: SignedCookieJar) -> Response {
    let mut cookie = Cookie::new(SESSION_COOKIE, "");
    cookie.set_path("/");
    (jar.remove(cookie), StatusCode::NO_CONTENT).into_response()
}

/// `GET /api/users` — list users (authenticated).
pub(crate) async fn list_users(State(state): State<AppState>) -> Response {
    match state.users.list().await {
        Ok(users) => Json(json!({
            "users": users
                .into_iter()
                .map(|u| UserView { username: u.username, created_at: u.created_at })
                .collect::<Vec<_>>()
        }))
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /api/users` — add a user (authenticated). 201 / 409 duplicate / 400 invalid.
pub(crate) async fn create_user(
    State(state): State<AppState>,
    Json(body): Json<CreateUserRequest>,
) -> Response {
    if let Err(msg) = validate_credentials(&body.username, &body.password) {
        return error_response(StatusCode::BAD_REQUEST, &msg);
    }
    let username = body.username.trim().to_string();
    match state.users.get(&username).await {
        Ok(Some(_)) => return error_response(StatusCode::CONFLICT, "user already exists"),
        Ok(None) => {}
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    let hash = match hash_password_prod(&body.password) {
        Ok(h) => h,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match state.users.create(&username, &hash).await {
        Ok(()) => (StatusCode::CREATED, Json(json!({ "username": username }))).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `DELETE /api/users/:username` — remove a user (authenticated). Anti-lockout: you cannot delete
/// yourself, nor the last remaining user.
pub(crate) async fn delete_user(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(username): Path<String>,
) -> Response {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        if c.value() == username {
            return error_response(
                StatusCode::BAD_REQUEST,
                "you cannot delete your own account",
            );
        }
    }
    match state.users.count().await {
        Ok(n) if n <= 1 => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "cannot delete the last remaining user",
            )
        }
        Ok(_) => {}
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    match state.users.delete(&username).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, "user not found"),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Auth gate: pass through only when a valid signed `photon_session` cookie is present **and**
/// its username still exists in the store (so deleting a user revokes their sessions).
pub(crate) async fn require_auth(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    request: Request,
    next: Next,
) -> Response {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        if matches!(state.users.get(c.value()).await, Ok(Some(_))) {
            return next.run(request).await;
        }
    }
    StatusCode::UNAUTHORIZED.into_response()
}

/// Verify `password` against a stored argon2 PHC hash. Any parse/verify failure is a `false`.
fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::users::{SqliteUserStore, UserStore};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt; // for `oneshot`

    fn hash(pw: &str) -> String {
        use argon2::password_hash::SaltString;
        use argon2::{Argon2, PasswordHasher};
        let salt = SaltString::encode_b64(b"photon-test-salt").unwrap();
        Argon2::default()
            .hash_password(pw.as_bytes(), &salt)
            .unwrap()
            .to_string()
    }

    /// A router over a store seeded with the given (username, password) pairs.
    fn router_with(users: &[(&str, &str)]) -> axum::Router {
        let store = SqliteUserStore::open_in_memory().unwrap();
        for (u, p) in users {
            store.seed(u, &hash(p));
        }
        let users: Arc<dyn UserStore> = Arc::new(store);
        crate::test_server_with_users(users).into_router()
    }

    fn json_body(v: serde_json::Value) -> Body {
        Body::from(v.to_string())
    }

    async fn read_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::json!(null))
    }

    #[tokio::test]
    async fn session_reports_needs_setup_when_empty() {
        let app = router_with(&[]);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_json(resp).await;
        assert_eq!(body["needs_setup"], true);
        assert_eq!(body["authenticated"], false);
    }

    #[tokio::test]
    async fn setup_creates_first_user_logs_in_then_is_locked() {
        let app = router_with(&[]);
        // First setup succeeds and returns a cookie.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/setup")
                    .header("content-type", "application/json")
                    .body(json_body(
                        serde_json::json!({"username":"admin","password":"hunter2!"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("photon_session="));

        // Second setup is refused (409) — this is what prevents open signup.
        let resp2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/setup")
                    .header("content-type", "application/json")
                    .body(json_body(
                        serde_json::json!({"username":"mallory","password":"hunter2!"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn setup_rejects_short_password() {
        let app = router_with(&[]);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/setup")
                    .header("content-type", "application/json")
                    .body(json_body(
                        serde_json::json!({"username":"admin","password":"short"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn login_accepts_correct_and_rejects_wrong() {
        let app = router_with(&[("admin", "hunter2")]);
        let ok = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("content-type", "application/json")
                    .body(json_body(
                        serde_json::json!({"username":"admin","password":"hunter2"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);

        let bad = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("content-type", "application/json")
                    .body(json_body(
                        serde_json::json!({"username":"admin","password":"nope"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_auth_rejects_cookie_for_deleted_user() {
        // Seed two users so we can delete one without tripping the last-user guard.
        let store = SqliteUserStore::open_in_memory().unwrap();
        store.seed("admin", &hash("hunter2"));
        store.seed("ghost", &hash("hunter2"));
        let users: Arc<dyn UserStore> = Arc::new(store);
        let app = crate::test_server_with_users(users.clone()).into_router();

        // Log in as ghost, capture the cookie.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("content-type", "application/json")
                    .body(json_body(
                        serde_json::json!({"username":"ghost","password":"hunter2"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();

        // Remove ghost out-of-band, then the cookie must no longer authorize.
        users.delete("ghost").await.unwrap();
        let resp2 = app
            .oneshot(
                Request::builder()
                    .uri("/api/services")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn user_crud_and_guards() {
        let app = router_with(&[("admin", "hunter2")]);
        let cookie = crate::session_cookie(&app).await;

        // Create a user.
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/users")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(json_body(
                        serde_json::json!({"username":"bob","password":"hunter2!"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        // Duplicate → 409.
        let dup = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/users")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(json_body(
                        serde_json::json!({"username":"bob","password":"hunter2!"}),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dup.status(), StatusCode::CONFLICT);

        // Deleting yourself → 400.
        let self_del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/users/admin")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(self_del.status(), StatusCode::BAD_REQUEST);

        // Deleting bob → 204.
        let ok_del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/users/bob")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok_del.status(), StatusCode::NO_CONTENT);

        // admin is now the last user — deleting anyone that leaves zero is blocked; deleting the
        // last remaining user (via a different account path) is blocked by the count guard.
        let last = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/users/admin")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Self-delete guard fires first here (admin is the caller); either way it is refused.
        assert_eq!(last.status(), StatusCode::BAD_REQUEST);
    }
}
