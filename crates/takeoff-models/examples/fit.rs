//! `cargo run -p takeoff-models --example fit -- model.gguf [mmproj.gguf] [ctx]`
use std::path::Path;

use takeoff_models::fit::{check_fit, CacheType, ModelShape};
use takeoff_models::sys::{memory_info, memory_info_from_llama};

fn main() {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let model = Path::new(args.first().expect("usage: fit <model.gguf> [mmproj.gguf] [ctx]"));
	let projector = args.get(1).filter(|a| a.ends_with(".gguf")).map(Path::new);
	let ctx: u64 = args.iter().find_map(|a| a.parse().ok()).unwrap_or(32_768);
	let shape = ModelShape::from_files(model, projector).expect("read gguf");
	let mem = match memory_info_from_llama(Path::new("llama-server")) {
		Some((mem, devices)) => {
			for d in devices.iter().filter(|d| d.total > 0) {
				println!("device {}: {} ({:.1} GB)", d.name, d.description, d.total as f64 / (1u64 << 30) as f64);
			}
			mem
		}
		None => {
			println!("llama-server not found; using the built-in memory rule of thumb");
			memory_info()
		}
	};
	println!(
		"{} ({}): {} attention + {} recurrent layers, trained ctx {}K, vocab {}",
		shape.name, shape.arch, shape.attention_layers, shape.recurrent_layers, shape.train_ctx / 1024, shape.n_vocab
	);
	for cache in [CacheType::F16, CacheType::Q8_0] {
		println!("  [{cache:?} cache] {}", check_fit(&shape, ctx, cache, &mem).summary());
	}
}
