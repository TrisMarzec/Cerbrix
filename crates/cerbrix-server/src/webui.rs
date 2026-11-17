//! Serves the web UI embedded in the binary: a single-page application whose files
//! `build.rs` includes from `ui/dist` (or one placeholder page when it was not built).
//!
//! [`router`] answers every path that no other route claims. It never answers for the APIs:
//! a path under `/api/` that matches no route gets a bare, empty 404 here (the JSON 404 of
//! `/api/v1` is made by that API itself and never reaches this fallback).

use axum::Router;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

/// One embedded file; `path` is relative to `ui/dist`, with `/` as separator.
pub struct Asset {
    pub path: &'static str,
    pub bytes: &'static [u8],
    pub content_type: &'static str,
    /// Quoted, ready for the `ETag` header.
    pub etag: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/webui_assets.rs"));

const CSP: &str = "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";

/// The routes of the UI: a fallback, so it never competes with a real route. Merge it last.
pub fn router() -> Router {
    Router::new().fallback(serve)
}

fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.path == path)
}

async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") || path == "/healthz" {
        // Exactly the answer the router gave before the UI existed: an empty 404.
        return StatusCode::NOT_FOUND.into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        let mut response = plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    if path.starts_with("/assets/") {
        // An exact lookup: `..` and encoded dots never name a file.
        return match find(&path[1..]) {
            Some(asset) => asset_response(asset, IMMUTABLE, &headers),
            None => plain(StatusCode::NOT_FOUND, "not found"),
        };
    }
    if path == "/licenses.txt" {
        return match find("licenses.txt") {
            Some(asset) => asset_response(asset, NO_CACHE, &headers),
            None => plain(StatusCode::NOT_FOUND, "not found"),
        };
    }
    // Every other path is a page of the application; the client routes it.
    match find("index.html") {
        Some(asset) => asset_response(asset, NO_CACHE, &headers),
        None => plain(StatusCode::NOT_FOUND, "not found"),
    }
}

fn plain(status: StatusCode, text: &'static str) -> Response {
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        text,
    )
        .into_response();
    secure(&mut response);
    response
}

fn asset_response(
    asset: &'static Asset,
    cache_control: &'static str,
    request: &HeaderMap,
) -> Response {
    let status = if matches_etag(request, asset.etag) {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let body: &'static [u8] = if status == StatusCode::OK {
        asset.bytes
    } else {
        &[]
    };
    let mut response = (
        status,
        [
            (header::CONTENT_TYPE, asset.content_type),
            (header::CACHE_CONTROL, cache_control),
            (header::ETAG, asset.etag),
        ],
        body,
    )
        .into_response();
    secure(&mut response);
    response
}

fn secure(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
}

/// True when `If-None-Match` names `etag` (weak comparison) or is `*`.
fn matches_etag(request: &HeaderMap, etag: &str) -> bool {
    request
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == etag)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use sqlx::PgPool;
    use tower::ServiceExt;

    use super::*;
    use crate::api::ui::testing::Fixture;

    async fn send(
        router: &Router,
        method: &str,
        uri: &str,
        if_none_match: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(v) = if_none_match {
            req = req.header(header::IF_NONE_MATCH, v);
        }
        let resp = router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, bytes.to_vec())
    }

    fn header_of<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
        headers
            .get(name)
            .unwrap_or_else(|| panic!("missing header {name}"))
            .to_str()
            .unwrap()
    }

    fn assert_secured(headers: &HeaderMap) {
        assert_eq!(header_of(headers, "content-security-policy"), CSP);
        assert_eq!(header_of(headers, "x-content-type-options"), "nosniff");
        assert_eq!(header_of(headers, "referrer-policy"), "no-referrer");
        assert_eq!(
            header_of(headers, "cross-origin-opener-policy"),
            "same-origin"
        );
    }

    fn index() -> &'static Asset {
        find("index.html").expect("index.html is always embedded")
    }

    #[tokio::test]
    async fn root_serves_index_no_cache_with_csp() {
        let (status, headers, body) = send(&router(), "GET", "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, index().bytes);
        assert_eq!(header_of(&headers, "cache-control"), "no-cache");
        assert_eq!(header_of(&headers, "etag"), index().etag);
        assert!(header_of(&headers, "content-type").starts_with("text/html"));
        assert_secured(&headers);
    }

    #[tokio::test]
    async fn deep_link_serves_index() {
        for uri in ["/hosts/db-02", "/problems?state=resolved", "/display/x"] {
            let (status, headers, body) = send(&router(), "GET", uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert_eq!(body, index().bytes, "{uri}");
            assert_eq!(header_of(&headers, "cache-control"), "no-cache", "{uri}");
            assert_secured(&headers);
        }
    }

    #[tokio::test]
    async fn head_serves_headers_of_index() {
        let (status, headers, _) = send(&router(), "HEAD", "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(header_of(&headers, "etag"), index().etag);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn api_paths_are_not_swallowed(pool: PgPool) {
        let app = Fixture::new(pool).await.router;

        let (status, headers, body) = send(&app, "GET", "/api/v1/nope", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(header_of(&headers, "content-type").starts_with("application/json"));
        assert_eq!(header_of(&headers, "cache-control"), "no-store");
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"], "not_found");

        for uri in ["/api/agent/v1/nope", "/api/nope", "/api"] {
            let (status, headers, body) = send(&app, "GET", uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert!(body.is_empty(), "{uri}");
            assert!(headers.get("content-security-policy").is_none(), "{uri}");
        }
        let (status, _, body) = send(&app, "POST", "/api/nope", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_ne!(body, index().bytes);

        let (status, headers, body) = send(&app, "GET", "/healthz", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(header_of(&headers, "content-type").starts_with("application/json"));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");

        // The pages are served by the composed router too.
        let (status, _, body) = send(&app, "GET", "/hosts/db-02", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, index().bytes);
    }

    #[tokio::test]
    async fn asset_is_immutable_and_typed() {
        for asset in ASSETS.iter().filter(|a| a.path.starts_with("assets/")) {
            let uri = format!("/{}", asset.path);
            let (status, headers, body) = send(&router(), "GET", &uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert_eq!(body, asset.bytes, "{uri}");
            assert_eq!(
                header_of(&headers, "cache-control"),
                "public, max-age=31536000, immutable"
            );
            assert_eq!(header_of(&headers, "content-type"), asset.content_type);
            assert_eq!(header_of(&headers, "etag"), asset.etag);
            assert_secured(&headers);
        }
    }

    #[tokio::test]
    async fn licenses_are_plain_text_no_cache() {
        let (status, headers, _) = send(&router(), "GET", "/licenses.txt", None).await;
        if find("licenses.txt").is_some() {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                header_of(&headers, "content-type"),
                "text/plain; charset=utf-8"
            );
            assert_eq!(header_of(&headers, "cache-control"), "no-cache");
        } else {
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn unknown_asset_is_404_not_index() {
        let (status, headers, body) =
            send(&router(), "GET", "/assets/nothing-1a2b3c.js", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(header_of(&headers, "content-type").starts_with("text/plain"));
        assert_ne!(body, index().bytes);
        assert_secured(&headers);
    }

    #[tokio::test]
    async fn if_none_match_gives_304() {
        let tag = index().etag;
        for value in [
            tag.to_owned(),
            format!("W/{tag}"),
            format!("\"x\", {tag}"),
            "*".into(),
        ] {
            let (status, headers, body) = send(&router(), "GET", "/", Some(&value)).await;
            assert_eq!(status, StatusCode::NOT_MODIFIED, "{value}");
            assert!(body.is_empty());
            assert_eq!(header_of(&headers, "etag"), tag);
        }
        let (status, _, _) = send(&router(), "GET", "/", Some("\"other\"")).await;
        assert_eq!(status, StatusCode::OK);
        if let Some(asset) = ASSETS.iter().find(|a| a.path.starts_with("assets/")) {
            let uri = format!("/{}", asset.path);
            let (status, _, _) = send(&router(), "GET", &uri, Some(asset.etag)).await;
            assert_eq!(status, StatusCode::NOT_MODIFIED);
        }
    }

    #[tokio::test]
    async fn post_to_a_page_is_405() {
        for uri in ["/", "/hosts/db-02", "/assets/x.js", "/licenses.txt"] {
            let (status, headers, _) = send(&router(), "POST", uri, None).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{uri}");
            assert_eq!(header_of(&headers, "allow"), "GET, HEAD");
        }
        let (status, _, _) = send(&router(), "DELETE", "/", None).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn path_traversal_is_not_served() {
        for uri in [
            "/assets/../Cargo.toml",
            "/assets/%2e%2e/x",
            "/assets/..%2f..%2fCargo.toml",
            "/assets//etc/passwd",
        ] {
            let (status, _, body) = send(&router(), "GET", uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert!(!String::from_utf8_lossy(&body).contains("[package]"));
        }
    }

    #[tokio::test]
    async fn placeholder_says_how_to_build() {
        if !PLACEHOLDER {
            return;
        }
        let (_, _, body) = send(&router(), "GET", "/", None).await;
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("The web UI was not built."));
        assert!(text.contains("npm ci &amp;&amp; npm run build"));
        assert_eq!(ASSETS.len(), 1);
    }

    /// Run by CI after `npm run build` (`--ignored`): the real UI is embedded.
    #[test]
    #[ignore = "needs ui/dist: run after `npm run build` in ui/"]
    fn ui_is_embedded() {
        if PLACEHOLDER {
            panic!("ui/dist was missing when the server was built");
        }
        let html = std::str::from_utf8(index().bytes).unwrap();
        let mut scripts = 0;
        for part in html.split("src=\"/").skip(1) {
            let reference = part.split('"').next().unwrap();
            if reference.starts_with("assets/") && reference.ends_with(".js") {
                assert!(find(reference).is_some(), "{reference} is not embedded");
                scripts += 1;
            }
        }
        assert!(scripts >= 1, "index.html references no assets/*.js");
    }
}
