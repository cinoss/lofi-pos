//! Static handler that serves the built `apps/admin` SPA at `/ui/admin/*`.
//!
//! Falls back to `index.html` (with HTTP 200) for any unknown path so
//! client-side routing works (BrowserRouter basename `/ui/admin`). When
//! the directory does not exist (e.g., dev with no build yet) requests
//! return 404 — `serve` logs a warning at startup so it's visible.
//!
//! We use `ServeDir::fallback` (NOT `not_found_service`): the latter
//! wraps the fallback in `SetStatus<404>` so the SPA index would be
//! served with a 404 status, breaking browser hydration of react-router.

use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};

/// Build a `ServeDir` service that serves files under `admin_dist`,
/// falling back to `index.html` (with its native 200 status) for any
/// unknown path so client-side routing inside the SPA works.
pub fn service(admin_dist: PathBuf) -> ServeDir<ServeFile> {
    let index = admin_dist.join("index.html");
    ServeDir::new(admin_dist).fallback(ServeFile::new(index))
}
