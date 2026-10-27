//! "Will this model run on this machine?" — estimated from the GGUF header
//! and the machine's memory, before we ever start llama-server.
//!
//! need = weights (model + vision projector file sizes)
//!      + KV cache (attention layers × context × per-token bytes)
//!      + recurrent state (hybrid SSM layers, fixed size)
//!      + compute buffers (logits batch + vision encoder) + runtime overhead
//!
//! Estimates are deliberately a little conservative: a "fits" that then
//! fails to load is worse than a "tight" that loads fine.

use std::path::Path;

use crate::gguf::{read_meta, GgufError, GgufMeta, Value};
use crate::sys::MemoryInfo;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// llama-server's default micro-batch.
const N_UBATCH: u64 = 512;
const MAX_LAYERS: u64 = 4096;
const RUNTIME_OVERHEAD: u64 = 400 * 1024 * 1024;
/// Vision encoder activations while an image is being encoded.
const VISION_COMPUTE: u64 = 600 * 1024 * 1024;

/// KV cache element type, as passed to llama-server `--cache-type-k/v`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
	F16,
	Q8_0,
	Q4_0,
}

impl CacheType {
	fn bytes_per_elem(self) -> f64 {
		match self {
			CacheType::F16 => 2.0,
			CacheType::Q8_0 => 34.0 / 32.0,
			CacheType::Q4_0 => 18.0 / 32.0,
		}
	}
}

/// The memory-relevant shape of a model, read once from its GGUF header.
#[derive(Debug, Clone)]
pub struct ModelShape {
	pub name: String,
	pub arch: String,
	pub weights_bytes: u64,
	pub projector_bytes: u64,
	pub train_ctx: u64,
	pub n_vocab: u64,
	pub n_embd: u64,
	/// Per-token K+V elements for each attention layer that grows with context.
	full_attn_kv_elems: u64,
	/// Per-token K+V elements for sliding-window layers, and the window size.
	swa_kv_elems: u64,
	swa_window: u64,
	/// Fixed recurrent-state bytes (hybrid SSM / linear-attention layers).
	recurrent_bytes: u64,
	pub attention_layers: u64,
	pub recurrent_layers: u64,
}

impl ModelShape {
	/// Reads `model` (and the optional vision projector) headers. File sizes count as weights:
	/// on Apple silicon everything is offloaded to the GPU, which shares system RAM.
	pub fn from_files(model: &Path, projector: Option<&Path>) -> Result<Self, GgufError> {
		let meta = read_meta(model)?;
		let weights_bytes = std::fs::metadata(model)?.len();
		let projector_bytes = match projector {
			Some(p) => std::fs::metadata(p)?.len(),
			None => 0,
		};
		Ok(Self::from_meta(&meta, weights_bytes, projector_bytes))
	}

	pub fn from_meta(meta: &GgufMeta, weights_bytes: u64, projector_bytes: u64) -> Self {
		let arch = meta.arch().unwrap_or("unknown").to_string();
		// No model has more; a garbage header would allocate a vector per layer without end.
		let n_layer = meta.arch_u64("block_count").unwrap_or(0).min(MAX_LAYERS);
		let n_embd = meta.arch_u64("embedding_length").unwrap_or(0);
		let n_head = meta.arch_u64("attention.head_count").unwrap_or(1).max(1);
		let head_dim = n_embd / n_head;
		let k_len = meta.arch_u64("attention.key_length").unwrap_or(head_dim);
		let v_len = meta.arch_u64("attention.value_length").unwrap_or(head_dim);
		let kv_heads = meta
			.arch_per_layer("attention.head_count_kv", n_layer as usize)
			.unwrap_or_else(|| vec![n_head; n_layer as usize]);

		// Hybrid models (Qwen3.5+/Next, Jamba-style): only every Nth layer is attention.
		let interval = meta.arch_u64("full_attention_interval").filter(|&i| i > 1);
		let is_attention = |layer: u64| match interval {
			Some(i) => (layer + 1) % i == 0,
			None => true,
		};
		// Sliding-window attention (Gemma 2/3/4, etc.): a per-layer pattern if the file has one.
		let swa_window = meta.arch_u64("attention.sliding_window").unwrap_or(0);
		let swa_pattern: Option<Vec<bool>> = meta
			.get(&format!("{arch}.attention.sliding_window_pattern"))
			.and_then(|v| match v {
				Value::Array(items) => Some(items.iter().map(|x| matches!(x, Value::Bool(true))).collect()),
				_ => None,
			});

		let mut full = 0u64;
		let mut swa = 0u64;
		let mut attention_layers = 0u64;
		for layer in 0..n_layer {
			if !is_attention(layer) {
				continue;
			}
			attention_layers += 1;
			let heads = kv_heads.get(layer as usize).copied().unwrap_or(n_head);
			let elems = heads * (k_len + v_len);
			let sliding = swa_window > 0 && swa_pattern.as_ref().is_some_and(|p| p.get(layer as usize) == Some(&true));
			if sliding {
				swa += elems;
			} else {
				full += elems;
			}
		}

		let recurrent_layers = n_layer - attention_layers;
		let recurrent_bytes = if recurrent_layers > 0 {
			let conv = meta.arch_u64("ssm.conv_kernel").unwrap_or(4).saturating_sub(1);
			let inner = meta.arch_u64("ssm.inner_size").unwrap_or(0);
			let state = meta.arch_u64("ssm.state_size").unwrap_or(0);
			let groups = meta.arch_u64("ssm.group_count").unwrap_or(1);
			// f32 conv state + f32 SSM state per recurrent layer (llama.cpp r/s caches).
			recurrent_layers * 4 * (conv * (inner + 2 * groups * state) + state * inner)
		} else {
			0
		};

		let n_vocab = match meta.get("tokenizer.ggml.tokens") {
			Some(Value::SkippedArray(n)) => *n,
			_ => meta.arch_u64("vocab_size").unwrap_or(32_000),
		};
		Self {
			name: meta.get("general.name").and_then(Value::as_str).unwrap_or("model").to_string(),
			arch,
			weights_bytes,
			projector_bytes,
			train_ctx: meta.arch_u64("context_length").unwrap_or(4096),
			n_vocab,
			n_embd,
			full_attn_kv_elems: full,
			swa_kv_elems: swa,
			swa_window,
			recurrent_bytes,
			attention_layers,
			recurrent_layers,
		}
	}

	/// A shape without a header (catalog models before download): all of
	/// `kv_bytes_per_token` (at F16) is treated as full attention.
	pub fn rough(name: &str, weights_bytes: u64, projector_bytes: u64, kv_bytes_per_token_f16: u64, train_ctx: u64) -> Self {
		Self {
			name: name.to_string(),
			arch: "unknown".into(),
			weights_bytes,
			projector_bytes,
			train_ctx,
			n_vocab: 262_144,
			n_embd: 4096,
			full_attn_kv_elems: kv_bytes_per_token_f16 / 2,
			swa_kv_elems: 0,
			swa_window: 0,
			recurrent_bytes: 0,
			attention_layers: 0,
			recurrent_layers: 0,
		}
	}

	pub fn kv_bytes(&self, n_ctx: u64, cache: CacheType) -> u64 {
		let per_tok = self.full_attn_kv_elems as f64 * n_ctx as f64
			+ self.swa_kv_elems as f64 * n_ctx.min(self.swa_window.max(1)) as f64;
		(per_tok * cache.bytes_per_elem()) as u64
	}

	pub fn estimate(&self, n_ctx: u64, cache: CacheType) -> Estimate {
		let compute = N_UBATCH * 4 * (self.n_vocab + 8 * self.n_embd)
			+ if self.projector_bytes > 0 { VISION_COMPUTE } else { 0 };
		Estimate {
			weights: self.weights_bytes + self.projector_bytes,
			kv_cache: self.kv_bytes(n_ctx, cache),
			recurrent: self.recurrent_bytes,
			compute: compute + RUNTIME_OVERHEAD,
			n_ctx,
		}
	}
}

#[derive(Debug, Clone, Copy)]
pub struct Estimate {
	pub weights: u64,
	pub kv_cache: u64,
	pub recurrent: u64,
	pub compute: u64,
	pub n_ctx: u64,
}

impl Estimate {
	pub fn total(&self) -> u64 {
		self.weights + self.kv_cache + self.recurrent + self.compute
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
	/// Fits the budget and what is free right now.
	Fits,
	/// Fits the machine, but not what is free right now: close other apps first.
	Tight,
	/// More than this machine can give the model.
	TooBig,
}

#[derive(Debug, Clone)]
pub struct FitReport {
	pub verdict: Verdict,
	pub estimate: Estimate,
	/// Memory the model may use on this machine (GPU working-set limit on Apple silicon).
	pub budget: u64,
	pub available_now: u64,
	/// Largest context (tokens, multiple of 4096) that fits the budget, if any.
	pub max_ctx: Option<u64>,
}

impl FitReport {
	pub fn summary(&self) -> String {
		let gb = |b: u64| b as f64 / GIB;
		let e = &self.estimate;
		let verdict = match self.verdict {
			Verdict::Fits => "Fits",
			Verdict::Tight => "Tight: close other apps before loading",
			Verdict::TooBig => "Not enough memory",
		};
		format!(
			"{verdict} — needs {:.1} GB (weights {:.1} + context {:.1} @ {}K + state {:.2} + buffers {:.1}); \
			 this machine allows {:.1} GB, {:.1} GB free now{}",
			gb(e.total()),
			gb(e.weights),
			gb(e.kv_cache),
			e.n_ctx / 1024,
			gb(e.recurrent),
			gb(e.compute),
			gb(self.budget),
			gb(self.available_now),
			match self.max_ctx {
				Some(c) => format!("; max context that fits: {}K", c / 1024),
				None => String::new(),
			}
		)
	}
}

pub fn check_fit(shape: &ModelShape, n_ctx: u64, cache: CacheType, mem: &MemoryInfo) -> FitReport {
	let estimate = shape.estimate(n_ctx, cache);
	let need = estimate.total();
	let verdict = if need > mem.model_budget {
		Verdict::TooBig
	} else if need > mem.available {
		Verdict::Tight
	} else {
		Verdict::Fits
	};
	let mut max_ctx = None;
	let mut ctx = 4096;
	while ctx <= shape.train_ctx && shape.estimate(ctx, cache).total() <= mem.model_budget {
		max_ctx = Some(ctx);
		ctx += 4096;
	}
	FitReport { verdict, estimate, budget: mem.model_budget, available_now: mem.available, max_ctx }
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	use super::*;

	fn meta(pairs: &[(&str, Value)]) -> GgufMeta {
		let mut kv = BTreeMap::new();
		for (k, v) in pairs {
			kv.insert(k.to_string(), v.clone());
		}
		GgufMeta { version: 3, tensor_count: 0, kv }
	}

	#[test]
	fn hybrid_model_only_caches_attention_layers() {
		// Shape of Qwen3.8-27B: 64 layers, every 4th is attention, 4 KV heads × 256.
		let m = meta(&[
			("general.architecture", Value::Str("qwen35".into())),
			("qwen35.block_count", Value::U64(64)),
			("qwen35.embedding_length", Value::U64(5120)),
			("qwen35.attention.head_count", Value::U64(24)),
			("qwen35.attention.head_count_kv", Value::U64(4)),
			("qwen35.attention.key_length", Value::U64(256)),
			("qwen35.attention.value_length", Value::U64(256)),
			("qwen35.full_attention_interval", Value::U64(4)),
		]);
		let s = ModelShape::from_meta(&m, 0, 0);
		assert_eq!(s.attention_layers, 16);
		// 16 layers × 4 heads × 512 elems × 2 bytes = 64 KiB per token.
		assert_eq!(s.kv_bytes(1, CacheType::F16), 65_536);
	}

	#[test]
	fn garbage_layer_count_is_bounded() {
		let m = meta(&[("general.architecture", Value::Str("x".into())), ("x.block_count", Value::U64(u64::MAX))]);
		assert_eq!(ModelShape::from_meta(&m, 0, 0).attention_layers, MAX_LAYERS);
	}

	#[test]
	fn verdicts() {
		let s = ModelShape::from_meta(&meta(&[]), 10 << 30, 0);
		let mem = |budget: u64, available: u64| MemoryInfo { total: budget, available, model_budget: budget };
		assert_eq!(check_fit(&s, 4096, CacheType::F16, &mem(20 << 30, 20 << 30)).verdict, Verdict::Fits);
		assert_eq!(check_fit(&s, 4096, CacheType::F16, &mem(20 << 30, 8 << 30)).verdict, Verdict::Tight);
		assert_eq!(check_fit(&s, 4096, CacheType::F16, &mem(8 << 30, 8 << 30)).verdict, Verdict::TooBig);
	}
}
