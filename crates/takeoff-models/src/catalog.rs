//! Curated local vision models that work for takeoff (the agent must see the
//! drawing, so text-only models are left out). Files come from Hugging Face.
//! Sizes are the download total (model + vision projector), from the
//! construction.live desktop catalog.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogModel {
	pub id: String,
	pub label: String,
	/// Hugging Face repo, e.g. "ggml-org/Qwen3.8-27B-GGUF".
	pub repo: String,
	/// Model GGUF file in the repo.
	pub file: String,
	/// Vision projector GGUF in the repo.
	pub mmproj: Option<String>,
	/// Total download size (model + projector), bytes.
	pub size_bytes: u64,
	pub vision: bool,
	/// Context we run it with by default.
	pub context_tokens: u32,
	pub notes: String,
	/// Context-cache bytes per token at F16, when known (measured with
	/// llama.cpp). Used by the pre-download fit check instead of a guess.
	#[serde(default)]
	pub kv_bytes_per_token: Option<u64>,
}

impl CatalogModel {
	/// Download URL of a file in this model's repo.
	pub fn url(&self, file: &str) -> String {
		format!("https://huggingface.co/{}/resolve/main/{}", self.repo, file)
	}
}

fn m(id: &str, label: &str, repo: &str, file: &str, mmproj: &str, size: u64, notes: &str) -> CatalogModel {
	CatalogModel {
		id: id.into(),
		label: label.into(),
		repo: repo.into(),
		file: file.into(),
		mmproj: Some(mmproj.into()),
		size_bytes: size,
		vision: true,
		context_tokens: 32_768,
		notes: notes.into(),
		kv_bytes_per_token: None,
	}
}

pub fn builtin() -> Vec<CatalogModel> {
	let mut models = vec![
		m(
			"Qwen3.8-27B-Q4_K_M",
			"Qwen3.8 27B (Q4_K_M, vision)",
			"ggml-org/Qwen3.8-27B-GGUF",
			"Qwen3.8-27B-Q4_K_M.gguf",
			"mmproj-Qwen3.8-27B-Q8_0.gguf",
			21_045_144_781,
			"Best local takeoff quality. Needs about 22 GB of memory at 32K context.",
		),
		m(
			"gemma-4-26B-A4B-it-Q4_K_M",
			"Gemma 4 26B-A4B (Q4_K_M, vision)",
			"unsloth/gemma-4-26B-A4B-it-GGUF",
			"gemma-4-26B-A4B-it-UD-Q4_K_M.gguf",
			"mmproj-F16.gguf",
			19_542_101_197,
			"Mixture-of-experts: fast for its size. Needs about 20 GB.",
		),
		m(
			"gemma-4-12b-it-Q4_K_M",
			"Gemma 4 12B (Q4_K_M, vision)",
			"unsloth/gemma-4-12b-it-GGUF",
			"gemma-4-12b-it-Q4_K_M.gguf",
			"mmproj-F16.gguf",
			7_816_840_479,
			"Good balance for 16 GB machines.",
		),
		m(
			"gemma-4-E4B-it-Q4_K_M",
			"Gemma 4 E4B (Q4_K_M, vision)",
			"unsloth/gemma-4-E4B-it-GGUF",
			"gemma-4-E4B-it-Q4_K_M.gguf",
			"mmproj-F16.gguf",
			6_442_450_944,
			"Small and quick; fine for reading sheets, weaker at dense counts.",
		),
		m(
			"gemma-4-E2B-it-Q4_K_M",
			"Gemma 4 E2B (Q4_K_M, vision)",
			"unsloth/gemma-4-E2B-it-GGUF",
			"gemma-4-E2B-it-Q4_K_M.gguf",
			"mmproj-F16.gguf",
			4_402_341_478,
			"Smallest; for 8 GB machines. Expect to check its work.",
		),
	];
	// Qwen3.8 27B is hybrid: 16 of 64 layers keep a cache, 4 KV heads × 256 × 2 × f16
	// = 64 KiB per token (llama.cpp reports 2048 MiB at 32K).
	models[0].kv_bytes_per_token = Some(64 * 1024);
	models
}

pub fn find(id: &str) -> Option<CatalogModel> {
	builtin().into_iter().find(|m| m.id == id)
}
