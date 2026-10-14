//! PDF layers (optional content groups).
//!
//! pdfium renders optional content according to the document's default
//! configuration (`/OCProperties /D`) but has no public API to change it. So
//! to hide layers we append a tiny incremental update that rewrites the
//! catalog's `/OCProperties` with the wanted `/OFF` list, and load
//! "original file + update" into pdfium from memory. The original file is
//! never modified, and the update is a few KB even for a 700 MB set.

use std::collections::HashSet;
use std::path::Path;

use crate::cos::{incremental_update, key, text_string, CosError, CosFile, Obj};

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
	pub name: String,
	/// Hidden in the document's own default view.
	pub default_off: bool,
	obj: (u32, u16),
}

pub struct LayerInfo {
	pub layers: Vec<Layer>,
	file: CosFile,
	root: (u32, u16),
	catalog: crate::cos::Dict,
	ocprops: crate::cos::Dict,
}

impl LayerInfo {
	/// Reads the layer list. `Ok(None)` when the PDF has no layers.
	pub fn read(path: &Path) -> Result<Option<Self>, CosError> {
		let file = CosFile::open(path)?;
		let Some(root) = file.root_ref() else { return Ok(None) };
		let Some(catalog) = file.get(root.0)?.as_dict().cloned() else { return Ok(None) };
		let Some(oc) = key(&catalog, "OCProperties") else { return Ok(None) };
		let Some(ocprops) = file.resolve(oc)?.as_dict().cloned() else { return Ok(None) };
		let ocgs: Vec<(u32, u16)> = match key(&ocprops, "OCGs").map(|o| file.resolve(o)) {
			Some(Ok(Obj::Array(a))) => a.iter().filter_map(Obj::as_ref).collect(),
			_ => vec![],
		};
		let default = key(&ocprops, "D").map(|d| file.resolve(d)).transpose()?.and_then(|d| d.as_dict().cloned()).unwrap_or_default();
		let refs = |name: &str| -> HashSet<(u32, u16)> {
			match key(&default, name).map(|o| file.resolve(o)) {
				Some(Ok(Obj::Array(a))) => a.iter().filter_map(Obj::as_ref).collect(),
				_ => HashSet::new(),
			}
		};
		let base_off = key(&default, "BaseState").and_then(Obj::as_name) == Some(b"OFF");
		let (off, on) = (refs("OFF"), refs("ON"));
		let mut layers = Vec::new();
		for r in ocgs {
			let Some(d) = file.get(r.0)?.as_dict().cloned() else { continue };
			let name = match key(&d, "Name") {
				Some(Obj::Str(s)) => text_string(s),
				_ => format!("Layer {}", r.0),
			};
			let default_off = if base_off { !on.contains(&r) } else { off.contains(&r) };
			layers.push(Layer { name, default_off, obj: r });
		}
		if layers.is_empty() {
			return Ok(None);
		}
		Ok(Some(Self { layers, file, root, catalog, ocprops }))
	}

	/// Layer names, de-duplicated (CAD exports often repeat a name per sheet;
	/// toggling a name toggles every group with it, like the platform does).
	pub fn names(&self) -> Vec<String> {
		let mut seen = HashSet::new();
		self.layers.iter().filter(|l| seen.insert(l.name.clone())).map(|l| l.name.clone()).collect()
	}

	/// Bytes to append so exactly the layers named in `hidden` are off.
	pub fn update_hiding(&self, hidden: &HashSet<String>) -> Vec<u8> {
		let off: Vec<Obj> = self.layers.iter().filter(|l| hidden.contains(&l.name)).map(|l| Obj::Ref(l.obj.0, l.obj.1)).collect();
		let mut d = crate::cos::Dict::new();
		d.insert(b"BaseState".to_vec(), Obj::Name(b"ON".to_vec()));
		d.insert(b"OFF".to_vec(), Obj::Array(off));
		// Keep the panel order and radio groups for other readers.
		if let Some(Ok(Some(orig))) = key(&self.ocprops, "D").map(|o| self.file.resolve(o).map(|o| o.as_dict().cloned())) {
			for k in ["Order", "RBGroups", "Name", "Locked"] {
				if let Some(v) = key(&orig, k) {
					d.insert(k.as_bytes().to_vec(), v.clone());
				}
			}
		}
		let mut ocprops = self.ocprops.clone();
		ocprops.insert(b"D".to_vec(), Obj::Dict(d));
		// Usage-based auto states could override /D; drop them.
		ocprops.remove(b"AS".as_slice());
		let mut catalog = self.catalog.clone();
		catalog.insert(b"OCProperties".to_vec(), Obj::Dict(ocprops));
		incremental_update(&self.file, &[(self.root.0, Obj::Dict(catalog))])
	}

	pub fn mmap(&self) -> std::sync::Arc<memmap2::Mmap> {
		self.file.mmap()
	}
}
