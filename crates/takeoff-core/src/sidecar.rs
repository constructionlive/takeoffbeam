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
	use crate::model::*;

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

	#[test]
	fn corrupt_files_load_to_an_error() {
		let dir = std::env::temp_dir().join(format!("takeoff-sidecar-bad-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let pdf = dir.join("x.pdf");
		let good = serde_json::to_vec(&TakeoffData::default()).unwrap();
		let bad: [&[u8]; 6] = [b"", b"\xff\xfe\x00garbage", br#"{"version": "one"}"#, br#"{"version": 1, "reviewType": "takeoff", "marks": [{"id": 3}]}"#, br#"{"version": 1e999}"#, &good[..good.len() / 2]];
		for b in bad {
			std::fs::write(sidecar_path(&pdf), b).unwrap();
			assert!(matches!(load(&pdf), Err(SidecarError::Json(_))), "{}", String::from_utf8_lossy(b));
		}
		// Well-formed but odd: too few points, far off the page, page 0, an id at usize::MAX.
		let odd = br##"{"version": 1, "reviewType": "takeoff",
			"measurements": [
				{"id": "a", "page": 1, "type": "angle", "points": [{"x": 0.1, "y": 0.1}, {"x": 0.2, "y": 0.2}]},
				{"id": "b", "page": 0, "type": "area", "points": []},
				{"id": "c", "page": 1, "type": "length", "points": [{"x": 1e300, "y": -1e300}, {"x": 0.5, "y": 0.5}]}
			],
			"marks": [{"id": "m", "page": 1, "x": 1e300, "y": 0.5, "w": 0, "h": -2}],
			"pageScales": [{"page": 1, "p1": {"x": 0.5, "y": 0.5}, "p2": {"x": 0.5, "y": 0.5}, "distance": 10, "unit": "ft"}]
		}"##;
		std::fs::write(sidecar_path(&pdf), odd).unwrap();
		let raw = load(&pdf).unwrap();
		let mut clean = raw.clone();
		crate::normalize::normalize(&mut clean);
		for d in [&raw, &clean] {
			for m in &d.measurements {
				let _ = crate::geometry::measurement_quantity(m, d.page_scales.first(), 1000.0, 800.0);
			}
			let _ = crate::numbering::mark_seq(&d.marks);
			let _ = crate::numbering::measurement_seq(&d.measurements);
		}
		std::fs::remove_dir_all(dir).ok();
	}

	fn round_trip(d: &TakeoffData, name: &str) -> (TakeoffData, String) {
		let dir = std::env::temp_dir().join(format!("takeoff-sidecar-{name}-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let pdf = dir.join("x.pdf");
		save(&pdf, d).unwrap();
		let text = std::fs::read_to_string(sidecar_path(&pdf)).unwrap();
		let back = load(&pdf).unwrap();
		std::fs::remove_dir_all(dir).ok();
		(back, text)
	}

	#[test]
	fn files_without_spaces_or_legends_keep_their_shape() {
		let mut d = TakeoffData::default();
		d.categories.push(Category::new("c1", "Duplex", "#4CC2FF", CategoryKind::Count));
		let (back, text) = round_trip(&d, "plain");
		assert_eq!(back.categories, d.categories);
		assert!(back.spaces.is_empty() && back.legends.is_empty());
		assert!(!text.contains("\"spaces\"") && !text.contains("\"legends\""), "no new keys when unused");
		// A platform file with neither loads.
		let platform: TakeoffData = serde_json::from_str(r#"{"version":1,"reviewType":"takeoff","categories":[],"marks":[]}"#).unwrap();
		assert!(platform.spaces.is_empty() && platform.legends.is_empty());
	}

	#[test]
	fn spaces_and_legends_round_trip() {
		let mut d = TakeoffData::default();
		let pt = |x, y| Point { x, y };
		d.spaces.push(Space { id: "sp1".into(), page: 1, name: "Unit 204".into(), color: "#34D399".into(), points: vec![pt(0.1, 0.1), pt(0.4, 0.1), pt(0.4, 0.4)], hidden: true, meta: ItemMeta { author: Some("me".into()), ..Default::default() } });
		let mut l = Legend::new("lg1", 2, 0.6, 0.7, 0.3, 0.2);
		l.title = Some("Power".into());
		l.scope = LegendScope::Document;
		l.category_ids = vec!["c1".into()];
		l.columns = LegendColumns { count: true, quantity: false, cost: true };
		l.markups = true;
		d.legends.push(l);
		let (back, text) = round_trip(&d, "new");
		assert_eq!(back.spaces, d.spaces);
		assert_eq!(back.legends, d.legends);
		assert!(text.contains("\"categoryIds\"") && text.contains("\"scope\": \"document\""), "camelCase like the rest");
	}

	#[test]
	fn unknown_fields_and_values_are_ignored() {
		// What an older Takeoff Desk (or construction.live) does with this version's
		// keys, and what this version does with a newer one's.
		let json = r##"{"version":1,"reviewType":"takeoff","futureThing":{"a":1},
			"spaces":[{"id":"sp1","page":1,"name":"Kitchen","color":"#fff","points":[{"x":0,"y":0},{"x":1,"y":0},{"x":1,"y":1}],"futureField":2}],
			"legends":[{"id":"lg1","page":1,"x":0.1,"y":0.1,"w":0.2,"h":0.2,"scope":"set","columns":{"count":false,"area":true}}]}"##;
		let d: TakeoffData = serde_json::from_str(json).unwrap();
		assert_eq!(d.spaces[0].name, "Kitchen");
		assert_eq!(d.legends[0].scope, LegendScope::Unknown, "a newer scope reads as unknown");
		assert_eq!(d.legends[0].columns, LegendColumns { count: false, quantity: true, cost: false });
		assert_eq!(d.legends[0].title(), "Legend");
	}
}
