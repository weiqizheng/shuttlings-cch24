use std::net::Ipv4Addr;

use axum::{
    body::Body,
    extract::Query,
    http::{header::LOCATION, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::Deserialize;

async fn hello_world() -> &'static str {
    "Hello, bird!"
}

async fn seek() -> impl IntoResponse {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_static("https://www.youtube.com/watch?v=9Gc4QTqslN4"),
    );
    response
}

// #[derive(Deserialize)]
// struct Day2Task1Query {
//     from: String,
//     key: String,
// }

// async fn day_2_task_1(query: Query<Day2Task1Query>) -> impl IntoResponse {
//     let from = query.from.parse::<Ipv4Addr>().unwrap();
//     let key = query.key.parse::<Ipv4Addr>().unwrap();
//     from.octets()
//         .into_iter()
//         .zip(key.octets())
//         .for_each(|(mut from, key)| from += key);
//     println!("from {from}");
// }

#[shuttle_runtime::main]
async fn main() -> shuttle_axum::ShuttleAxum {
    let router = Router::new()
        .route("/", get(hello_world))
        .route("/-1/seek", get(seek));

    Ok(router.into())
}
