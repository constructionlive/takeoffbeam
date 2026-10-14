//! The takeoff lives next to the drawing: `plan.pdf` → `plan.pdf.takeoff.json`.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::TakeoffData;

pub fn sidecar_path(pdf: &Path) -> PathBuf {
	let mut name = pdf.file_name().map(|n| n.to_os_string()).unwrap_or_default();
	name.push(".takeoff.json");
	pdf.with_file_name(name)
}

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
	#[error("io: {0}")]
	Io(#[from] std::io::Error),
	#[error("bad takeoff file: {0}")]
	Json(#[from] serde_json::Error),
}

/// Loads the sidecar, or an empty takeoff when there is none.
pub fn load(pdf: &Path) -> Result<TakeoffData, SidecarError> {
	let path = sidecar_path(pdf);
	match std::fs::read(&path) {
		Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TakeoffData::default()),
		Err(e) => Err(e.into()),
	}
}

/// Atomic save: write a temp file beside the target, then rename over it.
pub fn save(pdf: &Path, data: &TakeoffData) -> Result<(), SidecarError> {
	save_to(&sidecar_path(pdf), data)
}

pub fn save_to(path: &Path, data: &TakeoffData) -> Result<(), SidecarError> {
	let mut data = data.clone();
	data.last_updated = Some(crate::time::iso_now());
	let json = serde_json::to_vec_pretty(&data)?;
	let tmp = path.with_extension("json.tmp");
	{
		let mut f = std::fs::File::create(&tmp)?;
		f.write_all(&json)?;
		f.sync_all()?;
	}
	std::fs::rename(&tmp, path)?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn path_and_round_trip() {
		assert_eq!(sidecar_path(Path::new("/a/b/plan.pdf")), PathBuf::from("/a/b/plan.pdf.takeoff.json"));
		let dir = std::env::temp_dir().join(format!("takeoff-sidecar-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let pdf = dir.join("x.pdf");
		assert_eq!(load(&pdf).unwrap(), TakeoffData::default());
		let mut d = TakeoffData::default();
		d.hidden_layers.push("E-POWER".into());
		save(&pdf, &d).unwrap();
		let back = load(&pdf).unwrap();
		assert_eq!(back.hidden_layers, vec!["E-POWER".to_string()]);
		assert!(back.last_updated.is_some());
		std::fs::remove_dir_all(dir).ok();
	}
}
