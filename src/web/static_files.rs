use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, Uri, header},
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
    let base = STATIC_BASE.get().map(String::as_str).unwrap_or("");
    let replaced = replaced.replace(BASE_PLACEHOLDER, base);
    // The bundle references assets with relative URLs (vite `base: ''`) so it
    // works both at the root and under a sub-path (`web_path`). A <base> tag
    // anchors those URLs to the app root; without it, reloading a nested SPA
    // route like /daemon/:id resolves them against the route path instead.
    let replaced = replaced.replacen("<head>", &format!("<head>\n  <base href=\"{base}/\">"), 1);
    Body::from(replaced.into_bytes())
}

/// Serve static files from the embedded Vue SPA dist folder.
///
/// Any request path is first tried as a static file. If not found,
/// falls back to `index.html` so the SPA router can handle client-side routes.
/// When serving `index.html`, replaces placeholders with the actual token and base path.
pub async fn static_handler(uri: Uri, headers: HeaderMap) -> impl IntoResponse {
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
            // SPA fallback: return index.html for unknown paths (client-side routing).
            // Only do this for navigation requests (Accept: text/html); scripts,
            // stylesheets, and other subresources request with a different Accept
            // and must get a 404 rather than index.html with the wrong MIME type.
            let accepts_html = headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("text/html"));
            if !accepts_html {
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("404 Not Found"))
                    .unwrap();
            }
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
