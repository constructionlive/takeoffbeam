//! Minimal lazy PDF object reader ("COS" layer), just enough to read the
//! document catalog and its optional-content (layer) dictionaries without
//! loading the file: xref tables and xref streams (with PNG predictors),
//! compressed object streams, and `/Prev` chains. Memory-mapped, so a 700 MB
//! drawing set costs a few page faults.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use memmap2::Mmap;

#[derive(Debug, Clone, PartialEq)]
pub enum Obj {
	Null,
	Bool(bool),
	Int(i64),
	Real(f64),
	Name(Vec<u8>),
	Str(Vec<u8>),
	Array(Vec<Obj>),
	Dict(Dict),
	Ref(u32, u16),
	/// A stream: its dictionary and the byte range of its (encoded) data.
	Stream(Dict, std::ops::Range<usize>),
}

pub type Dict = BTreeMap<Vec<u8>, Obj>;

impl Obj {
	pub fn as_dict(&self) -> Option<&Dict> {
		match self {
			Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
			_ => None,
		}
	}
	pub fn as_array(&self) -> Option<&[Obj]> {
		match self {
			Obj::Array(a) => Some(a),
			_ => None,
		}
	}
	pub fn as_int(&self) -> Option<i64> {
		match self {
			Obj::Int(i) => Some(*i),
			Obj::Real(r) => Some(*r as i64),
			_ => None,
		}
	}
	pub fn as_name(&self) -> Option<&[u8]> {
		match self {
			Obj::Name(n) => Some(n),
			_ => None,
		}
	}
	pub fn as_ref(&self) -> Option<(u32, u16)> {
		match self {
			Obj::Ref(n, g) => Some((*n, *g)),
			_ => None,
		}
	}
}

pub fn key<'a>(d: &'a Dict, k: &str) -> Option<&'a Obj> {
	d.get(k.as_bytes())
}

#[derive(Debug, thiserror::Error)]
pub enum CosError {
	#[error("io: {0}")]
	Io(#[from] std::io::Error),
	#[error("pdf syntax at byte {0}: {1}")]
	Syntax(usize, &'static str),
	#[error("encrypted PDFs are not supported for layer edits")]
	Encrypted,
}

#[derive(Debug, Clone, Copy)]
enum XrefEntry {
	Offset(usize),
	InStream(u32, u32),
	Free,
}

pub struct CosFile {
	data: Arc<Mmap>,
	xref: HashMap<u32, XrefEntry>,
	pub trailer: Dict,
	/// Offset of the newest xref section (`startxref`).
	pub startxref: usize,
	/// The newest section is an xref stream (write updates the same way).
	pub xref_is_stream: bool,
	objstm_cache: std::cell::RefCell<HashMap<u32, Arc<Vec<(u32, Obj)>>>>,
}

impl CosFile {
	pub fn open(path: &Path) -> Result<Self, CosError> {
		let file = File::open(path)?;
		// SAFETY: the file is opened read-only; a concurrent truncation by another
		// process would fault, which is the usual mmap caveat for a viewer.
		let data = Arc::new(unsafe { Mmap::map(&file)? });
		Self::from_mmap(data)
	}

	pub fn bytes(&self) -> &[u8] {
		&self.data
	}

	pub fn mmap(&self) -> Arc<Mmap> {
		self.data.clone()
	}

	fn from_mmap(data: Arc<Mmap>) -> Result<Self, CosError> {
		let buf: &[u8] = &data;
		let tail_start = buf.len().saturating_sub(2048);
		let tail = &buf[tail_start..];
		let pos = rfind(tail, b"startxref").ok_or(CosError::Syntax(buf.len(), "no startxref"))? + tail_start;
		let mut p = Parser { buf, pos: pos + 9 };
		let startxref = p.int()? as usize;
		let mut file = CosFile {
			data: data.clone(),
			xref: HashMap::new(),
			trailer: Dict::new(),
			startxref,
			xref_is_stream: false,
			objstm_cache: Default::default(),
		};
		let mut next = Some(startxref);
		let mut seen = std::collections::HashSet::new();
		let mut first = true;
		while let Some(off) = next.take() {
			if !seen.insert(off) || off >= buf.len() {
				break;
			}
			let (trailer, is_stream) = file.read_section(off)?;
			if first {
				file.trailer = trailer.clone();
				file.xref_is_stream = is_stream;
				first = false;
			}
			// Hybrid files: a classic trailer may point at an xref stream too.
			if let Some(stm) = key(&trailer, "XRefStm").and_then(Obj::as_int) {
				if stm >= 0 && seen.insert(stm as usize) {
					file.read_section(stm as usize)?;
				}
			}
			next = key(&trailer, "Prev").and_then(Obj::as_int).map(|v| v as usize);
		}
		if key(&file.trailer, "Encrypt").is_some() {
			return Err(CosError::Encrypted);
		}
		Ok(file)
	}

	/// Reads one xref section at `off`; entries already known (newer) win.
	fn read_section(&mut self, off: usize) -> Result<(Dict, bool), CosError> {
		let buf: &[u8] = &self.data.clone();
		if off >= buf.len() {
			return Err(CosError::Syntax(off, "xref offset"));
		}
		let mut p = Parser { buf, pos: off };
		p.ws();
		if p.rest().starts_with(b"xref") {
			p.pos += 4;
			loop {
				p.ws();
				if p.rest().starts_with(b"trailer") {
					p.pos += 7;
					let trailer = p.object()?.as_dict().cloned().ok_or(CosError::Syntax(p.pos, "trailer"))?;
					return Ok((trailer, false));
				}
				let start = p.int()? as u32;
				let count = p.int()? as u32;
				for i in 0..count {
					p.ws();
					let line = buf.get(p.pos..p.pos + 18).ok_or(CosError::Syntax(p.pos, "xref entry"))?;
					let offset: usize = parse_ascii(&line[0..10]).ok_or(CosError::Syntax(p.pos, "xref offset"))?;
					let kind = line[17];
					p.pos += 18;
					self.xref.entry(start.wrapping_add(i)).or_insert(if kind == b'n' { XrefEntry::Offset(offset) } else { XrefEntry::Free });
				}
			}
		}
		// Xref stream: "n g obj << /Type /XRef ... >> stream"
		p.int()?;
		p.int()?;
		p.keyword(b"obj")?;
		let obj = p.object()?;
		let Obj::Stream(dict, range) = obj else { return Err(CosError::Syntax(off, "xref stream")) };
		let data = decode_stream(&dict, &buf[range])?;
		let w: Vec<usize> = key(&dict, "W")
			.and_then(Obj::as_array)
			.map(|a| a.iter().filter_map(Obj::as_int).map(|v| v as usize).collect())
			.ok_or(CosError::Syntax(off, "xref /W"))?;
		// Fields wider than 8 bytes don't fit a u64 (and a garbage /W would overflow the row size).
		if w.len() != 3 || w.iter().any(|&v| v > 8) {
			return Err(CosError::Syntax(off, "xref /W"));
		}
		let size = key(&dict, "Size").and_then(Obj::as_int).unwrap_or(0);
		let index: Vec<i64> = key(&dict, "Index")
			.and_then(Obj::as_array)
			.map(|a| a.iter().filter_map(Obj::as_int).collect())
			.unwrap_or_else(|| vec![0, size]);
		let row = w[0] + w[1] + w[2];
		let field = |r: &[u8], start: usize, len: usize, default: u64| -> u64 {
			if len == 0 {
				return default;
			}
			r[start..start + len].iter().fold(0u64, |acc, b| (acc << 8) | *b as u64)
		};
		let mut at = 0;
		for pair in index.chunks(2) {
			let [start, count] = pair else { break };
			for i in 0..*count {
				let Some(r) = data.get(at..at + row) else { break };
				at += row;
				let t = field(r, 0, w[0], 1);
				let a = field(r, w[0], w[1], 0);
				let b = field(r, w[0] + w[1], w[2], 0);
				let e = match t {
					1 => XrefEntry::Offset(a as usize),
					2 => XrefEntry::InStream(a as u32, b as u32),
					_ => XrefEntry::Free,
				};
				self.xref.entry(start.wrapping_add(i) as u32).or_insert(e);
			}
		}
		Ok((dict, true))
	}

	pub fn get(&self, num: u32) -> Result<Obj, CosError> {
		match self.xref.get(&num).copied().unwrap_or(XrefEntry::Free) {
			XrefEntry::Free => Ok(Obj::Null),
			XrefEntry::Offset(off) => {
				let mut p = Parser { buf: &self.data, pos: off };
				p.int()?;
				p.int()?;
				p.keyword(b"obj")?;
				let mut o = p.object()?;
				// Streams with an indirect /Length were cut at "endstream"; fine for us.
				if let Obj::Stream(..) = o {
					return Ok(o);
				}
				Ok(std::mem::replace(&mut o, Obj::Null))
			}
			XrefEntry::InStream(stm, idx) => {
				let objs = self.object_stream(stm)?;
				Ok(objs.get(idx as usize).filter(|(n, _)| *n == num).or_else(|| objs.iter().find(|(n, _)| *n == num)).map(|(_, o)| o.clone()).unwrap_or(Obj::Null))
			}
		}
	}

	/// Resolves `o` if it is a reference (one level).
	pub fn resolve(&self, o: &Obj) -> Result<Obj, CosError> {
		match o {
			Obj::Ref(n, _) => self.get(*n),
			other => Ok(other.clone()),
		}
	}

	fn object_stream(&self, num: u32) -> Result<Arc<Vec<(u32, Obj)>>, CosError> {
		if let Some(hit) = self.objstm_cache.borrow().get(&num) {
			return Ok(hit.clone());
		}
		// Object streams can't live in object streams; a file that says so would recurse forever.
		if !matches!(self.xref.get(&num), Some(XrefEntry::Offset(_))) {
			return Err(CosError::Syntax(0, "object stream"));
		}
		let Obj::Stream(dict, range) = self.get(num)? else { return Err(CosError::Syntax(0, "object stream")) };
		let data = decode_stream(&dict, &self.data[range])?;
		// Each header entry takes at least four bytes ("1 0 "), which bounds a garbage /N.
		let n = key(&dict, "N").and_then(Obj::as_int).unwrap_or(0).clamp(0, data.len() as i64 / 4) as usize;
		let first = key(&dict, "First").and_then(Obj::as_int).unwrap_or(0).max(0) as usize;
		let mut header = Parser { buf: &data, pos: 0 };
		let mut entries = Vec::with_capacity(n);
		for _ in 0..n {
			let obj_num = header.int()? as u32;
			let off = header.int()? as usize;
			entries.push((obj_num, off));
		}
		let mut out = Vec::with_capacity(n);
		for (obj_num, off) in entries {
			let mut p = Parser { buf: &data, pos: first.saturating_add(off) };
			out.push((obj_num, p.object().unwrap_or(Obj::Null)));
		}
		let out = Arc::new(out);
		self.objstm_cache.borrow_mut().insert(num, out.clone());
		Ok(out)
	}

	pub fn root_ref(&self) -> Option<(u32, u16)> {
		key(&self.trailer, "Root").and_then(Obj::as_ref)
	}

	pub fn size(&self) -> u32 {
		key(&self.trailer, "Size").and_then(Obj::as_int).unwrap_or(0) as u32
	}
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
	hay.windows(needle.len()).rposition(|w| w == needle)
}

fn parse_ascii<T: std::str::FromStr>(b: &[u8]) -> Option<T> {
	std::str::from_utf8(b).ok()?.trim().parse().ok()
}

pub fn decode_stream(dict: &Dict, raw: &[u8]) -> Result<Vec<u8>, CosError> {
	let filters: Vec<Vec<u8>> = match key(dict, "Filter") {
		Some(Obj::Name(n)) => vec![n.clone()],
		Some(Obj::Array(a)) => a.iter().filter_map(|o| o.as_name().map(<[u8]>::to_vec)).collect(),
		_ => vec![],
	};
	let mut data = raw.to_vec();
	for f in &filters {
		match f.as_slice() {
			b"FlateDecode" | b"Fl" => {
				let mut out = Vec::new();
				// Tolerate truncated/garbage tails the way viewers do.
				let _ = flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out);
				data = out;
			}
			_ => return Err(CosError::Syntax(0, "unsupported stream filter")),
		}
	}
	if let Some(parms) = key(dict, "DecodeParms").and_then(Obj::as_dict) {
		let predictor = key(parms, "Predictor").and_then(Obj::as_int).unwrap_or(1);
		if predictor >= 10 {
			// No row is longer than the data, so clamping a garbage /Columns changes nothing real.
			let columns = key(parms, "Columns").and_then(Obj::as_int).unwrap_or(1).clamp(1, data.len().max(1) as i64) as usize;
			data = png_unpredict(&data, columns);
		}
	}
	Ok(data)
}

fn png_unpredict(data: &[u8], columns: usize) -> Vec<u8> {
	let row = columns + 1;
	let mut out = Vec::with_capacity(data.len());
	let mut prev = vec![0u8; columns];
	for chunk in data.chunks(row) {
		if chunk.len() < row {
			break;
		}
		let (filter, src) = (chunk[0], &chunk[1..]);
		let mut cur = vec![0u8; columns];
		for i in 0..columns {
			let left = if i > 0 { cur[i - 1] } else { 0 };
			let up = prev[i];
			let up_left = if i > 0 { prev[i - 1] } else { 0 };
			cur[i] = match filter {
				0 => src[i],
				1 => src[i].wrapping_add(left),
				2 => src[i].wrapping_add(up),
				3 => src[i].wrapping_add(((left as u16 + up as u16) / 2) as u8),
				4 => {
					let p = left as i16 + up as i16 - up_left as i16;
					let (pa, pb, pc) = ((p - left as i16).abs(), (p - up as i16).abs(), (p - up_left as i16).abs());
					let pred = if pa <= pb && pa <= pc { left } else if pb <= pc { up } else { up_left };
					src[i].wrapping_add(pred)
				}
				_ => src[i],
			};
		}
		out.extend_from_slice(&cur);
		prev = cur;
	}
	out
}

struct Parser<'a> {
	buf: &'a [u8],
	pos: usize,
}

fn is_ws(b: u8) -> bool {
	matches!(b, b' ' | b'\n' | b'\r' | b'\t' | 0x0c | 0)
}
fn is_delim(b: u8) -> bool {
	matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

impl<'a> Parser<'a> {
	fn peek(&self) -> Option<u8> {
		self.buf.get(self.pos).copied()
	}

	/// The bytes from `pos` on (empty past the end: offsets come from the file).
	fn rest(&self) -> &'a [u8] {
		self.buf.get(self.pos..).unwrap_or_default()
	}

	fn ws(&mut self) {
		while let Some(b) = self.peek() {
			if is_ws(b) {
				self.pos += 1;
			} else if b == b'%' {
				while let Some(c) = self.peek() {
					if c == b'\n' || c == b'\r' {
						break;
					}
					self.pos += 1;
				}
			} else {
				break;
			}
		}
	}

	fn token(&mut self) -> &'a [u8] {
		self.ws();
		let start = self.pos;
		while let Some(b) = self.peek() {
			if is_ws(b) || is_delim(b) {
				break;
			}
			self.pos += 1;
		}
		self.buf.get(start..self.pos).unwrap_or_default()
	}

	fn int(&mut self) -> Result<i64, CosError> {
		let at = self.pos;
		parse_ascii(self.token()).ok_or(CosError::Syntax(at, "expected integer"))
	}

	fn keyword(&mut self, kw: &[u8]) -> Result<(), CosError> {
		let at = self.pos;
		if self.token() == kw {
			Ok(())
		} else {
			Err(CosError::Syntax(at, "expected keyword"))
		}
	}

	fn object(&mut self) -> Result<Obj, CosError> {
		self.ws();
		let at = self.pos;
		let b = self.peek().ok_or(CosError::Syntax(at, "eof"))?;
		match b {
			b'/' => {
				self.pos += 1;
				let start = self.pos;
				while let Some(c) = self.peek() {
					if is_ws(c) || is_delim(c) {
						break;
					}
					self.pos += 1;
				}
				Ok(Obj::Name(unescape_name(&self.buf[start..self.pos])))
			}
			b'(' => Ok(Obj::Str(self.literal_string())),
			b'<' if self.buf.get(self.pos + 1) == Some(&b'<') => {
				self.pos += 2;
				let mut d = Dict::new();
				loop {
					self.ws();
					if self.rest().starts_with(b">>") {
						self.pos += 2;
						break;
					}
					let k = match self.object()? {
						Obj::Name(n) => n,
						_ => return Err(CosError::Syntax(self.pos, "dict key")),
					};
					let v = self.object()?;
					d.insert(k, v);
				}
				// A stream follows its dictionary.
				let save = self.pos;
				self.ws();
				if self.rest().starts_with(b"stream") {
					self.pos += 6;
					if self.peek() == Some(b'\r') {
						self.pos += 1;
					}
					if self.peek() == Some(b'\n') {
						self.pos += 1;
					}
					let start = self.pos;
					let len = key(&d, "Length").and_then(Obj::as_int).and_then(|l| usize::try_from(l).ok()).and_then(|l| l.checked_add(start));
					let end = match len {
						Some(end) if self.buf.get(end..).is_some_and(|rest| {
							let mut q = Parser { buf: rest, pos: 0 };
							q.ws();
							q.rest().starts_with(b"endstream")
						}) => end,
						// Indirect or wrong /Length: scan for endstream.
						_ => find_from(self.buf, start, b"endstream").ok_or(CosError::Syntax(start, "endstream"))?,
					};
					self.pos = end;
					return Ok(Obj::Stream(d, start..end));
				}
				self.pos = save;
				Ok(Obj::Dict(d))
			}
			b'<' => {
				self.pos += 1;
				let start = self.pos;
				while self.peek().is_some_and(|c| c != b'>') {
					self.pos += 1;
				}
				let hex: Vec<u8> = self.buf[start..self.pos].iter().copied().filter(|c| c.is_ascii_hexdigit()).collect();
				if self.peek().is_some() {
					self.pos += 1;
				}
				let mut out = Vec::with_capacity(hex.len() / 2 + 1);
				for pair in hex.chunks(2) {
					let hi = (pair[0] as char).to_digit(16).unwrap_or(0);
					let lo = pair.get(1).and_then(|c| (*c as char).to_digit(16)).unwrap_or(0);
					out.push((hi * 16 + lo) as u8);
				}
				Ok(Obj::Str(out))
			}
			b'[' => {
				self.pos += 1;
				let mut items = Vec::new();
				loop {
					self.ws();
					if self.peek() == Some(b']') {
						self.pos += 1;
						break;
					}
					if self.peek().is_none() {
						return Err(CosError::Syntax(self.pos, "unterminated array"));
					}
					items.push(self.object()?);
				}
				Ok(Obj::Array(items))
			}
			_ => {
				let tok = self.token();
				match tok {
					b"null" => Ok(Obj::Null),
					b"true" => Ok(Obj::Bool(true)),
					b"false" => Ok(Obj::Bool(false)),
					_ => {
						if tok.is_empty() {
							self.pos += 1;
							return Err(CosError::Syntax(at, "unexpected byte"));
						}
						let s = std::str::from_utf8(tok).map_err(|_| CosError::Syntax(at, "token"))?;
						if let Ok(i) = s.parse::<i64>() {
							// "n g R" reference?
							let save = self.pos;
							let g = self.token();
							if let Ok(gen) = std::str::from_utf8(g).unwrap_or("x").parse::<u16>() {
								if self.token() == b"R" {
									return Ok(Obj::Ref(i as u32, gen));
								}
							}
							self.pos = save;
							Ok(Obj::Int(i))
						} else if let Ok(r) = s.parse::<f64>() {
							Ok(Obj::Real(r))
						} else {
							Ok(Obj::Null)
						}
					}
				}
			}
		}
	}

	fn literal_string(&mut self) -> Vec<u8> {
		self.pos += 1;
		let mut depth = 1;
		let mut out = Vec::new();
		while let Some(b) = self.peek() {
			self.pos += 1;
			match b {
				b'\\' => {
					let Some(c) = self.peek() else { break };
					self.pos += 1;
					match c {
						b'n' => out.push(b'\n'),
						b'r' => out.push(b'\r'),
						b't' => out.push(b'\t'),
						b'b' => out.push(8),
						b'f' => out.push(12),
						b'0'..=b'7' => {
							let mut v = (c - b'0') as u32;
							for _ in 0..2 {
								match self.peek() {
									Some(d @ b'0'..=b'7') => {
										v = v * 8 + (d - b'0') as u32;
										self.pos += 1;
									}
									_ => break,
								}
							}
							out.push(v as u8);
						}
						b'\r' => {
							if self.peek() == Some(b'\n') {
								self.pos += 1;
							}
						}
						b'\n' => {}
						other => out.push(other),
					}
				}
				b'(' => {
					depth += 1;
					out.push(b);
				}
				b')' => {
					depth -= 1;
					if depth == 0 {
						break;
					}
					out.push(b);
				}
				_ => out.push(b),
			}
		}
		out
	}
}

fn find_from(buf: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
	buf.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn unescape_name(raw: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(raw.len());
	let mut i = 0;
	while i < raw.len() {
		if raw[i] == b'#' && i + 3 <= raw.len() {
			if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or("x"), 16) {
				out.push(v);
				i += 3;
				continue;
			}
		}
		out.push(raw[i]);
		i += 1;
	}
	out
}

/// PDF text string → Rust string (UTF-16BE with BOM, UTF-8 with BOM, or PDFDocEncoding≈Latin-1).
pub fn text_string(bytes: &[u8]) -> String {
	if bytes.starts_with(&[0xFE, 0xFF]) {
		let units: Vec<u16> = bytes[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
		return String::from_utf16_lossy(&units);
	}
	if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
		return String::from_utf8_lossy(&bytes[3..]).into_owned();
	}
	bytes.iter().map(|&b| b as char).collect()
}

// ── Writing ─────────────────────────────────────────────────────────────

pub fn write_obj(o: &Obj, out: &mut Vec<u8>) {
	match o {
		Obj::Null => out.extend_from_slice(b"null"),
		Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
		Obj::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
		Obj::Real(r) => out.extend_from_slice(format!("{r}").as_bytes()),
		Obj::Name(n) => {
			out.push(b'/');
			for &b in n {
				if b <= b' ' || b >= 0x7f || is_delim(b) || b == b'#' {
					out.extend_from_slice(format!("#{b:02X}").as_bytes());
				} else {
					out.push(b);
				}
			}
		}
		Obj::Str(s) => {
			out.push(b'<');
			for b in s {
				out.extend_from_slice(format!("{b:02X}").as_bytes());
			}
			out.push(b'>');
		}
		Obj::Array(a) => {
			out.push(b'[');
			for (i, item) in a.iter().enumerate() {
				if i > 0 {
					out.push(b' ');
				}
				write_obj(item, out);
			}
			out.push(b']');
		}
		Obj::Dict(d) | Obj::Stream(d, _) => {
			out.extend_from_slice(b"<<");
			for (k, v) in d {
				write_obj(&Obj::Name(k.clone()), out);
				out.push(b' ');
				write_obj(v, out);
			}
			out.extend_from_slice(b">>");
		}
		Obj::Ref(n, g) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
	}
}

/// Bytes to append to the file so objects in `replace` take the given values
/// (an incremental update). Uses the same xref flavour as the file's newest
/// section, so strict readers that don't mix them still accept it.
pub fn incremental_update(file: &CosFile, replace: &[(u32, Obj)]) -> Vec<u8> {
	let base = file.bytes().len();
	let mut out = Vec::new();
	if !file.bytes().ends_with(b"\n") {
		out.push(b'\n');
	}
	let mut offsets = Vec::new();
	for (num, obj) in replace {
		offsets.push((*num, base + out.len()));
		out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
		write_obj(obj, &mut out);
		out.extend_from_slice(b"\nendobj\n");
	}
	let mut size = file.size();
	for (n, _) in replace {
		size = size.max(n + 1);
	}
	let mut trailer = Dict::new();
	for k in ["Root", "Info", "ID"] {
		if let Some(v) = key(&file.trailer, k) {
			trailer.insert(k.as_bytes().to_vec(), v.clone());
		}
	}
	trailer.insert(b"Prev".to_vec(), Obj::Int(file.startxref as i64));
	let xref_at = base + out.len();
	if file.xref_is_stream {
		// One more object: the xref stream itself (uncompressed, /W [1 4 2]).
		let xref_num = size;
		size += 1;
		let mut rows = Vec::new();
		let mut index = Vec::new();
		let mut entries: Vec<(u32, usize)> = offsets.clone();
		entries.push((xref_num, xref_at));
		entries.sort();
		for (n, off) in &entries {
			index.push(Obj::Int(*n as i64));
			index.push(Obj::Int(1));
			rows.push(1u8);
			rows.extend_from_slice(&(*off as u32).to_be_bytes());
			rows.extend_from_slice(&0u16.to_be_bytes());
		}
		trailer.insert(b"Type".to_vec(), Obj::Name(b"XRef".to_vec()));
		trailer.insert(b"Size".to_vec(), Obj::Int(size as i64));
		trailer.insert(b"W".to_vec(), Obj::Array(vec![Obj::Int(1), Obj::Int(4), Obj::Int(2)]));
		trailer.insert(b"Index".to_vec(), Obj::Array(index));
		trailer.insert(b"Length".to_vec(), Obj::Int(rows.len() as i64));
		out.extend_from_slice(format!("{xref_num} 0 obj\n").as_bytes());
		write_obj(&Obj::Dict(trailer), &mut out);
		out.extend_from_slice(b"\nstream\n");
		out.extend_from_slice(&rows);
		out.extend_from_slice(b"\nendstream\nendobj\n");
	} else {
		out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
		let mut sorted = offsets.clone();
		sorted.sort();
		for (n, off) in sorted {
			out.extend_from_slice(format!("{n} 1\n{off:010} 00000 n \n").as_bytes());
		}
		trailer.insert(b"Size".to_vec(), Obj::Int(size as i64));
		out.extend_from_slice(b"trailer\n");
		write_obj(&Obj::Dict(trailer), &mut out);
		out.push(b'\n');
	}
	out.extend_from_slice(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_nested_objects() {
		let src = b"<< /Type /Catalog /Pages 3 0 R /Names [ (a\\(b\\)) <48 49> 1.5 -2 true null ] /N#20x /Sub /Inner << /K 1 >> >>";
		let mut p = Parser { buf: src, pos: 0 };
		let o = p.object().unwrap();
		let d = o.as_dict().unwrap();
		assert_eq!(key(d, "Pages"), Some(&Obj::Ref(3, 0)));
		let names = key(d, "Names").unwrap().as_array().unwrap();
		assert_eq!(names[0], Obj::Str(b"a(b)".to_vec()));
		assert_eq!(names[1], Obj::Str(b"HI".to_vec()));
		assert_eq!(names[2], Obj::Real(1.5));
		assert_eq!(names[3], Obj::Int(-2));
		assert!(key(d, "N x").is_some());
		let mut w = Vec::new();
		write_obj(&o, &mut w);
		let mut p2 = Parser { buf: &w, pos: 0 };
		assert_eq!(p2.object().unwrap(), o);
	}

	#[test]
	fn utf16_names() {
		assert_eq!(text_string(&[0xFE, 0xFF, 0, b'E', 0, b'-', 0x00, 0xE9]), "E-é");
	}

	fn cos(bytes: &[u8]) -> Result<CosFile, CosError> {
		let mut m = memmap2::MmapMut::map_anon(bytes.len().max(1))?;
		m[..bytes.len()].copy_from_slice(bytes);
		CosFile::from_mmap(Arc::new(m.make_read_only()?))
	}

	fn sample() -> Vec<u8> {
		sample_with(false)
	}

	/// Catalog, an object stream holding object 3, and an xref stream (/W [1 4 2]).
	/// `self_ref`: the xref says the object stream is inside itself.
	fn sample_with(self_ref: bool) -> Vec<u8> {
		let mut b = b"%PDF-1.5\n".to_vec();
		let body = b"3 0 << /Name (A) /Raw <4142> >>";
		let objs = [b"<< /Type /Catalog /OCProperties << /OCGs [3 0 R] >> >>".to_vec(), [format!("<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n", body.len()).as_bytes(), body, b"\nendstream"].concat()];
		let mut offs = Vec::new();
		for (i, o) in objs.iter().enumerate() {
			offs.push(b.len() as u32);
			b.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
			b.extend_from_slice(o);
			b.extend_from_slice(b"\nendobj\n");
		}
		let xref_at = b.len();
		let mut rows = Vec::new();
		for (t, a) in [(0u8, 0u32), (1, offs[0]), if self_ref { (2, 2) } else { (1, offs[1]) }, (2, 2), (1, xref_at as u32)] {
			rows.push(t);
			rows.extend_from_slice(&a.to_be_bytes());
			rows.extend_from_slice(&0u16.to_be_bytes());
		}
		b.extend_from_slice(format!("4 0 obj\n<< /Type /XRef /Size 5 /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n", rows.len()).as_bytes());
		b.extend_from_slice(&rows);
		b.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
		b
	}

	fn poke(f: &CosFile) {
		for n in 0..8 {
			if let Ok(o) = f.get(n) {
				let _ = f.resolve(&o);
			}
		}
	}

	#[test]
	fn reads_sample() {
		let f = cos(&sample()).unwrap();
		assert_eq!(f.root_ref(), Some((1, 0)));
		assert_eq!(key(f.get(3).unwrap().as_dict().unwrap(), "Raw"), Some(&Obj::Str(b"AB".to_vec())));
	}

	#[test]
	fn truncated_or_mangled_files_never_panic() {
		let src = sample();
		for n in 1..src.len() {
			if let Ok(f) = cos(&src[..n]) {
				poke(&f);
			}
		}
		for i in 0..src.len() {
			for v in [b'0', b'9', b'-', b'<', b'(', b'[', b' ', 0xFF] {
				let mut b = src.clone();
				b[i] = v;
				if let Ok(f) = cos(&b) {
					poke(&f);
				}
			}
		}
	}

	#[test]
	fn garbage_objects_error_not_panic() {
		// A hex string cut off by the end of the buffer inside a dictionary.
		assert!(Parser { buf: b"<< /K <ab", pos: 0 }.object().is_err());
		// Negative /Length: scan for endstream instead.
		let o = Parser { buf: b"<< /Length -1 >>\nstream\nabc\nendstream", pos: 0 }.object().unwrap();
		assert!(matches!(o, Obj::Stream(_, r) if r == (24..28)));
		// Offsets past the end of the buffer.
		assert!(Parser { buf: b"1 0 obj", pos: 99 }.int().is_err());
		// A garbage predictor row width.
		let mut parms = Dict::new();
		parms.insert(b"Predictor".to_vec(), Obj::Int(12));
		parms.insert(b"Columns".to_vec(), Obj::Int(-1));
		let mut d = Dict::new();
		d.insert(b"DecodeParms".to_vec(), Obj::Dict(parms));
		assert!(decode_stream(&d, b"\x00\x01\x00\x02").is_ok());
		// /W wider than a u64, an /XRefStm past the end, and an object stream that lives in itself.
		let swap = |from: &str, to: &str| {
			let src = sample();
			let at = src.windows(from.len()).position(|w| w == from.as_bytes()).unwrap();
			[&src[..at], to.as_bytes(), &src[at + from.len()..]].concat()
		};
		assert!(cos(&swap("/W [1 4 2]", "/W [9 4 2]")).is_err());
		assert!(cos(&swap("/Size 5", "/XRefStm 99999999")).is_err());
		let f = cos(&sample_with(true)).unwrap();
		assert!(f.get(3).is_err());
	}
}
