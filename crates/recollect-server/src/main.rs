mod api;
mod app;
mod files;
mod mcp;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::response::{Html, IntoResponse};
use clap::{Parser, Subcommand};
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager};
use tower_http::services::ServeDir;

use crate::app::App;

#[derive(Parser)]
#[command(name = "recollect", about = "A structured graph of ideas, with an LLM in the loop")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Make a new workspace directory.
    Init {
        dir: PathBuf,
        /// What to call it. Defaults to the directory's name.
        #[arg(long)]
        name: Option<String>,
        /// Which systems it uses.
        #[arg(long, value_delimiter = ',', default_value = "plan,schedule")]
        systems: Vec<String>,
    },
    /// Serve the web UI, REST API and MCP endpoint.
    Serve {
        /// Open this workspace. Otherwise whatever was open last, or a choice of samples.
        #[arg(long, short)]
        workspace: Option<PathBuf>,
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Built web UI to serve (default: web/dist in the source tree).
        #[arg(long)]
        web: Option<PathBuf>,
    },
}

fn source_tree(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
}

/// Where this keeps its own things, such as working copies of samples.
fn home() -> PathBuf {
    std::env::var_os("RECOLLECT_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".recollect"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match Cli::parse().cmd {
        Cmd::Init { dir, name, systems } => {
            let name = name.or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "Untitled".to_string());
            app::init(&dir, &name, systems).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("workspace ready at {}", dir.display());
            Ok(())
        }
        Cmd::Serve { workspace, port, web } => serve(workspace, port, web).await,
    }
}

async fn serve(workspace: Option<PathBuf>, port: u16, web: Option<PathBuf>) -> Result<()> {
    let app = App::new(home());
    match workspace {
        Some(dir) => app.open_dir(&dir, None).map_err(|e| anyhow::anyhow!("{e}"))?,
        None => app.reopen_last(),
    }

    let mcp_app = app.clone();
    let mcp = StreamableHttpService::new(
        move || Ok(mcp::RecollectMcp::new(mcp_app.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let dist = web.unwrap_or_else(|| source_tree("web/dist"));
    if !dist.join("index.html").exists() {
        tracing::warn!("no built UI at {} (run `npm run build` in web/, or use the Vite dev server)", dist.display());
    }
    // The page is read each time, so one from an older build isn't kept after a rebuild, and it says
    // it's served by this. Everything else is a file from the build.
    let page_dir = dist.clone();
    let page = move || {
        let dist = page_dir.clone();
        async move {
            let mut res = match api::index_html(&dist) {
                Some(html) => Html(html).into_response(),
                None => (axum::http::StatusCode::NOT_FOUND, "no built UI: run `npm run build` in web/").into_response(),
            };
            res.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-cache"));
            res
        }
    };
    let assets = tower_http::set_header::SetResponseHeader::overriding(
        ServeDir::new(&dist).append_index_html_on_directories(false).fallback(axum::routing::get(page.clone())),
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );

    let router = api::router(app.clone()).route("/", axum::routing::get(page.clone())).route("/index.html", axum::routing::get(page)).nest_service("/mcp", mcp).fallback_service(assets);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("UI http://{addr}/   MCP http://{addr}/mcp");
    axum::serve(listener, router).await?;
    Ok(())
}
