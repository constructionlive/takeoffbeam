//! Local models for Takeoff Desk: a catalog of vision models that work for
//! takeoff, resumable downloads, installed-model discovery, a memory-fit
//! check from the GGUF header *before* loading, and a supervised
//! `llama-server` that serves the model on 127.0.0.1.

pub mod catalog;
pub mod download;
pub mod fit;
pub mod gguf;
pub mod server;
pub mod store;
pub mod sys;

use fit::{check_fit, CacheType, FitReport, ModelShape};

fn machine_memory() -> sys::MemoryInfo {
	server::find_llama_server()
		.and_then(|bin| sys::memory_info_from_llama(&bin))
		.map(|(m, _)| m)
		.unwrap_or_else(sys::memory_info)
}

/// Will this installed model fit at `ctx` tokens? Reads the GGUF header, so
/// hybrid/sliding-window attention is counted exactly. A header that cannot
/// be read falls back to the size-only estimate used for catalog models.
pub fn fit_for(model: &store::InstalledModel, ctx: u32, cache: CacheType) -> FitReport {
	let mem = machine_memory();
	match ModelShape::from_files(&model.model_path, model.mmproj_path.as_deref()) {
		Ok(shape) => check_fit(&shape, ctx as u64, cache, &mem),
		Err(_) => check_fit(&rough_shape(&model.label, model.size_bytes, 0), ctx as u64, cache, &mem),
	}
}

/// Will this catalog model fit, before downloading it? There is no header
/// yet, so the context cache is a conservative guess: 128 KiB per token at
/// F16 for a ~10 GB model, scaled with model size (clamped 32–256 KiB) —
/// roughly a dense model with 8 KV heads. Hybrid models (Qwen3.5+) need about
/// a quarter of that, so this errs towards "tight" rather than a false "fits".
pub fn fit_for_catalog(model: &catalog::CatalogModel, ctx: u32) -> FitReport {
	let mem = machine_memory();
	// The projector is part of size_bytes; ~0.6 GB of it, which also turns on the
	// vision-encoder compute allowance.
	let projector = if model.mmproj.is_some() { 600 << 20 } else { 0 };
	let weights = model.size_bytes.saturating_sub(projector);
	let shape = match model.kv_bytes_per_token {
		Some(per_token) => ModelShape::rough(&model.label, weights, projector, per_token, 131_072),
		None => rough_shape(&model.label, weights, projector),
	};
	check_fit(&shape, ctx as u64, CacheType::F16, &mem)
}

fn rough_shape(name: &str, weights: u64, projector: u64) -> ModelShape {
	let gb = weights as f64 / (1u64 << 30) as f64;
	let per_token = (128.0 * 1024.0 * (gb / 10.0)).clamp(32.0 * 1024.0, 256.0 * 1024.0) as u64;
	ModelShape::rough(name, weights, projector, per_token, 131_072)
}

/// A command that never pops a console window on Windows (GUI app children).
pub fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
	#[allow(unused_mut)]
	let mut cmd = std::process::Command::new(program);
	#[cfg(windows)]
	{
		use std::os::windows::process::CommandExt;
		const CREATE_NO_WINDOW: u32 = 0x0800_0000;
		cmd.creation_flags(CREATE_NO_WINDOW);
	}
	cmd
}
