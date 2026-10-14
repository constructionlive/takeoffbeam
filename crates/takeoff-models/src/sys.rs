//! How much memory a local model may use on this machine.

#[derive(Debug, Clone, Copy)]
pub struct MemoryInfo {
	pub total: u64,
	/// Free + reclaimable right now.
	pub available: u64,
	/// What the model may use at most. On Apple silicon the GPU shares RAM but
	/// Metal caps its working set (about 2/3 of RAM up to 36 GB, 3/4 above),
	/// unless the user raised `iogpu.wired_limit_mb`.
	pub model_budget: u64,
}

pub fn memory_info() -> MemoryInfo {
	let mut sys = sysinfo::System::new();
	sys.refresh_memory();
	let total = sys.total_memory();
	let available = sys.available_memory();
	MemoryInfo { total, available, model_budget: model_budget(total) }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn model_budget(total: u64) -> u64 {
	if let Some(mb) = sysctl_u64("iogpu.wired_limit_mb").filter(|&mb| mb > 0) {
		return (mb * 1024 * 1024).min(total);
	}
	let gib = total / (1 << 30);
	if gib > 36 { total / 4 * 3 } else { total / 3 * 2 }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn model_budget(total: u64) -> u64 {
	// CPU / unified-memory inference elsewhere: leave room for the OS and the app.
	total.saturating_sub(3 << 30).min(total / 10 * 9)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn sysctl_u64(name: &str) -> Option<u64> {
	let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", name]).output().ok()?;
	String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// One compute device as llama.cpp sees it (`llama-server --list-devices`).
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
	pub name: String,
	pub description: String,
	pub total: u64,
	pub free: u64,
}

/// Parses `--list-devices` output lines like `  MTL0: Apple M3 Max (28753 MiB, 28753 MiB free)`.
pub fn parse_list_devices(output: &str) -> Vec<Device> {
	const MIB: u64 = 1024 * 1024;
	output
		.lines()
		.filter_map(|line| {
			let (name, rest) = line.trim().split_once(": ")?;
			let open = rest.rfind('(')?;
			let nums: Vec<u64> = rest[open + 1..]
				.split(|c: char| !c.is_ascii_digit())
				.filter(|s| !s.is_empty())
				.filter_map(|s| s.parse().ok())
				.collect();
			let [total, free] = nums[..] else { return None };
			Some(Device { name: name.to_string(), description: rest[..open].trim().to_string(), total: total * MIB, free: free * MIB })
		})
		.collect()
}

/// Asks llama-server which devices it will use. The largest GPU's limit is the
/// real model budget (Metal working-set cap on Apple silicon, VRAM on CUDA/Vulkan),
/// so this beats the built-in rule of thumb whenever llama-server is available.
pub fn memory_info_from_llama(llama_server: &std::path::Path) -> Option<(MemoryInfo, Vec<Device>)> {
	let out = std::process::Command::new(llama_server).arg("--list-devices").output().ok()?;
	let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
	let devices = parse_list_devices(&text);
	let gpu = devices.iter().filter(|d| d.name != "BLAS" && d.name != "CPU" && d.total > 0).max_by_key(|d| d.total)?;
	let base = memory_info();
	let info = MemoryInfo {
		total: base.total,
		available: base.available.min(gpu.free),
		model_budget: gpu.total,
	};
	Some((info, devices))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_llama_device_list() {
		let out = "Available devices:\n  BLAS: Accelerate (0 MiB, 0 MiB free)\n  MTL0: Apple M3 Max (28753 MiB, 28000 MiB free)\n";
		let d = parse_list_devices(out);
		assert_eq!(d.len(), 2);
		assert_eq!(d[1].name, "MTL0");
		assert_eq!(d[1].description, "Apple M3 Max");
		assert_eq!(d[1].total, 28753 * 1024 * 1024);
		assert_eq!(d[1].free, 28000 * 1024 * 1024);
	}
}
