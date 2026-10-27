//! Tool host (in the app) and bridge (in the CLI's child process): the drawing
//! tools served over MCP to the CLI, through a localhost socket.

use super::*;

/// One turn's wiring into the host: where events go, what the tools see.
#[derive(Clone)]
pub(super) struct Link {
	pub(super) tx: Sender<AgentEvent>,
	pub(super) session: Arc<Mutex<ChatSession>>,
	pub(super) cancel: Arc<AtomicBool>,
	pub(super) current_page: u32,
	/// Model id the coordinate dialect is chosen for.
	pub(super) model: String,
	pub(super) sweeps: Option<ProviderConfig>,
	/// Assistant text streamed since the last tool call (recorded before the call).
	pub(super) pending: Arc<Mutex<String>>,
	/// Where overlay files are read and written.
	pub(super) workspace: Option<PathBuf>,
	pub(super) others: Vec<tools::OtherDrawing>,
}

pub(super) struct Shared {
	pub(super) doc: Arc<Mutex<Document>>,
	pub(super) pdf: PathBuf,
	pub(super) token: String,
	/// Kept across turns so view ids from earlier turns stay valid.
	pub(super) ctx: Mutex<Option<ToolCtx>>,
	pub(super) link: Mutex<Option<Link>>,
	pub(super) stop: AtomicBool,
	pub(super) calls: AtomicU64,
}

/// Serves the drawing tools for ONE open document on 127.0.0.1, to bridges
/// that present the right token.
pub struct ToolHost {
	pub(super) addr: SocketAddr,
	pub(super) shared: Arc<Shared>,
}

pub(super) fn random_token() -> String {
	let mut s = String::new();
	for i in 0..2u64 {
		let mut h = std::collections::hash_map::RandomState::new().build_hasher();
		h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
		h.write_u64(i ^ std::process::id() as u64);
		s.push_str(&format!("{:016x}", h.finish()));
	}
	s
}

impl ToolHost {
	pub fn start(doc: Arc<Mutex<Document>>, pdf: PathBuf) -> Result<Self, String> {
		let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("could not open the tool port: {e}"))?;
		let addr = listener.local_addr().map_err(|e| e.to_string())?;
		let shared = Arc::new(Shared { doc, pdf, token: random_token(), ctx: Mutex::new(None), link: Mutex::new(None), stop: AtomicBool::new(false), calls: AtomicU64::new(0) });
		let sh = shared.clone();
		std::thread::Builder::new()
			.name("takeoff-tool-host".into())
			.spawn(move || {
				for stream in listener.incoming() {
					if sh.stop.load(Ordering::Relaxed) {
						break;
					}
					if let Ok(s) = stream {
						let sh = sh.clone();
						let _ = std::thread::Builder::new().name("takeoff-tool-conn".into()).spawn(move || sh.serve(s));
					}
				}
			})
			.map_err(|e| e.to_string())?;
		Ok(Self { addr, shared })
	}

	/// Whether this host works on `doc` (a reopened file gets a new document).
	pub fn serves(&self, doc: &Arc<Mutex<Document>>) -> bool {
		Arc::ptr_eq(&self.shared.doc, doc)
	}
}

impl Drop for ToolHost {
	fn drop(&mut self) {
		self.shared.stop.store(true, Ordering::Relaxed);
		// Wake the accept loop so it sees the flag.
		let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
	}
}

pub(super) fn attach(ctx: &mut ToolCtx, link: &Link, doc: &Mutex<Document>) {
	ctx.events = Some(link.tx.clone());
	ctx.cancel = Some(link.cancel.clone());
	ctx.current_page = link.current_page;
	ctx.llm = link.sweeps.clone();
	ctx.model_id = link.model.clone();
	ctx.dialect = takeoff_core::coords::dialect_for(&link.model);
	ctx.workspace = link.workspace.clone();
	// Back on this chat's drawing, with this turn's set of open tabs.
	let _ = ctx.set_others(link.others.clone());
	let hidden: HashSet<String> = doc.lock().data.hidden_layers.iter().cloned().collect();
	let _ = ctx.drawing.set_hidden_layers(&hidden);
}

impl Shared {
	pub(super) fn begin(&self, link: Link) {
		if let Some(ctx) = self.ctx.lock().as_mut() {
			attach(ctx, &link, &self.doc);
		}
		*self.link.lock() = Some(link);
	}

	pub(super) fn end(&self) {
		*self.link.lock() = None;
	}

	pub(super) fn serve(&self, stream: TcpStream) {
		let Ok(read) = stream.try_clone() else { return };
		let mut reader = BufReader::new(read);
		let mut writer = stream;
		let mut first = String::new();
		if reader.read_line(&mut first).is_err() || first.trim() != self.token {
			return;
		}
		for line in reader.lines() {
			let Ok(line) = line else { break };
			if line.trim().is_empty() {
				continue;
			}
			let reply = match serde_json::from_str::<Value>(&line) {
				Ok(msg) => mcp::reply(&msg, || self.list(), |name, args| self.call(name, args)),
				Err(e) => Some(mcp::error(&Value::Null, -32700, &format!("parse error: {e}"))),
			};
			if let Some(r) = reply {
				if writeln!(writer, "{r}").and_then(|_| writer.flush()).is_err() {
					break;
				}
			}
		}
	}

	pub(super) fn list(&self) -> Value {
		let sweeps = self.link.lock().as_ref().is_some_and(|l| l.sweeps.is_some());
		// The CLIs bring their own shell; ours would only duplicate it.
		let mut skip: Vec<&str> = tools::SHELL_TOOLS.to_vec();
		if !sweeps {
			skip.extend(SWEEPS);
		}
		mcp::tool_list(vec![], &skip)
	}

	pub(super) fn call(&self, name: &str, args: Value) -> Value {
		let Some(link) = self.link.lock().clone() else { return mcp::text_result("The Takeoff Desk turn has ended; nothing was changed.", true) };
		if link.cancel.load(Ordering::Relaxed) {
			return mcp::text_result("Cancelled by the user.", true);
		}
		let mut guard = self.ctx.lock();
		if guard.is_none() {
			match takeoff_pdf::Drawing::open(&self.pdf) {
				Ok(d) => {
					let mut ctx = ToolCtx::new(self.doc.clone(), d, &link.model, None);
					attach(&mut ctx, &link, &self.doc);
					*guard = Some(ctx);
				}
				Err(e) => return mcp::text_result(format!("could not open the drawing: {e}"), true),
			}
		}
		let ctx = guard.as_mut().expect("tool context");
		let call_id = format!("{SERVER}_{}", self.calls.fetch_add(1, Ordering::Relaxed) + 1);
		let text_before = std::mem::take(&mut *link.pending.lock());
		link.session.lock().messages.push(Msg::Assistant { text: text_before, reasoning: String::new(), tool_calls: vec![ToolCall { id: call_id.clone(), name: name.to_string(), arguments: args.to_string() }] });
		let _ = link.tx.send(AgentEvent::ToolStarted { call_id: call_id.clone(), name: name.to_string(), args: args.clone() });
		let (ok, text, images, result) = match tools::call(ctx, name, args) {
			Ok(mut out) => {
				clip_tool_text(&mut out.text);
				let r = mcp::output_result(&out);
				(true, out.text, out.images, r)
			}
			Err(e) => (false, format!("Error: {e}"), vec![], mcp::text_result(e, true)),
		};
		let first_thumb = images.first().and_then(|i| thumb(&i.png));
		let summary = text.lines().next().unwrap_or("").chars().take(300).collect::<String>();
		let _ = link.tx.send(AgentEvent::ToolFinished { call_id: call_id.clone(), name: name.to_string(), ok, summary, image: first_thumb.as_ref().map(|t| t.0.clone()) });
		link.session.lock().messages.push(Msg::Tool {
			call_id,
			name: name.to_string(),
			text,
			ok,
			// The CLI holds the renders; keep captions so a later model knows they existed.
			images: vec![],
			image_captions: images.iter().map(|i| i.caption.clone()).collect(),
			thumb: first_thumb.map(|t| Arc::new(t.1)),
		});
		result
	}
}

/// Body of `takeoff-desk --mcp-bridge <addr>`: pipe stdio to the app's tool
/// host. Returns the process exit code.
pub fn run_bridge(addr: &str) -> i32 {
	let token = std::env::var(TOKEN_ENV).unwrap_or_default();
	let Ok(mut stream) = TcpStream::connect(addr) else {
		eprintln!("takeoff bridge: Takeoff Desk is not listening on {addr}");
		return 1;
	};
	if writeln!(stream, "{token}").is_err() {
		return 1;
	}
	let Ok(mut up) = stream.try_clone() else { return 1 };
	std::thread::spawn(move || {
		let _ = std::io::copy(&mut std::io::stdin().lock(), &mut up);
		let _ = up.shutdown(Shutdown::Write);
	});
	// Stdout is line-buffered, so each JSON-RPC reply goes out as it arrives.
	let _ = std::io::copy(&mut stream, &mut std::io::stdout().lock());
	0
}
