//! Tiny UTC formatting so the core needs no date crate.

/// "2026-10-14T18:03:12Z"
pub fn iso_now() -> String {
	iso_from_ms(crate::model::now_ms())
}

pub fn iso_from_ms(ms: i64) -> String {
	let secs = ms.div_euclid(1000);
	let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
	let (y, m, d) = civil_from_days(days);
	format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// "2026-10-14" for list columns.
pub fn date_from_ms(ms: i64) -> String {
	iso_from_ms(ms)[..10].to_string()
}

// Howard Hinnant's days → civil date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
	let z = z + 719_468;
	let era = z.div_euclid(146_097);
	let doe = z.rem_euclid(146_097);
	let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
	let y = yoe + era * 400;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
	let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
	(if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
	#[test]
	fn epoch_and_known_date() {
		assert_eq!(super::iso_from_ms(0), "1970-01-01T00:00:00Z");
		assert_eq!(super::iso_from_ms(1_791_993_600_000), "2026-10-14T16:00:00Z");
	}
}
