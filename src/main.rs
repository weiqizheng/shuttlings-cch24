use std::{
    net::{Ipv4Addr, Ipv6Addr},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header::LOCATION, HeaderMap, HeaderValue, Response, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use cargo_manifest::{Manifest, MaybeInherited::Local};
use jyt::{Converter, Ext};
use leaky_bucket::RateLimiter;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::Deserialize;
use tokio::sync::Mutex;

struct AppState {
    limiter: Mutex<RateLimiter>,
    game: Mutex<Game>,
}

#[shuttle_runtime::main]
async fn main() -> shuttle_axum::ShuttleAxum {
    let limiter = day_9_init_rate_limiter();
    let game = Game::new();
    let shared_state = Arc::new(AppState {
        limiter: Mutex::new(limiter),
        game: Mutex::new(game),
    });

    let router = Router::new()
        .route("/12/random-board", get(day_12_random_board))
        .route("/12/place/:team/:column", post(day_12_place))
        .route("/12/board", get(day_12_board))
        .route("/12/reset", post(day_12_reset))
        .route("/9/milk", post(day_9_milk))
        .route("/9/refill", post(day_9_refill))
        .route("/5/manifest", post(day_5_manifest))
        .route("/2/dest", get(day_2_dest))
        .route("/2/key", get(day_2_key))
        .route("/2/v6/dest", get(day_2_v6_dest))
        .route("/2/v6/key", get(day_2_v6_key))
        .route("/-1/seek", get(day_1_seek))
        .route("/", get(day_1_hello_world))
        .with_state(shared_state);

    Ok(router.into())
}

// day 12

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameItem {
    Wall,
    Empty,
    Cookie,
    Milk,
}

struct Game {
    board: [[GameItem; 6]; 5],
    winner: Option<GameItem>,
    board_full: bool,
    rng: StdRng,
}

impl Game {
    fn new() -> Self {
        let mut board = [[GameItem::Empty; 6]; 5];
        Self::reset_board(&mut board);
        Self {
            board,
            winner: None,
            board_full: false,
            rng: rand::rngs::StdRng::seed_from_u64(2024),
        }
    }

    fn reset(&mut self) {
        Self::reset_board(&mut self.board);
        self.winner = None;
        self.board_full = false;
        self.rng = rand::rngs::StdRng::seed_from_u64(2024);
    }

    fn is_column_full(&self, column: usize) -> bool {
        self.board[0][column] != GameItem::Empty
    }

    fn is_finished(&self) -> bool {
        self.winner.is_some() || self.board_full
    }

    fn put_item(&mut self, item: GameItem, column: usize) {
        for i in (0..4).rev() {
            if self.board[i][column] == GameItem::Empty {
                self.board[i][column] = item;
                break;
            }
        }
        // check wins
        self.check_win();

        // check full
        self.board_full = self.board[0].iter().all(|&item| item != GameItem::Empty);
    }

    fn put_random_item(&mut self, item: GameItem, row: usize, column: usize) {
        self.board[row][column] = item;
        // check wins
        self.check_win();
    }

    fn check_win(&mut self) {
        // check row
        for i in 0..4 {
            if self.board[i][1] != GameItem::Empty
                && self.board[i][1..5]
                    .windows(2)
                    .all(|pair| pair[0] == pair[1])
            {
                self.winner = Some(self.board[i][1]);
                return;
            }
        }
        // check column
        for j in 1..5 {
            if self.board[0][j] != GameItem::Empty
                && (1..4).all(|i| self.board[i - 1][j] == self.board[i][j])
            {
                self.winner = Some(self.board[0][j]);
                return;
            }
        }

        // check diagonals
        if self.board[0][1] != GameItem::Empty
            && (1..4).all(|i| self.board[i - 1][i] == self.board[i][i + 1])
        {
            self.winner = Some(self.board[0][1]);
            return;
        }

        if self.board[0][4] != GameItem::Empty
            && (1..4).all(|i| self.board[i - 1][5 - i] == self.board[i][4 - i])
        {
            self.winner = Some(self.board[0][4]);
        }
    }

    fn print_board(&self) -> String {
        let mut board = String::new();
        for i in 0..5 {
            for j in 0..6 {
                let cell = match self.board[i][j] {
                    GameItem::Wall => '⬜',
                    GameItem::Empty => '⬛',
                    GameItem::Cookie => '🍪',
                    GameItem::Milk => '🥛',
                };
                board.push(cell);
            }
            board.push('\n');
        }
        if self.winner.is_some() {
            board.push(match self.winner.unwrap() {
                GameItem::Cookie => '🍪',
                GameItem::Milk => '🥛',
                _ => unreachable!(),
            });
            board.push_str(" wins!\n");
        } else if self.board_full {
            board.push_str("No winner.\n");
        }
        board
    }

    fn reset_board(board: &mut [[GameItem; 6]; 5]) {
        for i in 0..4 {
            board[i][0] = GameItem::Wall;
            for j in 1..5 {
                board[i][j] = GameItem::Empty;
            }
            board[i][5] = GameItem::Wall;
        }
        for j in 0..6 {
            board[4][j] = GameItem::Wall;
        }
    }
}

async fn day_12_random_board(State(state): State<Arc<AppState>>) -> Response<Body> {
    let mut game = state.game.lock().await;
    for i in 0..4 {
        for j in 1..5 {
            let team = if game.rng.gen::<bool>() {
                GameItem::Cookie
            } else {
                GameItem::Milk
            };
            game.put_random_item(team, i, j);
        }
    }
    Response::new(Body::from(game.print_board()))
}

async fn day_12_place(
    State(state): State<Arc<AppState>>,
    path: Path<(String, i32)>,
) -> Response<Body> {
    let team = match path.0 .0.as_str() {
        "cookie" => GameItem::Cookie,
        "milk" => GameItem::Milk,
        _ => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::empty())
                .unwrap()
        }
    };
    let column = path.0 .1;
    if !(1..=4).contains(&column) {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::empty())
            .unwrap();
    }
    let column = column as usize;
    let mut game = state.game.lock().await;
    if game.is_column_full(column) {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::from(game.print_board()))
            .unwrap();
    }

    if game.is_finished() {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::from(game.print_board()))
            .unwrap();
    }
    game.put_item(team, column);
    Response::new(Body::from(game.print_board()))
}

async fn day_12_board(State(state): State<Arc<AppState>>) -> Response<Body> {
    Response::new(Body::from(state.game.lock().await.print_board()))
}

async fn day_12_reset(State(state): State<Arc<AppState>>) -> Response<Body> {
    let mut game = state.game.lock().await;
    game.reset();
    Response::new(Body::from(game.print_board()))
}

// day 9

fn day_9_init_rate_limiter() -> RateLimiter {
    RateLimiter::builder()
        .max(5)
        .initial(5)
        .interval(Duration::from_secs(1))
        .build()
}

fn day_9_bad_request() -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::empty())
        .unwrap()
}

async fn day_9_milk(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Response<Body> {
    let withdrawn = state.limiter.lock().await.try_acquire(1);
    match headers.get("content-type") {
        Some(content_type) if content_type == HeaderValue::from_static("application/json") => {
            match body.parse::<serde_json::Value>() {
                Ok(json) => match (
                    json.get("liters"),
                    json.get("gallons"),
                    json.get("litres"),
                    json.get("pints"),
                ) {
                    (Some(liters), None, None, None) => {
                        if let Some(liters) = liters.as_f64() {
                            let gallons = liters / 3.78541253;
                            let mut data = json::JsonValue::new_object();
                            data["gallons"] = gallons.into();
                            Response::new(Body::from(data.dump()))
                        } else {
                            day_9_bad_request()
                        }
                    }
                    (None, Some(gallons), None, None) => {
                        if let Some(gallons) = gallons.as_f64() {
                            let liters = gallons * 3.78541253;
                            let mut data = json::JsonValue::new_object();
                            data["liters"] = liters.into();
                            Response::new(Body::from(data.dump()))
                        } else {
                            day_9_bad_request()
                        }
                    }
                    (None, None, Some(litres), None) => {
                        if let Some(litres) = litres.as_f64() {
                            let pints = litres * 1.7598;
                            let mut data = json::JsonValue::new_object();
                            data["pints"] = pints.into();
                            Response::new(Body::from(data.dump()))
                        } else {
                            day_9_bad_request()
                        }
                    }
                    (None, None, None, Some(pints)) => {
                        if let Some(pints) = pints.as_f64() {
                            let litres = pints / 1.7598;
                            let mut data = json::JsonValue::new_object();
                            data["litres"] = litres.into();
                            Response::new(Body::from(data.dump()))
                        } else {
                            day_9_bad_request()
                        }
                    }
                    _ => day_9_bad_request(),
                },
                Err(_) => day_9_bad_request(),
            }
        }
        _ => {
            if withdrawn {
                Response::new(Body::from("Milk withdrawn\n"))
            } else {
                Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .body(Body::from("No milk available\n"))
                    .unwrap()
            }
        }
    }
}

async fn day_9_refill(State(state): State<Arc<AppState>>) -> Response<Body> {
    let mut limiter = state.limiter.lock().await;
    *limiter = day_9_init_rate_limiter();
    Response::new(Body::empty())
}

// day 5

fn day_5_handle_toml(body: String) -> Response<Body> {
    match Manifest::from_str(&body) {
        Ok(manifest) => {
            let contains_magic_keyword = match manifest
                .package
                .as_ref()
                .and_then(|package| package.keywords.as_ref())
            {
                Some(Local(keywords)) => keywords.iter().any(|keyword| keyword == "Christmas 2024"),
                _ => false,
            };
            if !contains_magic_keyword {
                return day_5_magic_keyword_response();
            }
            match manifest
                .package
                .as_ref()
                .and_then(|package| package.metadata.as_ref())
            {
                Some(metadata) => {
                    let mut orders = Vec::new();
                    if let Some(orders_array) =
                        metadata.get("orders").and_then(|orders| orders.as_array())
                    {
                        for order_item in orders_array {
                            if let (
                                Some(toml::Value::String(item)),
                                Some(toml::Value::Integer(quantity)),
                            ) = (order_item.get("item"), order_item.get("quantity"))
                            {
                                orders.push(format!("{}: {}", item, quantity));
                            }
                        }
                    }
                    if orders.is_empty() {
                        day_5_no_content_response()
                    } else {
                        Response::new(Body::from(orders.join("\n")))
                    }
                }
                _ => day_5_no_content_response(),
            }
        }
        Err(_) => day_5_invalid_manifest_response(),
    }
}

fn day_5_no_content_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap()
}

fn day_5_invalid_manifest_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from("Invalid manifest"))
        .unwrap()
}

fn day_5_magic_keyword_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from("Magic keyword not provided"))
        .unwrap()
}

fn day_5_unsupported_media_type_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
        .body(Body::empty())
        .unwrap()
}

async fn day_5_manifest(headers: HeaderMap, body: String) -> Response<Body> {
    match headers.get("content-type") {
        Some(content_type) if content_type == HeaderValue::from_static("application/toml") => {
            day_5_handle_toml(body)
        }
        Some(content_type) if content_type == HeaderValue::from_static("application/json") => {
            match body.to_toml(Ext::Json) {
                Ok(toml) => day_5_handle_toml(toml),
                Err(_) => day_5_invalid_manifest_response(),
            }
        }
        Some(content_type) if content_type == HeaderValue::from_static("application/yaml") => {
            match body.to_toml(Ext::Yaml) {
                Ok(toml) => day_5_handle_toml(toml),
                Err(_) => day_5_invalid_manifest_response(),
            }
        }
        _ => day_5_unsupported_media_type_response(),
    }
}

// day 2

#[derive(Deserialize)]
struct Day2DestQuery {
    from: Ipv4Addr,
    key: Ipv4Addr,
}

async fn day_2_dest(query: Query<Day2DestQuery>) -> impl IntoResponse {
    let mut from = query.from.octets();
    let key = query.key;
    from.iter_mut()
        .zip(key.octets())
        .for_each(|(from, key)| *from = from.wrapping_add(key));
    let dest = Ipv4Addr::new(from[0], from[1], from[2], from[3]);

    Html(dest.to_string())
}

#[derive(Deserialize)]
struct Day2KeyQuery {
    from: Ipv4Addr,
    to: Ipv4Addr,
}

async fn day_2_key(query: Query<Day2KeyQuery>) -> impl IntoResponse {
    let mut to = query.to.octets();
    let from = query.from;
    to.iter_mut()
        .zip(from.octets())
        .for_each(|(to, from)| *to = to.wrapping_sub(from));

    let key = Ipv4Addr::new(to[0], to[1], to[2], to[3]);
    Html(key.to_string())
}

#[derive(Deserialize)]
struct Day2V6DestQuery {
    from: Ipv6Addr,
    key: Ipv6Addr,
}

async fn day_2_v6_dest(query: Query<Day2V6DestQuery>) -> impl IntoResponse {
    let dest = Ipv6Addr::from_bits(query.from.to_bits() ^ query.key.to_bits());

    Html(dest.to_string())
}

#[derive(Deserialize)]
struct Day2V6KeyQuery {
    from: Ipv6Addr,
    to: Ipv6Addr,
}

async fn day_2_v6_key(query: Query<Day2V6KeyQuery>) -> impl IntoResponse {
    let key = Ipv6Addr::from_bits(query.from.to_bits() ^ query.to.to_bits());

    Html(key.to_string())
}

// day -1

async fn day_1_hello_world() -> &'static str {
    "Hello, bird!"
}

async fn day_1_seek() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(LOCATION, "https://www.youtube.com/watch?v=9Gc4QTqslN4")
        .body(Body::empty())
        .unwrap()
}
