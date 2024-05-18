pub mod canister_match;
pub mod denylist;
pub mod geoip;
pub mod headers;
pub mod request_id;
pub mod validate;

use std::str::FromStr;

use axum::extract::Request;
use fqdn::FQDN;

// Attempts to extract host from HTTP2 "authority" pseudo-header or from HTTP/1.1 "Host" header
fn extract_authority(request: &Request) -> Option<FQDN> {
    // Try HTTP2 first, then Host header
    request
        .uri()
        .authority()
        .map(|x| x.host())
        .or_else(|| {
            request
                .headers()
                .get(http::header::HOST)
                .and_then(|x| x.to_str().ok())
        })
        // Split if it has a port
        .and_then(|x| x.split(':').next())
        .and_then(|x| FQDN::from_str(x).ok())
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Error;
    use fqdn::fqdn;
    use http::{HeaderValue, Uri};

    #[test]
    fn test_extract_authority() -> Result<(), Error> {
        // Try with port
        let req = axum::extract::Request::builder()
            .method("GET")
            .version(axum::http::version::Version::HTTP_11)
            .uri("http://foo.bar:12345")
            .body(axum::body::Body::empty())
            .unwrap();

        let auth = extract_authority(&req);
        assert_eq!(auth, Some(fqdn!("foo.bar")));

        // Without port
        let req = axum::extract::Request::builder()
            .method("GET")
            .version(axum::http::version::Version::HTTP_11)
            .uri("http://foo.bar")
            .body(axum::body::Body::empty())
            .unwrap();

        let auth = extract_authority(&req);
        assert_eq!(auth, Some(fqdn!("foo.bar")));

        // HTTP2
        let req = axum::extract::Request::builder()
            .method("GET")
            .version(axum::http::version::Version::HTTP_2)
            .uri("http://foo.bar")
            .body(axum::body::Body::empty())
            .unwrap();

        let auth = extract_authority(&req);
        assert_eq!(auth, Some(fqdn!("foo.bar")));

        // Missing authority
        let mut req = axum::extract::Request::builder()
            .method("GET")
            .version(axum::http::version::Version::HTTP_2)
            .uri("http://foo.bar")
            .body(axum::body::Body::empty())
            .unwrap();
        *req.uri_mut() = Uri::default();

        let auth = extract_authority(&req);
        assert_eq!(auth, None);

        // Missing authority / present header
        let mut req = axum::extract::Request::builder()
            .method("GET")
            .version(axum::http::version::Version::HTTP_11)
            .uri("http://foo.bar")
            .body(axum::body::Body::empty())
            .unwrap();
        *req.uri_mut() = Uri::default();
        (*req.headers_mut()).insert(http::header::HOST, HeaderValue::from_static("foo.bar"));

        let auth = extract_authority(&req);
        assert_eq!(auth, Some(fqdn!("foo.bar")));

        // Badly formatted
        let mut req = axum::extract::Request::builder()
            .method("GET")
            .version(axum::http::version::Version::HTTP_2)
            .uri("http://foo.bar")
            .body(axum::body::Body::empty())
            .unwrap();
        *req.uri_mut() = Uri::default();
        req.headers_mut()
            .insert("Host", HeaderValue::from_static("foo|||bar"));

        let auth = extract_authority(&req);
        assert_eq!(auth, None);

        Ok(())
    }
}
