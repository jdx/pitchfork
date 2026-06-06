use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;
use std::sync::OnceLock;

#[derive(Embed)]
#[folder = "ui/dist/"]
struct Assets;

/// Token injected into `index.html` when the server is bound to a non-loopback address.
/// Set once at server startup via `set_static_token()`.
static STATIC_TOKEN: OnceLock<String> = OnceLock::new();

static STATIC_BASE: OnceLock<String> = OnceLock::new();

/// Token placeholder in the bundled index.html.
const TOKEN_PLACEHOLDER: &str = "__PF_TOKEN_PLACEHOLDER__";

/// Base path placeholder in the bundled index.html.
const BASE_PLACEHOLDER: &str = "__PF_BASE_PLACEHOLDER__";

/// Store the token so the static handler can inject it into index.html responses.
pub fn set_static_token(token: String) {
    let _ = STATIC_TOKEN.set(token);
}

/// Store the base path so the static handler can inject it into index.html responses.
pub fn set_static_base(base: String) {
    let _ = STATIC_BASE.set(base);
}

fn inject_into_index_html(data: &[u8]) -> Body {
    let html = String::from_utf8_lossy(data);
    let replaced = STATIC_TOKEN
        .get()
        .filter(|t| !t.is_empty())
        .map(|token| html.replace(TOKEN_PLACEHOLDER, token))
        .unwrap_or_else(|| html.into_owned());
    let replaced = STATIC_BASE
        .get()
        .map(|base| replaced.replace(BASE_PLACEHOLDER, base))
        .unwrap_or(replaced);
    Body::from(replaced.into_bytes())
}

/// Serve static files from the embedded Vue SPA dist folder.
///
/// Any request path is first tried as a static file. If not found,
/// falls back to `index.html` so the SPA router can handle client-side routes.
/// When serving `index.html`, replaces placeholders with the actual token and base path.
pub async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let data = content.data.to_vec();

            let body = if path == "index.html" {
                inject_into_index_html(&data)
            } else {
                Body::from(data)
            };

            let is_index = path == "index.html";
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref());
            if is_index {
                builder =
                    builder.header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate");
            }
            builder.body(body).unwrap()
        }
        None => {
            // SPA fallback: return index.html for unknown paths (client-side routing)
            match Assets::get("index.html") {
                Some(content) => {
                    let data = content.data.to_vec();
                    let body = inject_into_index_html(&data);

                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/html")
                        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
                        .body(body)
                        .unwrap()
                }
                None => Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("404 Not Found"))
                    .unwrap(),
            }
        }
    }
}
