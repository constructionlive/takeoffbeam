//! Minimal GGUF metadata reader. Reads the header key/values only, never the
//! tensor data, so it is cheap even on a 20 GB model file.
//! Format: https://github.com/ggml-org/ggml/blob/master/docs/gguf.md

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum GgufError {
	#[error("io: {0}")]
	Io(#[from] std::io::Error),
	#[error("not a GGUF file")]
	BadMagic,
	#[error("unsupported GGUF version {0}")]
	Version(u32),
	#[error("bad value type {0}")]
	BadType(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
	U64(u64),
	I64(i64),
	F64(f64),
	Bool(bool),
	Str(String),
	/// Numeric arrays are kept (per-layer head counts); string arrays such as
	/// the tokenizer vocab are skipped and only their length is recorded.
	Array(Vec<Value>),
	SkippedArray(u64),
}

impl Value {
	pub fn as_u64(&self) -> Option<u64> {
		match self {
			Value::U64(v) => Some(*v),
			Value::I64(v) if *v >= 0 => Some(*v as u64),
			_ => None,
		}
	}
	pub fn as_str(&self) -> Option<&str> {
		match self {
			Value::Str(s) => Some(s),
			_ => None,
		}
	}
}

#[derive(Debug, Clone)]
pub struct GgufMeta {
	pub version: u32,
	pub tensor_count: u64,
	pub kv: BTreeMap<String, Value>,
}

impl GgufMeta {
	pub fn get(&self, key: &str) -> Option<&Value> {
		self.kv.get(key)
	}
	pub fn arch(&self) -> Option<&str> {
		self.get("general.architecture").and_then(Value::as_str)
	}
	/// `{arch}.{suffix}` as a number.
	pub fn arch_u64(&self, suffix: &str) -> Option<u64> {
		let arch = self.arch()?;
		self.get(&format!("{arch}.{suffix}")).and_then(Value::as_u64)
	}
	/// `{arch}.{suffix}` as a per-layer list. A scalar is broadcast to `n` layers.
	pub fn arch_per_layer(&self, suffix: &str, n: usize) -> Option<Vec<u64>> {
		let arch = self.arch()?;
		match self.get(&format!("{arch}.{suffix}"))? {
			Value::Array(items) => Some(items.iter().filter_map(Value::as_u64).collect()),
			v => v.as_u64().map(|x| vec![x; n]),
		}
	}
}

pub fn read_meta(path: &Path) -> Result<GgufMeta, GgufError> {
	let mut r = BufReader::with_capacity(1 << 20, File::open(path)?);
	let mut magic = [0u8; 4];
	r.read_exact(&mut magic)?;
	if &magic != b"GGUF" {
		return Err(GgufError::BadMagic);
	}
	let version = read_u32(&mut r)?;
	if !(2..=3).contains(&version) {
		return Err(GgufError::Version(version));
	}
	let tensor_count = read_u64(&mut r)?;
	let kv_count = read_u64(&mut r)?;
	let mut kv = BTreeMap::new();
	for _ in 0..kv_count {
		let key = read_string(&mut r)?;
		let ty = read_u32(&mut r)?;
		kv.insert(key, read_value(&mut r, ty)?);
	}
	Ok(GgufMeta { version, tensor_count, kv })
}

fn read_value(r: &mut impl Read, ty: u32) -> Result<Value, GgufError> {
	Ok(match ty {
		0 => Value::U64(read_n::<1>(r)?[0] as u64),
		1 => Value::I64(read_n::<1>(r)?[0] as i8 as i64),
		2 => Value::U64(u16::from_le_bytes(read_n(r)?) as u64),
		3 => Value::I64(i16::from_le_bytes(read_n(r)?) as i64),
		4 => Value::U64(read_u32(r)? as u64),
		5 => Value::I64(i32::from_le_bytes(read_n(r)?) as i64),
		6 => Value::F64(f32::from_le_bytes(read_n(r)?) as f64),
		7 => Value::Bool(read_n::<1>(r)?[0] != 0),
		8 => Value::Str(read_string(r)?),
		9 => {
			let item_ty = read_u32(r)?;
			let n = read_u64(r)?;
			if item_ty == 8 || item_ty == 9 {
				for _ in 0..n {
					read_value(r, item_ty)?;
				}
				Value::SkippedArray(n)
			} else {
				let mut items = Vec::with_capacity(n.min(1 << 16) as usize);
				for _ in 0..n {
					items.push(read_value(r, item_ty)?);
				}
				Value::Array(items)
			}
		}
		10 => Value::U64(read_u64(r)?),
		11 => Value::I64(i64::from_le_bytes(read_n(r)?)),
		12 => Value::F64(f64::from_le_bytes(read_n(r)?)),
		t => return Err(GgufError::BadType(t)),
	})
}

fn read_n<const N: usize>(r: &mut impl Read) -> Result<[u8; N], GgufError> {
	let mut b = [0u8; N];
	r.read_exact(&mut b)?;
	Ok(b)
}
fn read_u32(r: &mut impl Read) -> Result<u32, GgufError> {
	Ok(u32::from_le_bytes(read_n(r)?))
}
fn read_u64(r: &mut impl Read) -> Result<u64, GgufError> {
	Ok(u64::from_le_bytes(read_n(r)?))
}
fn read_string(r: &mut impl Read) -> Result<String, GgufError> {
	let len = read_u64(r)?;
	// Read what is there rather than allocating a garbage length up front (a truncated download).
	let mut buf = Vec::new();
	r.take(len).read_to_end(&mut buf)?;
	if (buf.len() as u64) < len {
		return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
	}
	Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn garbage_string_length_is_an_error() {
		let mut b = (1u64 << 60).to_le_bytes().to_vec();
		b.extend_from_slice(b"abc");
		assert!(matches!(read_string(&mut b.as_slice()), Err(GgufError::Io(_))));
		let mut b = 3u64.to_le_bytes().to_vec();
		b.extend_from_slice(b"abc");
		assert_eq!(read_string(&mut b.as_slice()).unwrap(), "abc");
	}
}
