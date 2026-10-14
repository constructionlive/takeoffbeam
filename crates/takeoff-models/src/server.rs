//! Run a local model with llama.cpp's `llama-server` (OpenAI-compatible API
//! on 127.0.0.1). The server runs in its own process group so stopping it —
//! or dropping the handle, or the app quitting — never leaves it behind.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::fit::CacheType;
use crate::store::InstalledModel;

/// Where `llama-server` is: `$TAKEOFF_LLAMA_SERVER`, next to the executable,
/// the app bundle's Resources/bin, Homebrew, then `PATH`.
pub fn find_llama_server() -> Option<PathBuf> {
	let exe_name = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
	let mut candidates: Vec<PathBuf> = Vec::new();
	if let Ok(p) = std::env::var("TAKEOFF_LLAMA_SERVER") {
		candidates.push(PathBuf::from(p));
	}
	if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
		candidates.push(dir.join(exe_name));
		candidates.push(dir.join("bin").join(exe_name));
		candidates.push(dir.join("../Resources/bin").join(exe_name));
		candidates.push(dir.join("../Resources").join(exe_name));
	}
	candidates.push(PathBuf::from("/opt/homebrew/bin").join(exe_name));
	candidates.push(PathBuf::from("/usr/local/bin").join(exe_name));
	if let Some(path) = std::env::var_os("PATH") {
		candidates.extend(std::env::split_paths(&path).map(|d| d.join(exe_name)));
	}
	candidates.into_iter().find(|p| p.is_file())
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerState {
	/// Process up, model still loading.
	Starting,
	Ready,
	/// Exited or never became healthy; the message carries the log tail.
	Failed(String),
	Stopped,
}

/// Extra knobs; `LlamaServer::start` uses the defaults.
#[derive(Clone, Debug)]
pub struct ServerOptions {
	/// Let the model "think" before answering. Off by default: thinking makes
	/// every tool step slower for little takeoff gain on local models.
	pub reasoning: bool,
	/// Parallel request slots (the agent loop is sequential).
	pub slots: u32,
}

impl Default for ServerOptions {
	fn default() -> Self {
		Self { reasoning: false, slots: 1 }
	}
}

const LOG_LINES: usize = 400;

pub struct LlamaServer {
	child: Option<Child>,
	port: u16,
	model_id: String,
	log: Arc<Mutex<VecDeque<String>>>,
	state: Arc<Mutex<ServerState>>,
	started: Instant,
}

fn free_port() -> Result<u16, String> {
	let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
	Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

impl LlamaServer {
	pub fn start(model: &InstalledModel, ctx_tokens: u32, cache: CacheType) -> Result<Self, String> {
		Self::start_with(model, ctx_tokens, cache, &ServerOptions::default())
	}

	pub fn start_with(model: &InstalledModel, ctx_tokens: u32, cache: CacheType, opts: &ServerOptions) -> Result<Self, String> {
		let bin = find_llama_server().ok_or("llama-server was not found. Install llama.cpp (brew install llama.cpp) or set TAKEOFF_LLAMA_SERVER.")?;
		let port = free_port()?;
		let mut cmd = Command::new(&bin);
		cmd.arg("-m").arg(&model.model_path);
		if let Some(mm) = &model.mmproj_path {
			cmd.arg("--mmproj").arg(mm);
		}
		cmd.args(["--host", "127.0.0.1", "--port", &port.to_string()])
			.args(["-c", &ctx_tokens.to_string()])
			.args(["-np", &opts.slots.max(1).to_string()])
			.args(["-a", &model.id])
			.arg("--jinja")
			.arg("--no-webui")
			.args(["--reasoning", if opts.reasoning { "on" } else { "off" }]);
		let kv = match cache {
			CacheType::F16 => None,
			CacheType::Q8_0 => Some("q8_0"),
			CacheType::Q4_0 => Some("q4_0"),
		};
		if let Some(t) = kv {
			cmd.args(["-ctk", t, "-ctv", t]);
		}
		cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
		#[cfg(unix)]
		{
			use std::os::unix::process::CommandExt;
			cmd.process_group(0);
		}
		let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", bin.display()))?;
		let log = Arc::new(Mutex::new(VecDeque::with_capacity(LOG_LINES)));
		for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
			let log = log.clone();
			std::thread::spawn(move || {
				for line in BufReader::new(stream).lines().map_while(Result::ok) {
					let mut g = log.lock();
					if g.len() >= LOG_LINES {
						g.pop_front();
					}
					g.push_back(line);
				}
			});
		}
		Ok(Self { child: Some(child), port, model_id: model.id.clone(), log, state: Arc::new(Mutex::new(ServerState::Starting)), started: Instant::now() })
	}

	pub fn port(&self) -> u16 {
		self.port
	}

	/// OpenAI-compatible base URL (POST `{base}/chat/completions`).
	pub fn base_url(&self) -> String {
		format!("http://127.0.0.1:{}/v1", self.port)
	}

	/// The model name to send in requests (the server alias).
	pub fn model_id(&self) -> String {
		self.model_id.clone()
	}

	pub fn uptime(&self) -> Duration {
		self.started.elapsed()
	}

	pub fn log_tail(&self, n: usize) -> Vec<String> {
		let g = self.log.lock();
		g.iter().skip(g.len().saturating_sub(n)).cloned().collect()
	}

	/// Current state: checks the process and `GET /health` (cheap; call per frame is fine
	/// once Ready since it is cached, but throttle while Starting).
	pub fn state(&mut self) -> ServerState {
		let current = self.state.lock().clone();
		if matches!(current, ServerState::Failed(_) | ServerState::Stopped) {
			return current;
		}
		if let Some(child) = &mut self.child {
			if let Ok(Some(status)) = child.try_wait() {
				let tail = self.log_tail(12).join("\n");
				let s = ServerState::Failed(format!("llama-server exited ({status}).\n{tail}"));
				*self.state.lock() = s.clone();
				self.child = None;
				return s;
			}
		}
		if current == ServerState::Ready {
			return current;
		}
		let healthy = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], self.port).into(), Duration::from_millis(200)).is_ok()
			&& health_ok(self.port);
		if healthy {
			*self.state.lock() = ServerState::Ready;
			return ServerState::Ready;
		}
		ServerState::Starting
	}

	/// Block until Ready or Failed, or `timeout` passes.
	pub fn wait_ready(&mut self, timeout: Duration) -> ServerState {
		let deadline = Instant::now() + timeout;
		loop {
			let s = self.state();
			if s != ServerState::Starting || Instant::now() >= deadline {
				return s;
			}
			std::thread::sleep(Duration::from_millis(250));
		}
	}

	pub fn stop(&mut self) {
		if let Some(mut child) = self.child.take() {
			terminate(&mut child);
		}
		*self.state.lock() = ServerState::Stopped;
	}
}

fn health_ok(port: u16) -> bool {
	let c = reqwest::blocking::Client::builder().timeout(Duration::from_millis(800)).build();
	let Ok(c) = c else { return false };
	c.get(format!("http://127.0.0.1:{port}/health")).send().map(|r| r.status().is_success()).unwrap_or(false)
}

#[cfg(unix)]
fn terminate(child: &mut Child) {
	let pgid = child.id() as i32;
	// SAFETY: plain signal syscalls on the process group we created.
	unsafe {
		libc::killpg(pgid, libc::SIGTERM);
	}
	let deadline = Instant::now() + Duration::from_secs(5);
	while Instant::now() < deadline {
		if let Ok(Some(_)) = child.try_wait() {
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	unsafe {
		libc::killpg(pgid, libc::SIGKILL);
	}
	let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate(child: &mut Child) {
	let _ = child.kill();
	let _ = child.wait();
}

impl Drop for LlamaServer {
	fn drop(&mut self) {
		self.stop();
	}
}
