use std::fmt;

use axum::{extract::Request, middleware::Next, response::Response};
use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};
use uuid::Uuid;

#[allow(clippy::declare_interior_mutable_const)]
pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Clone, Copy)]
pub struct RequestId(pub Uuid);

// Generate & insert request UUID into extensions and headers
pub async fn middleware(mut request: Request, next: Next) -> Response {
    let request_id = RequestId(Uuid::now_v7());
    let hdr = request_id.0.to_string();
    let hdr = HeaderValue::from_maybe_shared(Bytes::from(hdr)).unwrap();

    request.extensions_mut().insert(request_id);
    request.headers_mut().insert(HEADER, hdr.clone());

    let mut response = next.run(request).await;
    response.extensions_mut().insert(request_id);
    response.headers_mut().insert(HEADER, hdr);
    response
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
