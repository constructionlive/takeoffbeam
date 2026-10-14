//! Scale presets and quantity formatting.
//!
//! A PDF page's size in points IS its paper size (1 pt = 1/72 in), so a
//! printed scale such as 1/4" = 1'-0" is exact without clicking two points:
//! one paper inch along the page width is `72 / page_width_pt` of the page.

use crate::model::{DisplaySettings, LengthFormat, PageScale, Point};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScalePreset {
	pub label: &'static str,
	/// Paper length, in inches.
	pub paper_in: f64,
	/// Real length that paper length stands for, in `unit`.
	pub real: f64,
	pub unit: &'static str,
}

const fn arch(label: &'static str, paper_in: f64) -> ScalePreset {
	ScalePreset { label, paper_in, real: 1.0, unit: "ft" }
}
const fn eng(label: &'static str, feet: f64) -> ScalePreset {
	ScalePreset { label, paper_in: 1.0, real: feet, unit: "ft" }
}
const fn metric(label: &'static str, ratio: f64) -> ScalePreset {
	// 1 paper mm = ratio mm real → express per paper metre in metres.
	ScalePreset { label, paper_in: 1000.0 / 25.4, real: ratio, unit: "m" }
}

pub const ARCHITECTURAL: [ScalePreset; 12] = [
	arch("1/32\" = 1'-0\"", 1.0 / 32.0),
	arch("1/16\" = 1'-0\"", 1.0 / 16.0),
	arch("3/32\" = 1'-0\"", 3.0 / 32.0),
	arch("1/8\" = 1'-0\"", 1.0 / 8.0),
	arch("3/16\" = 1'-0\"", 3.0 / 16.0),
	arch("1/4\" = 1'-0\"", 0.25),
	arch("3/8\" = 1'-0\"", 0.375),
	arch("1/2\" = 1'-0\"", 0.5),
	arch("3/4\" = 1'-0\"", 0.75),
	arch("1\" = 1'-0\"", 1.0),
	arch("1 1/2\" = 1'-0\"", 1.5),
	arch("3\" = 1'-0\"", 3.0),
];

pub const ENGINEERING: [ScalePreset; 9] = [
	eng("1\" = 10'", 10.0),
	eng("1\" = 20'", 20.0),
	eng("1\" = 30'", 30.0),
	eng("1\" = 40'", 40.0),
	eng("1\" = 50'", 50.0),
	eng("1\" = 60'", 60.0),
	eng("1\" = 80'", 80.0),
	eng("1\" = 100'", 100.0),
	eng("1\" = 200'", 200.0),
];

pub const METRIC: [ScalePreset; 9] = [
	metric("1:1", 1.0),
	metric("1:5", 5.0),
	metric("1:10", 10.0),
	metric("1:20", 20.0),
	metric("1:50", 50.0),
	metric("1:100", 100.0),
	metric("1:200", 200.0),
	metric("1:500", 500.0),
	metric("1:1000", 1000.0),
];

impl ScalePreset {
	/// The preset as a stored calibration for a page `width_pt` wide.
	/// The reference runs along the top edge from x=0.
	pub fn to_page_scale(&self, page: u32, width_pt: f64) -> PageScale {
		PageScale {
			page,
			p1: Point { x: 0.0, y: 0.0 },
			p2: Point { x: self.paper_in * 72.0 / width_pt, y: 0.0 },
			distance: self.real,
			unit: self.unit.to_string(),
		}
	}

	pub fn all() -> impl Iterator<Item = &'static ScalePreset> {
		ARCHITECTURAL.iter().chain(ENGINEERING.iter()).chain(METRIC.iter())
	}
}

/// Factor to convert `from` length units into `to`. None for unknown units.
pub fn length_factor(from: &str, to: &str) -> Option<f64> {
	fn metres(u: &str) -> Option<f64> {
		Some(match u {
			"ft" => 0.3048,
			"in" => 0.0254,
			"yd" => 0.9144,
			"mi" => 1609.344,
			"m" => 1.0,
			"cm" => 0.01,
			"mm" => 0.001,
			"km" => 1000.0,
			_ => return None,
		})
	}
	Some(metres(from)? / metres(to)?)
}

/// "12'-6 1/2\"" from a length in feet, rounded to 1/`denominator` inch.
pub fn feet_inches(feet: f64, denominator: u32) -> String {
	let sign = if feet < 0.0 { "-" } else { "" };
	let total_frac = (feet.abs() * 12.0 * denominator as f64).round() as u64;
	let den = denominator as u64;
	let whole_in = total_frac / den;
	let mut num = total_frac % den;
	let ft = whole_in / 12;
	let inch = whole_in % 12;
	let mut d = den;
	while num > 0 && num % 2 == 0 && d % 2 == 0 {
		num /= 2;
		d /= 2;
	}
	if num == 0 {
		format!("{sign}{ft}'-{inch}\"")
	} else {
		format!("{sign}{ft}'-{inch} {num}/{d}\"")
	}
}

/// A length for display under the document's settings.
pub fn format_length(value: f64, unit: &str, display: &DisplaySettings) -> String {
	if display.length_format == LengthFormat::FeetInches {
		if let Some(f) = length_factor(unit, "ft") {
			return feet_inches(value * f, display.fraction_denominator.max(2));
		}
	}
	format!("{:.*} {unit}", display.precision as usize, value)
}

/// Any quantity (length, area "ft²", volume "ft³", count "ea", angle "°").
pub fn format_quantity(value: f64, unit: &str, display: &DisplaySettings) -> String {
	if unit == "°" {
		return format!("{value:.1}°");
	}
	if unit == "ea" {
		return format!("{value:.0} ea");
	}
	if unit.ends_with('²') || unit.ends_with('³') {
		return format!("{:.*} {unit}", display.precision as usize, value);
	}
	format_length(value, unit, display)
}

/// Thousands separators for totals: 12,345.6
pub fn group_thousands(s: &str) -> String {
	let (int, rest) = match s.find(|c: char| !c.is_ascii_digit() && c != '-') {
		Some(i) => s.split_at(i),
		None => (s, ""),
	};
	let (sign, digits) = int.strip_prefix('-').map(|d| ("-", d)).unwrap_or(("", int));
	let mut out = String::new();
	for (i, c) in digits.chars().enumerate() {
		if i > 0 && (digits.len() - i) % 3 == 0 {
			out.push(',');
		}
		out.push(c);
	}
	format!("{sign}{out}{rest}")
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::geometry::scale_units_per_px;

	#[test]
	fn quarter_inch_preset_is_exact() {
		// 36×24 in sheet = 2592×1728 pt. At 1/4" = 1'-0", 36 in of paper = 144 ft.
		let s = ARCHITECTURAL[5].to_page_scale(1, 2592.0);
		let (w, h) = (2592.0 * 2.0, 1728.0 * 2.0); // any render size
		let upp = scale_units_per_px(&s, w, h).unwrap();
		assert!((upp * w - 144.0).abs() < 1e-9);
	}

	#[test]
	fn metric_preset() {
		// A1 width 841 mm at 1:100 = 84.1 m.
		let w_pt = 841.0 / 25.4 * 72.0;
		let s = METRIC[5].to_page_scale(1, w_pt);
		let upp = scale_units_per_px(&s, 1000.0, 700.0).unwrap();
		assert!((upp * 1000.0 - 84.1).abs() < 1e-9);
	}

	#[test]
	fn feet_inches_format() {
		assert_eq!(feet_inches(12.5, 16), "12'-6\"");
		assert_eq!(feet_inches(12.0 + 6.5 / 12.0, 16), "12'-6 1/2\"");
		assert_eq!(feet_inches(0.0, 16), "0'-0\"");
		assert_eq!(feet_inches(1.0 - 1.0 / 12.0 / 64.0, 16), "1'-0\"");
	}

	#[test]
	fn thousands() {
		assert_eq!(group_thousands("12345.67 ft"), "12,345.67 ft");
		assert_eq!(group_thousands("-1234"), "-1,234");
		assert_eq!(group_thousands("999"), "999");
	}
}
