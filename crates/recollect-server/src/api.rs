use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use recollect_graph::Entity;
use recollect_systems::{ALWAYS, SYSTEMS};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{App, AppError};
use crate::files;
use crate::views::{self, World};

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
        .route("/api/view", post(view))
        .route("/api/plan", post(plan))
        .route("/api/ops", post(ops))
        .route("/api/undo", post(undo))
        .route("/api/checkout", post(checkout))
        .route("/api/history", get(history))
        .route("/api/proposals", get(proposals))
        .route("/api/proposals/{id}/accept", post(accept))
        .route("/api/proposals/{id}/reject", post(reject))
        .route("/api/events", get(events))
        .route("/api/try", post(try_formula))
        .route("/api/sweep", post(sweep))
        .route("/api/rows", post(rows))
        .route("/api/files", get(file_list))
        .route("/api/file", get(file))
        .with_state(app)
}

/// Working cells out can take a while on a big graph, so they don't run on the threads serving requests.
async fn blocking<T: Send + 'static>(app: Arc<App>, f: impl FnOnce(&App) -> Result<T, AppError> + Send + 'static) -> Result<T, AppError> {
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
    let current = app.with(|o| Ok(json!({ "name": o.config.name, "dir": o.dir, "sample": o.sample }))).ok();
    let samples: Vec<SampleInfo> = app.sample_list().into_iter().map(|s| SampleInfo { name: s.name, title: s.title, about: s.about, group: s.group, systems: s.systems }).collect();
    let systems: Vec<Value> = SYSTEMS.iter().filter(|s| !ALWAYS.contains(&s.name)).map(|s| json!({ "name": s.name, "about": s.about })).collect();
    Json(json!({ "current": current, "samples": samples, "systems": systems, "home": app.home }))
}

#[derive(Deserialize)]
struct OpenBody {
    sample: Option<String>,
    path: Option<PathBuf>,
    #[serde(default)]
    reset: bool,
}

async fn open(State(app): S, Json(body): Json<OpenBody>) -> Result<StatusCode, AppError> {
    let result = tokio::task::spawn_blocking(move || match (body.sample, body.path) {
        (Some(s), _) => app.open_sample(&s, body.reset),
        (None, Some(p)) => app.open_dir(&p, None),
        (None, None) => Err(AppError::Refused("say which workspace to open".to_string())),
    });
    result.await.map_err(|e| AppError::Other(e.into()))??;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewBody {
    path: PathBuf,
    name: String,
    #[serde(default)]
    systems: Vec<String>,
}

async fn create(State(app): S, Json(body): Json<NewBody>) -> Result<StatusCode, AppError> {
    let result = tokio::task::spawn_blocking(move || app.create(&body.path, &body.name, body.systems));
    result.await.map_err(|e| AppError::Other(e.into()))??;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
struct ViewBody {
    #[serde(default)]
    world: World,
}

async fn view(State(app): S, Json(body): Json<ViewBody>) -> Result<impl IntoResponse, AppError> {
    let v = blocking(app, move |a| a.with(|o| Ok(views::view(o, &body.world)))).await?;
    Ok(Json(v))
}

async fn plan(State(app): S, Json(body): Json<ViewBody>) -> Result<impl IntoResponse, AppError> {
    let v = blocking(app, move |a| a.with(|o| views::planning(o, &body.world).map_err(AppError::Refused))).await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct TryBody {
    node: Entity,
    text: String,
    #[serde(default)]
    world: World,
}

async fn try_formula(State(app): S, Json(body): Json<TryBody>) -> Result<impl IntoResponse, AppError> {
    let tried = blocking(app, move |a| a.with(|o| Ok(views::try_formula(o, body.node, &body.text, &body.world)))).await?;
    Ok(Json(tried))
}

#[derive(Deserialize)]
struct SweepBody {
    set: views::CellRef,
    values: Vec<f64>,
    read: Vec<views::CellRef>,
    #[serde(default)]
    world: World,
}

/// The most values one sweep can try, which is plenty for a chart.
const MOST_SWEPT: usize = 200;

async fn sweep(State(app): S, Json(body): Json<SweepBody>) -> Result<impl IntoResponse, AppError> {
    if body.values.len() > MOST_SWEPT {
        return Err(AppError::Refused(format!("{} values is more than the {MOST_SWEPT} a sweep can try", body.values.len())));
    }
    let swept = blocking(app, move |a| a.with(|o| views::sweep(o, &body.set, &body.values, &body.read, &body.world).map_err(AppError::Refused))).await?;
    Ok(Json(swept))
}

#[derive(Deserialize)]
struct RowsBody {
    table: String,
    from: i64,
    to: i64,
    #[serde(default)]
    world: World,
}

/// The most rows one ask can have.
const MOST_ROWS: i64 = 1000;

async fn rows(State(app): S, Json(body): Json<RowsBody>) -> Result<impl IntoResponse, AppError> {
    if body.to < body.from || body.to - body.from >= MOST_ROWS {
        return Err(AppError::Refused(format!("rows {} to {} isn't a run of up to {MOST_ROWS}", body.from, body.to)));
    }
    let rows = blocking(app, move |a| a.with(|o| Ok(views::rows(o, &body.table, body.from, body.to, &body.world)))).await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct OpsBody {
    ops: Vec<Value>,
    #[serde(default)]
    why: String,
}

async fn ops(State(app): S, Json(body): Json<OpsBody>) -> Result<impl IntoResponse, AppError> {
    let resolved = blocking(app, move |a| a.apply(body.ops, "you", &body.why)).await?;
    Ok(Json(resolved))
}

#[derive(Deserialize)]
struct UndoBody {
    seq: u64,
    #[serde(default)]
    why: String,
}

async fn undo(State(app): S, Json(body): Json<UndoBody>) -> Result<impl IntoResponse, AppError> {
    let seq = blocking(app, move |a| a.undo(body.seq, "you", &body.why)).await?;
    Ok(Json(json!({ "seq": seq })))
}

#[derive(Deserialize)]
struct CheckoutBody {
    seq: u64,
    #[serde(default)]
    why: String,
}

async fn checkout(State(app): S, Json(body): Json<CheckoutBody>) -> Result<StatusCode, AppError> {
    blocking(app, move |a| a.checkout(body.seq, "you", &body.why)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct HistoryQuery {
    node: Option<Entity>,
}

async fn history(State(app): S, Query(q): Query<HistoryQuery>) -> Result<impl IntoResponse, AppError> {
    Ok(Json(app.with(|o| Ok(views::history(o, q.node)))?))
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

async fn proposals(State(app): S) -> Result<impl IntoResponse, AppError> {
    let list = blocking(app, |a| a.proposals()).await?;
    let list: Vec<Value> = list
        .into_iter()
        .map(|p| {
            let ops: Vec<views::OpView> = p.proposal.ops.iter().map(views::op_view).collect();
            json!({ "id": p.proposal.id, "title": p.proposal.title, "rationale": p.proposal.rationale, "author": p.proposal.author, "created": p.proposal.created, "error": p.error, "ops": ops })
        })
        .collect();
    Ok(Json(list))
}

#[derive(Deserialize, Default)]
struct AcceptBody {
    /// Which ops to apply, by index. All of them if absent.
    ops: Option<Vec<usize>>,
}

async fn accept(State(app): S, Path(id): Path<String>, body: Option<Json<AcceptBody>>) -> Result<impl IntoResponse, AppError> {
    let body = body.map(|b| b.0).unwrap_or_default();
    let seq = blocking(app, move |a| a.accept(&id, body.ops.as_deref())).await?;
    Ok(Json(json!({ "seq": seq })))
}

async fn reject(State(app): S, Path(id): Path<String>) -> Result<impl IntoResponse, AppError> {
    app.reject(&id)?;
    Ok(StatusCode::NO_CONTENT)
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
