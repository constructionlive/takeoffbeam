//! Golden outputs from construction.live's TypeScript (scripts/parity/gen.sh):
//! numbering, measurement maths, unit keys and vision-coordinate handling
//! must match the platform exactly, so a takeoff means the same in both.

use serde_json::Value;
use takeoff_core::coords::{claude_resized_size, dialect_for, image_fit, safe_max_edge, Dialect};
use takeoff_core::geometry::{measurement_value, unit_key};
use takeoff_core::numbering::{mark_seq, measurement_seq};
use takeoff_core::{Mark, Measurement, PageScale};

fn fixture() -> Value {
	serde_json::from_str(include_str!("fixtures/platform_parity.json")).unwrap()
}

#[test]
fn badge_numbers_match() {
	let f = fixture();
	let marks: Vec<Mark> = serde_json::from_value(f["marks"].clone()).unwrap();
	let seq = mark_seq(&marks);
	for (id, n) in f["markSeq"].as_object().unwrap() {
		assert_eq!(seq[id] as u64, n.as_u64().unwrap(), "mark {id}");
	}
	let ms: Vec<Measurement> = serde_json::from_value(f["measurements"].clone()).unwrap();
	let seq = measurement_seq(&ms);
	for (id, n) in f["measurementSeq"].as_object().unwrap() {
		assert_eq!(seq[id] as u64, n.as_u64().unwrap(), "measurement {id}");
	}
}

#[test]
fn measurement_values_match() {
	let f = fixture();
	let ms: Vec<Measurement> = serde_json::from_value(f["measurements"].clone()).unwrap();
	let scale: PageScale = serde_json::from_value(f["scale"].clone()).unwrap();
	for (m, want) in ms.iter().zip(f["values"].as_array().unwrap()) {
		let got = measurement_value(m, Some(&scale), 2592.0, 1728.0);
		match (got, want) {
			(None, Value::Null) => {}
			(Some((v, u)), w) => {
				let wv = w["value"].as_f64().unwrap();
				assert!((v - wv).abs() <= 1e-9 * wv.abs().max(1.0), "{}: {v} vs {wv}", m.id);
				assert_eq!(u, w["unit"].as_str().unwrap());
			}
			(g, w) => panic!("{}: {g:?} vs {w}", m.id),
		}
	}
}

#[test]
fn unit_keys_match() {
	for (u, want) in fixture()["unitKeys"].as_object().unwrap() {
		assert_eq!(unit_key(u), want.as_str().unwrap(), "{u}");
	}
}

#[test]
fn image_fit_matches() {
	let f = fixture();
	for e in f["fits"].as_array().unwrap() {
		let model = e["model"].as_str().unwrap();
		let (w, h) = (e["w"].as_u64().unwrap() as u32, e["h"].as_u64().unwrap() as u32);
		let fit = image_fit(model, w, h);
		assert_eq!((fit.0 as u64, fit.1 as u64), (e["fit"]["w"].as_u64().unwrap(), e["fit"]["h"].as_u64().unwrap()), "{model} {w}x{h}");
		assert_eq!(safe_max_edge(model, 2000) as u64, e["safe"].as_u64().unwrap(), "{model}");
		let want = if e["dialect"] == "grid1000" { Dialect::Grid1000Yx } else { Dialect::Pixel };
		assert_eq!(dialect_for(model), want, "{model}");
	}
	let sizes = [(3400, 2200), (1092, 1092), (800, 4000), (7200, 4800), (1568, 1568), (1569, 100)];
	for ((w, h), want) in sizes.iter().zip(f["claude"].as_array().unwrap()) {
		let got = claude_resized_size(*w, *h);
		assert_eq!((got.0 as u64, got.1 as u64), (want["w"].as_u64().unwrap(), want["h"].as_u64().unwrap()));
	}
}

#[test]
fn box_parsing_matches() {
	let raw = "Sure:\n```json\n[{\"box_2d\":[100,200,300,450],\"label\":\"A\"},{\"box\":[\"5\",6,7,8]},{\"box\":[1,2,3]}]\n```";
	for e in fixture()["boxes"].as_array().unwrap() {
		let d = if e["dialect"] == "grid1000" { Dialect::Grid1000Yx } else { Dialect::Pixel };
		let parsed = d.parse_boxes(raw);
		let want = e["parsed"].as_array().unwrap();
		assert_eq!(parsed.len(), want.len());
		for (p, w) in parsed.iter().zip(want) {
			assert_eq!((p.x1, p.y1, p.x2, p.y2), (w["x1"].as_f64().unwrap(), w["y1"].as_f64().unwrap(), w["x2"].as_f64().unwrap(), w["y2"].as_f64().unwrap()));
			assert_eq!(p.label.as_deref(), w["label"].as_str());
		}
		let b = takeoff_core::coords::RawBox { x1: 100.0, y1: 200.0, x2: 300.0, y2: 450.0, label: None };
		let f = d.box_to_fractions(b, 1000.0, 800.0);
		let pg = takeoff_core::coords::region_to_page((0.25, 0.5, 0.5, 0.25), f);
		let w = &e["page"];
		for (g, k) in [(pg.0, "x"), (pg.1, "y"), (pg.2, "w"), (pg.3, "h")] {
			assert!((g - w[k].as_f64().unwrap()).abs() < 1e-12, "{k}");
		}
	}
}
