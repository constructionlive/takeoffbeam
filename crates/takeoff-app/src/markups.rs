//! The Markups List (bottom panel): every markup and quantity as a sortable,
//! filterable table, Bluebeam-style, with custom columns and CSV export.

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use takeoff_core::list::{rows, MarkupRow, RowKind};
use takeoff_core::{FieldDef, ItemRef, MarkStatus};

use crate::docstate::OpenDoc;
use crate::icons as i;
use crate::theme::{self, hex, Palette};
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortBy {
	Page,
	Space,
	Subject,
	Kind,
	Quantity,
	Author,
	Date,
	Status,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
	ThisPage,
	All,
}

pub struct MarkupsList {
	filter: String,
	sort: SortBy,
	ascending: bool,
	scope: Scope,
	only_pending: bool,
	new_column: String,
	/// Only rows in this space (or the spaces inside it).
	space: Option<String>,
	cache: Option<(u64, Vec<MarkupRow>)>,
	/// Last row clicked without Shift: where a Shift-click range starts.
	anchor: Option<String>,
}

impl Default for MarkupsList {
	fn default() -> Self {
		Self { filter: String::new(), sort: SortBy::Page, ascending: true, scope: Scope::All, only_pending: false, new_column: String::new(), space: None, cache: None, anchor: None }
	}
}

pub enum ListAction {
	ExportCsv,
	ExportSummary,
	ExportXlsx,
}

impl MarkupsList {
	pub fn ui(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette) -> Option<ListAction> {
		let mut action = None;
		let rev = doc.doc.lock().revision();
		if self.cache.as_ref().is_none_or(|(r, _)| *r != rev) {
			self.cache = Some((rev, rows(&doc.doc.lock())));
		}
		let all = self.cache.as_ref().unwrap().1.clone();
		let custom: Vec<FieldDef> = doc.doc.lock().data.custom_fields.clone();

		ui.horizontal_wrapped(|ui| {
			ui.label(RichText::new("Markups").font(theme::semibold(13.0)));
			ui.label(RichText::new(format!("{}", all.len())).size(12.0).color(p.muted));
			ui.add_space(12.0);
			ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(format!("{}  Filter subject, label, note…", i::FILTER)).desired_width(220.0));
			ui.selectable_value(&mut self.scope, Scope::All, "All pages");
			ui.selectable_value(&mut self.scope, Scope::ThisPage, "This page");
			ui.toggle_value(&mut self.only_pending, format!("{} To review", i::SPARKLES));
			let spaces: Vec<(String, String, usize)> = {
				let d = doc.doc.lock();
				let sp = takeoff_core::spaces::Spaces::new(&d.data);
				sp.tree().into_iter().map(|(s, depth)| (s.id.clone(), s.name.clone(), depth)).collect()
			};
			if self.space.as_ref().is_some_and(|f| !spaces.iter().any(|s| &s.0 == f)) {
				self.space = None;
			}
			if !spaces.is_empty() {
				let current = self.space.as_ref().and_then(|f| spaces.iter().find(|s| &s.0 == f)).map(|s| s.1.clone()).unwrap_or_else(|| "All spaces".into());
				egui::ComboBox::from_id_salt("list-space").selected_text(current).width(130.0).show_ui(ui, |ui| {
					ui.selectable_value(&mut self.space, None, "All spaces");
					for (id, name, depth) in &spaces {
						ui.selectable_value(&mut self.space, Some(id.clone()), format!("{}{name}", "   ".repeat(*depth)));
					}
				});
			}
			ui.add_space(12.0);
			{
				if ui.button(format!("{}  Summary CSV", i::SIGMA)).clicked() {
					action = Some(ListAction::ExportSummary);
				}
				if ui.button(format!("{}  Export CSV", i::FILE_SPREADSHEET)).clicked() {
					action = Some(ListAction::ExportCsv);
				}
				if ui.button(format!("{}  Excel", i::FILE_SPREADSHEET)).on_hover_text("The list and the quantity summary as an Excel workbook").clicked() {
					action = Some(ListAction::ExportXlsx);
				}
				ui.menu_button(format!("{}  Columns", i::TABLE), |ui| {
					widgets::section_label(ui, p, "Custom columns");
					for f in &custom {
						ui.horizontal(|ui| {
							ui.label(&f.name);
							if ui.small_button(i::X).on_hover_text("Remove column").clicked() {
								let id = f.id.clone();
								doc.doc.lock().edit("Remove column", |d| d.custom_fields.retain(|x| x.id != id));
							}
						});
					}
					ui.horizontal(|ui| {
						ui.add(egui::TextEdit::singleline(&mut self.new_column).hint_text("New column name").desired_width(140.0));
						if ui.button(i::PLUS).clicked() && !self.new_column.trim().is_empty() {
							let f = FieldDef { id: takeoff_core::new_id("f"), name: self.new_column.trim().to_string() };
							doc.doc.lock().edit("Add column", |d| d.custom_fields.push(f));
							self.new_column.clear();
						}
					});
				});
			}
		});

		let needle = self.filter.to_lowercase();
		let in_space: Option<std::collections::HashSet<String>> = self.space.as_ref().map(|f| {
			let d = doc.doc.lock();
			let sp = takeoff_core::spaces::Spaces::new(&d.data);
			d.data.spaces.iter().filter(|s| sp.is_within(Some(&s.id), f)).map(|s| s.id.clone()).collect()
		});
		let mut shown: Vec<MarkupRow> = all
			.into_iter()
			.filter(|r| self.scope == Scope::All || r.page == doc.page)
			.filter(|r| in_space.as_ref().is_none_or(|set| r.space_id.as_ref().is_some_and(|s| set.contains(s))))
			.filter(|r| !self.only_pending || r.status == "Pending")
			.filter(|r| needle.is_empty() || [&r.subject, &r.label, &r.note, &r.author, &r.status, &r.quantity, &r.space].iter().any(|s| s.to_lowercase().contains(&needle)) || r.fields.values().any(|v| v.to_lowercase().contains(&needle)))
			.collect();
		let key = |r: &MarkupRow| -> (String, f64) {
			match self.sort {
				SortBy::Page => (format!("{:06}{}{:06}", r.page, r.subject, r.number.unwrap_or(0)), 0.0),
				SortBy::Space => (format!("{}\u{0}{:06}{}{:06}", r.space, r.page, r.subject, r.number.unwrap_or(0)), 0.0),
				SortBy::Subject => (format!("{}{:06}", r.subject, r.number.unwrap_or(0)), 0.0),
				SortBy::Kind => (format!("{:?}{}", r.kind, r.subject), 0.0),
				SortBy::Quantity => (String::new(), r.value.unwrap_or(0.0)),
				SortBy::Author => (r.author.clone(), 0.0),
				SortBy::Date => (format!("{:020}", r.modified_ms), 0.0),
				SortBy::Status => (r.status.clone(), 0.0),
			}
		};
		shown.sort_by(|a, b| {
			let (ka, kb) = (key(a), key(b));
			ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
		});
		if !self.ascending {
			shown.reverse();
		}
		self.selection_bar(ui, doc, &shown, p);
		if shown.is_empty() {
			widgets::hint(ui, p, "No markups match. Everything you or the AI draw shows up here.");
			return action;
		}

		let mut sort_header = |ui: &mut egui::Ui, label: &str, by: SortBy| {
			let arrow = if self.sort == by { if self.ascending { format!(" {}", i::ARROW_UP) } else { format!(" {}", i::ARROW_DOWN) } } else { String::new() };
			if ui.add(egui::Button::new(RichText::new(format!("{label}{arrow}")).font(theme::semibold(11.5)).color(p.text2)).frame(false)).clicked() {
				if self.sort == by {
					self.ascending = !self.ascending;
				} else {
					self.sort = by;
					self.ascending = true;
				}
			}
		};
		let mut go_to: Option<(u32, ItemRef, bool)> = None;
		let mut clicked: Option<usize> = None;
		let text_h = 24.0;
		// Selectable labels would take the click; the row should get it wherever you click.
		ui.style_mut().interaction.selectable_labels = false;
		let mut table = TableBuilder::new(ui)
			.striped(true)
			.resizable(true)
			.sense(egui::Sense::click())
			.cell_layout(egui::Layout::left_to_right(egui::Align::Center))
			.column(Column::exact(44.0))
			.column(Column::initial(130.0).at_least(50.0))
			.column(Column::initial(90.0).at_least(60.0))
			.column(Column::initial(170.0).at_least(80.0))
			.column(Column::exact(36.0))
			.column(Column::initial(120.0).at_least(60.0))
			.column(Column::initial(110.0).at_least(60.0))
			.column(Column::initial(120.0).at_least(40.0))
			.column(Column::initial(90.0).at_least(40.0))
			.column(Column::initial(90.0).at_least(40.0))
			.column(Column::initial(80.0).at_least(40.0));
		for _ in &custom {
			table = table.column(Column::initial(100.0).at_least(40.0));
		}
		table = table.column(Column::remainder().at_least(80.0)).min_scrolled_height(0.0);
		table
			.header(22.0, |mut h| {
				h.col(|ui| sort_header(ui, "Page", SortBy::Page));
				h.col(|ui| sort_header(ui, "Space", SortBy::Space));
				h.col(|ui| sort_header(ui, "Type", SortBy::Kind));
				h.col(|ui| sort_header(ui, "Subject", SortBy::Subject));
				h.col(|ui| {
					ui.label(RichText::new("#").font(theme::semibold(11.5)).color(p.text2));
				});
				h.col(|ui| {
					ui.label(RichText::new("Label").font(theme::semibold(11.5)).color(p.text2));
				});
				h.col(|ui| sort_header(ui, "Quantity", SortBy::Quantity));
				h.col(|ui| {
					ui.label(RichText::new("Details").font(theme::semibold(11.5)).color(p.text2));
				});
				h.col(|ui| sort_header(ui, "Status", SortBy::Status));
				h.col(|ui| sort_header(ui, "Author", SortBy::Author));
				h.col(|ui| sort_header(ui, "Date", SortBy::Date));
				for f in &custom {
					h.col(|ui| {
						ui.label(RichText::new(&f.name).font(theme::semibold(11.5)).color(p.text2));
					});
				}
				h.col(|ui| {
					ui.label(RichText::new("Note").font(theme::semibold(11.5)).color(p.text2));
				});
			})
			.body(|body| {
				body.rows(text_h, shown.len(), |mut row| {
					let r = &shown[row.index()];
					let selected = doc.selection.contains(r.item.id());
					row.set_selected(selected);
					row.col(|ui| {
						ui.label(RichText::new(r.page.to_string()).color(p.text2));
					});
					row.col(|ui| {
						ui.add(egui::Label::new(RichText::new(&r.space).size(12.0)).truncate());
					});
					row.col(|ui| {
						let icon = match r.kind {
							RowKind::Count => i::HASH,
							RowKind::Length => i::RULER,
							RowKind::Perimeter => i::WAYPOINTS,
							RowKind::Area => i::PENTAGON,
							RowKind::Angle => i::TRIANGLE_RIGHT,
							RowKind::Annotation(_) => i::PEN_LINE,
						};
						ui.label(RichText::new(format!("{icon} {}", r.kind.label())).size(12.0));
					});
					row.col(|ui| {
						widgets::swatch(ui, hex(&r.color), 10.0);
						ui.add(egui::Label::new(RichText::new(&r.subject).size(12.5)).truncate());
					});
					row.col(|ui| {
						if let Some(n) = r.number {
							ui.label(RichText::new(n.to_string()).color(p.text2));
						}
					});
					row.col(|ui| {
						ui.add(egui::Label::new(RichText::new(&r.label).size(12.0)).truncate());
					});
					row.col(|ui| {
						ui.label(RichText::new(&r.quantity).font(theme::semibold(12.0)));
					});
					row.col(|ui| {
						ui.add(egui::Label::new(RichText::new(&r.extra).size(11.5).color(p.text2)).truncate());
					});
					row.col(|ui| {
						let color = match r.status.as_str() {
							"Pending" => p.accent,
							"Confirmed" | "Accepted" | "Completed" => p.success,
							"Rejected" => p.danger,
							_ => p.text2,
						};
						ui.label(RichText::new(&r.status).size(12.0).color(color));
					});
					row.col(|ui| {
						ui.add(egui::Label::new(RichText::new(&r.author).size(12.0)).truncate());
					});
					row.col(|ui| {
						ui.label(RichText::new(&r.date).size(12.0).color(p.text2));
					});
					for f in &custom {
						row.col(|ui| {
							ui.add(egui::Label::new(RichText::new(r.fields.get(&f.id).cloned().unwrap_or_default()).size(12.0)).truncate());
						});
					}
					row.col(|ui| {
						ui.add(egui::Label::new(RichText::new(&r.note).size(12.0).color(p.text2)).truncate());
					});
					let resp = row.response();
					if resp.clicked() {
						clicked = Some(row.index());
					}
					if resp.double_clicked() {
						go_to = Some((r.page, r.item.clone(), true));
					}
				});
			});
		if let Some(k) = clicked {
			let mods = ui.input(|i| i.modifiers);
			let id = shown[k].item.id().to_string();
			if mods.shift {
				// Range from the last plain/⌘ click to this row, added to the selection.
				let from = self.anchor.as_ref().and_then(|a| shown.iter().position(|r| r.item.id() == a)).unwrap_or(k);
				let (lo, hi) = (from.min(k), from.max(k));
				doc.selection.extend(shown[lo..=hi].iter().map(|r| r.item.id().to_string()));
			} else if mods.command || mods.ctrl {
				// Toggle one row.
				if !doc.selection.remove(&id) {
					doc.selection.insert(id.clone());
				}
				self.anchor = Some(id);
			} else {
				go_to = Some((shown[k].page, shown[k].item.clone(), false));
				self.anchor = Some(id);
			}
		}
		if let Some((page, item, zoom)) = go_to {
			if page != doc.page {
				doc.go_to_page(page);
			}
			doc.selection.clear();
			doc.selection.insert(item.id().to_string());
			if zoom {
				if let Some(b) = item_bounds(doc, &item) {
					doc.zoom_to(page, b);
				}
			}
		}
		action
	}

	/// Select all / clear, and what to do with the selection.
	fn selection_bar(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, shown: &[MarkupRow], p: &Palette) {
		let n = doc.selection.len();
		ui.horizontal_wrapped(|ui| {
			if ui.button(format!("{}  Select all ({})", i::SQUARE_DASHED, shown.len())).on_hover_text("Select every row shown (filters apply)").clicked() {
				doc.selection = shown.iter().map(|r| r.item.id().to_string()).collect();
			}
			if n == 0 {
				widgets::hint(ui, p, "Click a row to select · ⌘-click to add or remove · Shift-click for a range");
				return;
			}
			ui.label(RichText::new(format!("{n} selected")).font(theme::semibold(12.5)).color(p.accent));
			let pending = {
				let d = doc.doc.lock();
				d.data.marks.iter().filter(|m| doc.selection.contains(&m.id) && m.status == MarkStatus::Pending).count()
			};
			if pending > 0 && ui.button(format!("{}  Confirm {pending}", i::CHECK)).clicked() {
				let ids = doc.selection.clone();
				doc.doc.lock().edit("Confirm", |d| {
					for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
						m.status = MarkStatus::Confirmed;
					}
				});
			}
			if ui.button(RichText::new(format!("{}  Delete {n}", i::TRASH_2)).color(p.danger)).on_hover_text("Undo brings them back (⌘Z). Locked items are kept.").clicked() {
				let ids = doc.selection.clone();
				doc.doc.lock().edit("Delete", |d| d.delete_ids(&ids));
				doc.selection.clear();
			}
			if ui.button(format!("{}  Clear", i::X)).clicked() {
				doc.selection.clear();
			}
		});
	}
}

pub fn item_bounds(doc: &OpenDoc, item: &ItemRef) -> Option<(f64, f64, f64, f64)> {
	let d = doc.doc.lock();
	let pad = |b: (f64, f64, f64, f64)| (b.0 - 0.02, b.1 - 0.02, b.2 + 0.04, b.3 + 0.04);
	match item {
		ItemRef::Mark(id) => d.data.marks.iter().find(|m| &m.id == id).map(|m| pad((m.x, m.y, m.w, m.h))),
		ItemRef::Measurement(id) => d.data.measurements.iter().find(|m| &m.id == id).and_then(|m| takeoff_core::geometry::bounds(&m.points)).map(pad),
		ItemRef::Annotation(id) => d.data.annotations.iter().find(|a| &a.id == id).and_then(|a| takeoff_core::geometry::bounds(&a.points)).map(pad),
	}
}
