use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{App, AppError};
use crate::files;

type S = State<Arc<App>>;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let code = match &self {
            AppError::NothingOpen => StatusCode::CONFLICT,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Refused(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (code, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/workspaces", get(workspaces))
        .route("/api/workspaces/open", post(open))
        .route("/api/workspaces/new", post(create))
        .route("/api/call/{method}", post(call))
        .route("/api/events", get(events))
        .route("/api/files", get(file_list))
        .route("/api/file", get(file))
        .with_state(app)
}

/// Working cells out can take a while on a big graph, so they don't run on the threads serving requests.
async fn blocking<T: Send + 'static>(app: Arc<App>, f: impl FnOnce(&Arc<App>) -> Result<T, AppError> + Send + 'static) -> Result<T, AppError> {
    tokio::task::spawn_blocking(move || f(&app)).await.map_err(|e| AppError::Other(e.into()))?
}

#[derive(Serialize)]
struct SampleInfo {
    name: &'static str,
    title: &'static str,
    about: &'static str,
    group: &'static str,
    systems: &'static [&'static str],
}

async fn workspaces(State(app): S) -> impl IntoResponse {
    let current = app.with(|o| Ok(json!({ "name": o.ws.config.name, "id": o.ws.id, "location": o.ws.location, "sample": o.ws.sample }))).ok();
    let samples: Vec<SampleInfo> = app.sample_list().iter().map(|s| SampleInfo { name: s.name, title: s.title, about: s.about, group: s.group, systems: s.systems }).collect();
    Json(json!({ "current": current, "samples": samples, "home": app.home }))
}

#[derive(Deserialize)]
struct OpenBody {
    sample: Option<String>,
    path: Option<PathBuf>,
    #[serde(default)]
    reset: bool,
}

async fn open(State(app): S, Json(body): Json<OpenBody>) -> Result<StatusCode, AppError> {
    blocking(app, move |app| match (body.sample, body.path) {
        (Some(s), _) => app.open_sample(&s, body.reset),
        (None, Some(p)) => app.open_dir(&p, None),
        (None, None) => Err(AppError::Refused("say which workspace to open".to_string())),
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewBody {
    path: PathBuf,
    name: String,
}

async fn create(State(app): S, Json(body): Json<NewBody>) -> Result<StatusCode, AppError> {
    blocking(app, move |app| app.create(&body.path, &body.name)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Anything `recollect_core::dispatch` answers, by name: the same calls the browser's engine takes.
async fn call(State(app): S, Path(method): Path<String>, body: Option<Json<Value>>) -> Result<impl IntoResponse, AppError> {
    let args = body.map_or(Value::Null, |b| b.0);
    let value = blocking(app, move |a| a.call(&method, args, "you")).await?;
    Ok(Json(value))
}

async fn file_list(State(app): S) -> Result<impl IntoResponse, AppError> {
    let list = blocking(app, |a| {
        let dir = a.with(|o| Ok(o.dir.clone()))?;
        Ok(files::list(&dir)?)
    });
    Ok(Json(list.await?))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

async fn file(State(app): S, Query(q): Query<FileQuery>) -> Result<impl IntoResponse, AppError> {
    let text = blocking(app, move |a| {
        let dir = a.with(|o| Ok(o.dir.clone()))?;
        files::read(&dir, &q.path)
    });
    Ok(Json(text.await?))
}

async fn events(State(app): S, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| forward_events(socket, app))
}

async fn forward_events(mut socket: WebSocket, app: Arc<App>) {
    let mut rx = app.events.subscribe();
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let text = match ev {
                    Ok(ev) => serde_json::to_string(&ev).expect("events serialise"),
                    // Missed events: tell the client to look again.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => r#"{"type":"resync"}"#.into(),
                    Err(_) => break,
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                if !matches!(msg, Some(Ok(_))) {
                    break;
                }
            }
        }
    }
}

/// The page, saying it's served by this, so it talks to this rather than running the engine itself.
pub fn index_html(dist: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(dist.join("index.html")).ok()?;
    Some(text.replacen("<head>", "<head>\n    <meta name=\"recollect-backend\" content=\"http\" />", 1))
}
