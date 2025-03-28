use std::{
    collections::HashMap,
    net::{Ipv4Addr, Ipv6Addr},
    num::ParseIntError,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::LOCATION},
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use base64::prelude::*;
// use cargo_lock::Lockfile;
use cargo_manifest::{Manifest, MaybeInherited::Local};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use jyt::{Converter, Ext};
use leaky_bucket::RateLimiter;
use rand::{Rng, SeedableRng, distributions::Alphanumeric, rngs::StdRng, thread_rng};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgQueryResult, prelude::FromRow, types::uuid};
use tokio::sync::Mutex;
use tower_http::services::ServeDir;
use tracing::*;

#[shuttle_runtime::main]
async fn main(#[shuttle_shared_db::Postgres] pool: PgPool) -> shuttle_axum::ShuttleAxum {
    create_database(&pool)
        .await
        .expect("Failed to create database");

    let router = Router::new()
        .nest_service("/assets", ServeDir::new("assets"))
        .route("/23/star", get(day_23_star))
        .route("/23/present/{color}", get(day_23_present))
        .route("/23/ornament/{state}/{n}", get(day_23_ornament))
        .route("/23/lockfile", post(day_23_lockfile))
        .route("/19/reset", post(day_19_reset))
        .route("/19/cite/{id}", get(day_19_cite))
        .route("/19/remove/{id}", delete(day_19_remove))
        .route("/19/undo/{id}", put(day_19_undo))
        .route("/19/draft", post(day_19_draft))
        .route("/19/list", get(day_19_list))
        .with_state(Arc::new(Day19AppState {
            pool,
            pages: Mutex::new(HashMap::new()),
        }))
        .route("/16/decode", post(day_16_decode))
        .route("/16/wrap", post(day_16_wrap))
        .route("/16/unwrap", get(day_16_unwrap))
        .route("/12/random-board", get(day_12_random_board))
        .route("/12/place/{team}/{column}", post(day_12_place))
        .route("/12/board", get(day_12_board))
        .route("/12/reset", post(day_12_reset))
        .with_state(Arc::new(Day12AppState {
            game: Mutex::new(Game::new()),
        }))
        .route("/9/milk", post(day_9_milk))
        .route("/9/refill", post(day_9_refill))
        .with_state(Arc::new(Day9AppState {
            limiter: Mutex::new(day_9_init_rate_limiter()),
        }))
        .route("/5/manifest", post(day_5_manifest))
        .route("/2/dest", get(day_2_dest))
        .route("/2/key", get(day_2_key))
        .route("/2/v6/dest", get(day_2_v6_dest))
        .route("/2/v6/key", get(day_2_v6_key))
        .route("/-1/seek", get(day_1_seek))
        .route("/", get(day_1_hello_world));

    Ok(router.into())
}

// day 23

async fn day_23_star() -> impl IntoResponse {
    r#"<div id="star" class="lit"></div>"#
}

async fn day_23_present(Path(color): Path<String>) -> (StatusCode, String) {
    let next_color = match color.as_str() {
        "red" => "blue",
        "blue" => "purple",
        "purple" => "red",
        _ => {
            return (StatusCode::IM_A_TEAPOT, "".to_string());
        }
    };
    (
        StatusCode::OK,
        format!(
            r#"<div class="present {color}" hx-get="/23/present/{next_color}" hx-swap="outerHTML">
                <div class="ribbon"></div>
                <div class="ribbon"></div>
                <div class="ribbon"></div>
                <div class="ribbon"></div>
            </div>"#
        ),
    )
}

async fn day_23_ornament(Path((state, n)): Path<(String, String)>) -> (StatusCode, String) {
    let next_state = match state.as_str() {
        "on" => "off",
        "off" => "on",
        _ => {
            return (StatusCode::IM_A_TEAPOT, "".to_string());
        }
    };
    let n = htmlescape::encode_minimal(&n);
    // changed is removed in hx-trigger
    (
        StatusCode::OK,
        format!(
            r#"<div class="ornament{}" id="ornament{n}" hx-trigger="load delay:2s once" hx-get="/23/ornament/{next_state}/{n}" hx-swap="outerHTML"></div>"#,
            if state == "on" { " on" } else { "" }
        ),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct LockfileChecksum {
    color: i32,
    top: u8,
    left: u8,
}

#[derive(Debug, PartialEq, Eq)]
struct ParseChecksumError;

impl From<ParseIntError> for ParseChecksumError {
    fn from(_: ParseIntError) -> Self {
        ParseChecksumError
    }
}

impl FromStr for LockfileChecksum {
    type Err = ParseChecksumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() < 10 {
            return Err(ParseChecksumError);
        }
        let color = i32::from_str_radix(&s[..6], 16)?;
        let top = u8::from_str_radix(&s[6..8], 16)?;
        let left = u8::from_str_radix(&s[8..10], 16)?;
        Ok(LockfileChecksum { color, top, left })
    }
}

async fn day_23_lockfile(mut multipart: Multipart) -> (StatusCode, Body) {
    let mut body = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap().to_string();
        let data = field.bytes().await.unwrap();
        if name != "lockfile" {
            continue;
        }
        body.extend_from_slice(&data);
    }
    let body = String::from_utf8(body).unwrap();
    // toml
    match body.parse::<toml::Table>() {
        Ok(lock_toml) => {
            let mut response = String::new();
            match lock_toml.get("package").and_then(|item| item.as_array()) {
                Some(packages) => {
                    for package in packages {
                        if let Some(checksum_value) = package.get("checksum") {
                            if let Some(checksum) = checksum_value.as_str() {
                                match LockfileChecksum::from_str(checksum) {
                                    Ok(entry) => {
                                        response.push_str(&format!(
                                        r##"<div style="background-color:#{:06x};top:{}px;left:{}px;"></div>{}"##,
                                        entry.color, entry.top, entry.left, '\n'
                                    ));
                                    }
                                    Err(_) => {
                                        warn!("checksum parse error {}", checksum);
                                        return (StatusCode::UNPROCESSABLE_ENTITY, Body::empty());
                                    }
                                }
                            } else {
                                return (StatusCode::BAD_REQUEST, Body::empty());
                            }
                        }
                    }
                }
                None => {
                    return (StatusCode::BAD_REQUEST, Body::empty());
                }
            }
            (StatusCode::OK, Body::from(response))
        }
        Err(err) => {
            warn!("error parsing lockfile: {:?}", err);
            (StatusCode::BAD_REQUEST, Body::empty())
        }
    }
    // cargo_lock test #2 failed due to gimli dependency not found in lockfile
    // match Lockfile::from_str(&body) {
    //     Ok(lockfile) => {
    //         let mut response = String::new();
    //         for package in lockfile.packages {
    //             match package.checksum {
    //                 Some(checksum) => match LockfileChecksum::from_str(&checksum.to_string()) {
    //                     Ok(entry) => {
    //                         response.push_str(&format!(
    //                             r##"<div style="background-color:#{:x};top:{}px;left:{}px;"></div>"##,
    //                             entry.color, entry.top, entry.left
    //                         ));
    //                     }
    //                     Err(_) => {
    //                         println!("checksum parse error");
    //                         return Response::builder()
    //                             .status(StatusCode::UNPROCESSABLE_ENTITY)
    //                             .body(Body::empty())
    //                             .unwrap();
    //                     }
    //                 },
    //                 None => {
    //                     warn!("no checksum for package {}", &package.name);
    //                     // return Response::builder()
    //                     //     .status(StatusCode::UNPROCESSABLE_ENTITY)
    //                     //     .body(Body::empty())
    //                     //     .unwrap();
    //                 }
    //             }
    //         }
    //         Response::new(Body::from(response))
    //     }
    //     Err(err) => {
    //         warn!("error parsing lockfile: {:?}", err);
    //         Response::builder()
    //             .status(StatusCode::BAD_REQUEST)
    //             .body(Body::empty())
    //             .unwrap()
    //     }
    // }
}

// day 19

#[derive(Deserialize)]
struct QuotePost {
    author: String,
    quote: String,
}

struct Day19AppState {
    pool: PgPool,
    pages: Mutex<HashMap<String, i64>>,
}

#[derive(Deserialize, Serialize, FromRow)]
struct Quote {
    id: uuid::Uuid,
    author: String,
    quote: String,
    created_at: chrono::DateTime<chrono::Utc>,
    version: i32,
}

async fn create_database(pool: &PgPool) -> sqlx::Result<PgQueryResult> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS quotes (
        id UUID PRIMARY KEY,
        author TEXT NOT NULL,
        quote TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
        version INT NOT NULL DEFAULT 1
        );"#,
    )
    .execute(pool)
    .await
}

async fn day_19_reset(State(state): State<Arc<Day19AppState>>) -> impl IntoResponse {
    sqlx::query("DELETE FROM quotes")
        .execute(&state.pool)
        .await
        .unwrap();
    ""
}

async fn day_19_cite(
    State(state): State<Arc<Day19AppState>>,
    Path(id): Path<uuid::Uuid>,
) -> (StatusCode, Body) {
    match sqlx::query_as::<_, Quote>("SELECT * FROM quotes WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(quote) => (
            StatusCode::OK,
            Body::from(serde_json::to_string(&quote).unwrap()),
        ),
        Err(err) => {
            warn!("cite: error fetching quote with id {id}: {:?}", err);
            (StatusCode::NOT_FOUND, Body::empty())
        }
    }
}

async fn day_19_remove(
    State(state): State<Arc<Day19AppState>>,
    Path(id): Path<uuid::Uuid>,
) -> (StatusCode, Body) {
    match sqlx::query_as::<_, Quote>("DELETE FROM quotes WHERE id = $1 RETURNING *")
        .bind(id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(quote) => (
            StatusCode::OK,
            Body::from(serde_json::to_string(&quote).unwrap()),
        ),
        Err(err) => {
            if !matches!(err, sqlx::Error::RowNotFound) {
                warn!("Delete row err {:?}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, Body::empty())
            } else {
                (StatusCode::NOT_FOUND, Body::empty())
            }
        }
    }
}

async fn day_19_undo(
    Path(id): Path<uuid::Uuid>,
    State(state): State<Arc<Day19AppState>>,
    Json(quote_post): Json<QuotePost>,
) -> (StatusCode, Body) {
    match sqlx::query_as::<_, Quote>("SELECT * FROM quotes WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(mut quote) => {
            quote.version += 1;
            quote.author = quote_post.author;
            quote.quote = quote_post.quote;
            match sqlx::query(
                "UPDATE quotes SET version = $1, author = $2, quote = $3 WHERE id = $4",
            )
            .bind(quote.version)
            .bind(&quote.author)
            .bind(&quote.quote)
            .bind(id)
            .execute(&state.pool)
            .await
            {
                Ok(_) => (
                    StatusCode::OK,
                    Body::from(serde_json::to_string(&quote).unwrap()),
                ),
                Err(err) => {
                    warn!("error updating quote: {:?}", err);
                    (StatusCode::INTERNAL_SERVER_ERROR, Body::empty())
                }
            }
        }
        Err(err) => {
            warn!("undo: error fetching quote with id {id}: {:?}", err);
            (StatusCode::NOT_FOUND, Body::empty())
        }
    }
}

async fn day_19_draft(
    State(state): State<Arc<Day19AppState>>,
    Json(quote_post): Json<QuotePost>,
) -> (StatusCode, Body) {
    let quote = Quote {
        id: uuid::Uuid::new_v4(),
        author: quote_post.author,
        quote: quote_post.quote,
        created_at: chrono::Utc::now(),
        version: 1,
    };
    match sqlx::query("INSERT INTO quotes (id, author, quote, version) VALUES ($1, $2, $3, $4)")
        .bind(quote.id)
        .bind(&quote.author)
        .bind(&quote.quote)
        .bind(quote.version)
        .execute(&state.pool)
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Body::from(serde_json::to_string(&quote).unwrap()),
        ),
        Err(err) => {
            warn!(
                "draft: insert quote {} with author {} failed: err {:?}",
                quote.quote, quote.author, err
            );
            (StatusCode::INTERNAL_SERVER_ERROR, Body::empty())
        }
    }
}

#[derive(Serialize)]
struct QuotePage {
    quotes: Vec<Quote>,
    page: i64,
    next_token: Option<String>,
}

async fn day_19_list(
    State(state): State<Arc<Day19AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Body) {
    let mut tokens = state.pages.lock().await;
    let offset = match params.get("token") {
        Some(token) => match tokens.remove(token) {
            Some(offset) => offset,
            None => {
                return (StatusCode::BAD_REQUEST, Body::empty());
            }
        },
        None => 0,
    };

    match sqlx::query_as::<_, Quote>(
        "SELECT * FROM quotes ORDER BY created_at ASC LIMIT 3 OFFSET $1",
    )
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(quotes) => {
            let offset = offset + quotes.len() as i64;
            let total_cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quotes")
                .fetch_one(&state.pool)
                .await
                .unwrap();
            let next_token = if offset == total_cnt {
                None
            } else {
                let next_token: String = thread_rng()
                    .sample_iter(&Alphanumeric)
                    .take(16)
                    .map(char::from)
                    .collect();
                tokens.insert(next_token.clone(), offset);
                Some(next_token)
            };
            let quotes_page = QuotePage {
                quotes,
                page: (offset + 2) / 3,
                next_token,
            };
            (
                StatusCode::OK,
                Body::from(serde_json::to_string(&quotes_page).unwrap()),
            )
        }
        Err(err) => {
            warn!("list: error fetching quotes: {:?}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, Body::empty())
        }
    }
}

// day 16

const KEY: &[u8] = include_bytes!("../key/day16_santa_public_key.pem");

async fn day_16_decode(body: String) -> (StatusCode, Body) {
    match decode_header(&body) {
        Ok(header) => {
            let mut validation = Validation::new(header.alg);
            validation.required_spec_claims.clear();
            match decode::<Value>(&body, &DecodingKey::from_rsa_pem(KEY).unwrap(), &validation) {
                Ok(token) => (StatusCode::OK, Body::from(token.claims.to_string())),
                Err(err) => match err.kind() {
                    jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                        (StatusCode::UNAUTHORIZED, Body::empty())
                    }
                    _ => (StatusCode::BAD_REQUEST, Body::empty()),
                },
            }
        }
        Err(err) => {
            warn!("error decoding header: {:?}", err);
            (StatusCode::BAD_REQUEST, Body::empty())
        }
    }
}

async fn day_16_wrap(Json(body): Json<Value>) -> impl IntoResponse {
    let header = Header::new(Algorithm::HS256);
    let token = encode(&header, &body, &EncodingKey::from_secret("secret".as_ref()));

    [("set-cookie", format!("gift={}", token.unwrap()))]
}

async fn day_16_unwrap(headers: HeaderMap) -> (StatusCode, Body) {
    match headers
        .get("cookie")
        .and_then(|cookie| cookie.to_str().ok())
        .and_then(|cookie| cookie.strip_prefix("gift="))
    {
        Some(token) => {
            let parts = token.split('.').collect::<Vec<_>>();
            match BASE64_URL_SAFE_NO_PAD.decode(parts[1].as_bytes()) {
                Ok(body) => (StatusCode::OK, Body::from(body)),
                Err(err) => {
                    warn!("error decoding body: {:?}", err);
                    (StatusCode::BAD_REQUEST, Body::empty())
                }
            }
        }
        None => (StatusCode::BAD_REQUEST, Body::empty()),
    }
}

// day 12

struct Day12AppState {
    game: Mutex<Game>,
}

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

async fn day_12_random_board(State(state): State<Arc<Day12AppState>>) -> impl IntoResponse {
    let mut game = state.game.lock().await;
    for i in 0..4 {
        for j in 1..5 {
            let team = if game.rng.r#gen::<bool>() {
                GameItem::Cookie
            } else {
                GameItem::Milk
            };
            game.put_random_item(team, i, j);
        }
    }
    game.print_board()
}

async fn day_12_place(
    State(state): State<Arc<Day12AppState>>,
    Path((team, column)): Path<(String, i32)>,
) -> (StatusCode, Body) {
    let team = match team.as_str() {
        "cookie" => GameItem::Cookie,
        "milk" => GameItem::Milk,
        _ => {
            return (StatusCode::BAD_REQUEST, Body::empty());
        }
    };
    if !(1..=4).contains(&column) {
        return (StatusCode::BAD_REQUEST, Body::empty());
    }
    let column = column as usize;
    let mut game = state.game.lock().await;
    if game.is_column_full(column) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Body::from(game.print_board()),
        );
    }

    if game.is_finished() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Body::from(game.print_board()),
        );
    }
    game.put_item(team, column);
    (StatusCode::OK, Body::from(game.print_board()))
}

async fn day_12_board(State(state): State<Arc<Day12AppState>>) -> impl IntoResponse {
    state.game.lock().await.print_board()
}

async fn day_12_reset(State(state): State<Arc<Day12AppState>>) -> impl IntoResponse {
    let mut game = state.game.lock().await;
    game.reset();
    game.print_board()
}

// day 9

struct Day9AppState {
    limiter: Mutex<RateLimiter>,
}

fn day_9_init_rate_limiter() -> RateLimiter {
    RateLimiter::builder()
        .max(5)
        .initial(5)
        .interval(Duration::from_secs(1))
        .build()
}

fn day_9_bad_request() -> (StatusCode, Body) {
    (StatusCode::BAD_REQUEST, Body::empty())
}

async fn day_9_milk(
    State(state): State<Arc<Day9AppState>>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, Body) {
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
                            (StatusCode::OK, Body::from(data.dump()))
                        } else {
                            day_9_bad_request()
                        }
                    }
                    (None, Some(gallons), None, None) => {
                        if let Some(gallons) = gallons.as_f64() {
                            let liters = gallons * 3.78541253;
                            let mut data = json::JsonValue::new_object();
                            data["liters"] = liters.into();
                            (StatusCode::OK, Body::from(data.dump()))
                        } else {
                            day_9_bad_request()
                        }
                    }
                    (None, None, Some(litres), None) => {
                        if let Some(litres) = litres.as_f64() {
                            let pints = litres * 1.7598;
                            let mut data = json::JsonValue::new_object();
                            data["pints"] = pints.into();
                            (StatusCode::OK, Body::from(data.dump()))
                        } else {
                            day_9_bad_request()
                        }
                    }
                    (None, None, None, Some(pints)) => {
                        if let Some(pints) = pints.as_f64() {
                            let litres = pints / 1.7598;
                            let mut data = json::JsonValue::new_object();
                            data["litres"] = litres.into();
                            (StatusCode::OK, Body::from(data.dump()))
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
                (StatusCode::OK, Body::from("Milk withdrawn\n"))
            } else {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Body::from("No milk available\n"),
                )
            }
        }
    }
}

async fn day_9_refill(State(state): State<Arc<Day9AppState>>) -> impl IntoResponse {
    let mut limiter = state.limiter.lock().await;
    *limiter = day_9_init_rate_limiter();
    ""
}

// day 5

fn day_5_handle_toml(body: String) -> (StatusCode, Body) {
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
                        (StatusCode::OK, Body::from(orders.join("\n")))
                    }
                }
                _ => day_5_no_content_response(),
            }
        }
        Err(_) => day_5_invalid_manifest_response(),
    }
}

fn day_5_no_content_response() -> (StatusCode, Body) {
    (StatusCode::NO_CONTENT, Body::empty())
}

fn day_5_invalid_manifest_response() -> (StatusCode, Body) {
    (StatusCode::BAD_REQUEST, Body::from("Invalid manifest"))
}

fn day_5_magic_keyword_response() -> (StatusCode, Body) {
    (
        StatusCode::BAD_REQUEST,
        Body::from("Magic keyword not provided"),
    )
}

fn day_5_unsupported_media_type_response() -> (StatusCode, Body) {
    (StatusCode::UNSUPPORTED_MEDIA_TYPE, Body::empty())
}

async fn day_5_manifest(headers: HeaderMap, body: String) -> (StatusCode, Body) {
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

    dest.to_string()
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
    key.to_string()
}

#[derive(Deserialize)]
struct Day2V6DestQuery {
    from: Ipv6Addr,
    key: Ipv6Addr,
}

async fn day_2_v6_dest(query: Query<Day2V6DestQuery>) -> impl IntoResponse {
    let dest = Ipv6Addr::from_bits(query.from.to_bits() ^ query.key.to_bits());

    dest.to_string()
}

#[derive(Deserialize)]
struct Day2V6KeyQuery {
    from: Ipv6Addr,
    to: Ipv6Addr,
}

async fn day_2_v6_key(query: Query<Day2V6KeyQuery>) -> impl IntoResponse {
    let key = Ipv6Addr::from_bits(query.from.to_bits() ^ query.to.to_bits());

    key.to_string()
}

// day -1

async fn day_1_hello_world() -> &'static str {
    "Hello, bird!"
}

async fn day_1_seek() -> impl IntoResponse {
    (
        StatusCode::FOUND,
        [(LOCATION, "https://www.youtube.com/watch?v=9Gc4QTqslN4")],
    )
}
