//! Right panel tabs: Takeoff (items, totals, scale, tool chest) and
//! Properties (the selection, or the defaults for the current markup tool).

use eframe::egui::{self, Color32, RichText, ScrollArea};
use takeoff_core::units::{self, format_quantity, group_thousands, ScalePreset};
use takeoff_core::*;

use crate::canvas::Canvas;
use crate::docstate::OpenDoc;
use crate::icons as i;
use crate::settings::Settings;
use crate::theme::{self, hex, to_hex, Palette};
use crate::tools::Tool;
use crate::widgets;

pub const SWATCHES: [&str; 12] = ["#E8531A", "#F87171", "#FBBF24", "#A3E635", "#34D399", "#22D3EE", "#4CC2FF", "#3B82F6", "#A78BFA", "#F472B6", "#64748B", "#111827"];

#[derive(Default)]
pub struct RightPanel {
	pub tab: usize,
	renaming: Option<(String, String)>,
	pub spaces: crate::spaces::SpacesPanel,
}

pub(crate) fn color_row(ui: &mut egui::Ui, current: &str) -> Option<String> {
	let mut picked = None;
	ui.horizontal_wrapped(|ui| {
		ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
		for c in SWATCHES {
			let r = widgets::swatch(ui, hex(c), 18.0);
			if c.eq_ignore_ascii_case(current) {
				ui.painter().rect_stroke(r.rect.expand(2.0), 4.0, egui::Stroke::new(1.5, ui.visuals().text_color()), egui::StrokeKind::Outside);
			}
			if r.clicked() {
				picked = Some(c.to_string());
			}
		}
		let mut col = hex(current);
		if ui.color_edit_button_srgba(&mut col).changed() {
			picked = Some(to_hex(col));
		}
	});
	picked
}

impl RightPanel {
	// ── Takeoff tab ─────────────────────────────────────────────────────

	pub fn takeoff(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, canvas: &mut Canvas, settings: &mut Settings, p: &Palette) {
		ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
			self.scale_card(ui, doc, canvas, p);
			ui.add_space(10.0);
			self.items(ui, doc, canvas, settings, p);
			self.spaces.ui(ui, doc, canvas, p);
			crate::legends::panel(ui, doc, canvas, p);
			self.overlays(ui, doc, p);
		});
	}

	/// Pictures laid over the sheet (usually by the AI): show/hide, fade, delete.
	fn overlays(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, p: &Palette) {
		let list: Vec<Overlay> = doc.doc.lock().data.overlays.clone();
		if list.is_empty() {
			return;
		}
		ui.add_space(10.0);
		widgets::section_label(ui, p, "Overlays");
		let mut go_to = None;
		for o in &list {
			ui.horizontal(|ui| {
				let (icon, tip) = if o.hidden { (i::EYE_OFF, "Show") } else { (i::EYE, "Hide") };
				if widgets::icon_button(ui, p, icon, tip, false).clicked() {
					let id = o.id.clone();
					doc.doc.lock().edit(if o.hidden { "Show overlay" } else { "Hide overlay" }, |d| {
						if let Some(o) = d.overlays.iter_mut().find(|x| x.id == id) {
							o.hidden = !o.hidden;
						}
					});
				}
				let name = o.label.clone().unwrap_or_else(|| "Overlay".into());
				let here = o.page == doc.page;
				let text = RichText::new(format!("{name} · p{}", o.page)).size(12.5).color(if here { p.text } else { p.text2 });
				if ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click())).on_hover_text(if here { "On this page" } else { "Go to its page" }).clicked() && !here {
					go_to = Some(o.page);
				}
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if widgets::icon_button(ui, p, i::TRASH_2, "Delete overlay", false).clicked() {
						let id = o.id.clone();
						doc.doc.lock().edit("Delete overlay", |d| d.overlays.retain(|x| x.id != id));
					}
					let mut op = o.opacity as f32;
					let r = ui.add(egui::Slider::new(&mut op, 0.05..=1.0).show_value(false)).on_hover_text(format!("Opacity {:.0}%", op * 100.0));
					// One undo step per drag.
					if r.drag_started() {
						doc.doc.lock().begin_gesture("Overlay opacity");
					}
					if r.changed() {
						let id = o.id.clone();
						doc.doc.lock().edit("Overlay opacity", |d| {
							if let Some(o) = d.overlays.iter_mut().find(|x| x.id == id) {
								o.opacity = op as f64;
							}
						});
					}
					if r.drag_stopped() {
						doc.doc.lock().end_gesture();
					}
				});
			});
		}
		if let Some(pg) = go_to {
			doc.go_to_page(pg);
		}
	}

	fn scale_card(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, canvas: &mut Canvas, p: &Palette) {
		let page = doc.page;
		let (w_pt, _) = doc.page_size(page);
		let (scale, viewports, display) = {
			let d = doc.doc.lock();
			(d.data.scale_for(page).cloned(), d.data.viewports.iter().filter(|v| v.page == page).count(), d.data.display.clone())
		};
		widgets::card(ui, p, |ui| {
			ui.horizontal(|ui| {
				ui.label(RichText::new(i::SCALING).font(theme::icon_font(15.0)).color(if scale.is_some() { p.success } else { p.warning }));
				ui.label(RichText::new("Page scale").font(theme::semibold(13.0)));
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if ui.add(egui::Button::new(RichText::new(format!("{}  Calibrate", i::RULER)).size(12.0))).on_hover_text("Click both ends of a known dimension (K)").clicked() {
						canvas.set_tool(Tool::Calibrate);
					}
				});
			});
			let text = match &scale {
				Some(s) => describe_scale(s, w_pt),
				None => "Not set — lengths and areas show “no scale”".into(),
			};
			ui.label(RichText::new(text).color(if scale.is_some() { p.text } else { p.warning }).size(12.5));
			if viewports > 0 {
				widgets::hint(ui, p, &format!("{viewports} viewport(s) on this page use their own scale"));
			}
			ui.horizontal(|ui| {
				egui::ComboBox::from_id_salt("preset").selected_text("Preset scale").width(150.0).show_ui(ui, |ui| {
					for (title, list) in [("Architectural", &units::ARCHITECTURAL[..]), ("Engineering", &units::ENGINEERING[..]), ("Metric", &units::METRIC[..])] {
						ui.label(RichText::new(title).font(theme::semibold(11.0)).color(p.muted));
						for preset in list {
							if ui.selectable_label(false, preset.label).clicked() {
								let s = preset.to_page_scale(page, w_pt);
								doc.doc.lock().edit("Set scale", |d| {
									d.page_scales.retain(|x| x.page != page);
									d.page_scales.push(s);
								});
							}
						}
						ui.separator();
					}
				});
				if scale.is_some() && ui.small_button("Apply to all pages").on_hover_text("Same paper size pages only").clicked() {
					let s = scale.clone().unwrap();
					let sizes = doc.page_sizes.clone();
					let here = doc.page_size(page);
					doc.doc.lock().edit("Scale all pages", |d| {
						for (idx, sz) in sizes.iter().enumerate() {
							let pg = idx as u32 + 1;
							if *sz == here && pg != page {
								d.page_scales.retain(|x| x.page != pg);
								d.page_scales.push(PageScale { page: pg, ..s.clone() });
							}
						}
					});
				}
			});
			ui.horizontal(|ui| {
				ui.label(RichText::new("Show lengths").size(12.0).color(p.text2));
				let mut fmt = display.length_format;
				let mut prec = display.precision;
				let changed = ui.selectable_value(&mut fmt, LengthFormat::Decimal, "12.50 ft").changed() | ui.selectable_value(&mut fmt, LengthFormat::FeetInches, "12'-6\"").changed();
				let changed_p = fmt == LengthFormat::Decimal && ui.add(egui::DragValue::new(&mut prec).range(0..=4).suffix(" dp")).changed();
				if changed || changed_p {
					doc.doc.lock().edit("Display units", |d| {
						d.display.length_format = fmt;
						d.display.precision = prec;
					});
				}
			});
		});
	}

	fn items(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, canvas: &mut Canvas, settings: &mut Settings, p: &Palette) {
		let lines = crate::spaces::summary_for(&doc.doc.lock(), self.spaces.filter.as_deref());
		ui.horizontal(|ui| {
			widgets::section_label(ui, p, "Takeoff items");
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.menu_button(RichText::new(format!("{}  New", i::PLUS)).size(12.5), |ui| {
					for (kind, label) in [(CategoryKind::Count, "Count item"), (CategoryKind::Length, "Length item"), (CategoryKind::Area, "Area item")] {
						if ui.button(format!("{}  {label}", widgets::kind_icon(kind))).clicked() {
							let id = new_id("c");
							let mut d = doc.doc.lock();
							let n = d.data.categories.iter().filter(|c| c.kind() == kind).count() + 1;
							let color = d.data.next_category_color().to_string();
							let name = format!("{} {n}", match kind { CategoryKind::Count => "Count", CategoryKind::Length => "Length", CategoryKind::Area => "Area" });
							d.edit("New item", |t| t.categories.push(Category::new(id.clone(), name.clone(), color, kind)));
							drop(d);
							canvas.active_category = Some(id.clone());
							canvas.set_tool(match kind { CategoryKind::Count => Tool::Count, CategoryKind::Length => Tool::Length, CategoryKind::Area => if matches!(canvas.tool, Tool::Fill | Tool::RectArea | Tool::Volume) { canvas.tool } else { Tool::Area } });
							self.renaming = Some((id, name));
							ui.close();
						}
					}
					if !settings.tool_chest.is_empty() {
						ui.separator();
						ui.label(RichText::new("From Tool Chest").font(theme::semibold(11.0)).color(p.muted));
						for c in settings.tool_chest.clone() {
							if ui.button(format!("{}  {}", widgets::kind_icon(c.kind()), c.name)).clicked() {
								let mut c = c.clone();
								c.id = new_id("c");
								let id = c.id.clone();
								let kind = c.kind();
								doc.doc.lock().edit("Add from Tool Chest", |t| t.categories.push(c));
								canvas.active_category = Some(id);
								canvas.set_tool(match kind { CategoryKind::Count => Tool::Count, CategoryKind::Length => Tool::Length, CategoryKind::Area => Tool::Area });
								ui.close();
							}
						}
					}
				});
			});
		});
		self.spaces.filter_note(ui, doc, p);
		if lines.is_empty() {
			widgets::empty_state(ui, p, i::SIGMA, "No quantities yet", "Pick Count, Length or Area and click on the drawing — or ask the AI to count for you.");
			return;
		}
		let mut total_cost = 0.0;
		let mut any_cost = false;
		for line in &lines {
			let Some(cid) = line.category_id.clone() else {
				// Ungrouped items.
				widgets::card(ui, p, |ui| {
					ui.horizontal(|ui| {
						widgets::swatch(ui, hex(&line.color), 12.0);
						ui.label(RichText::new("Ungrouped").font(theme::semibold(12.5)));
						ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(&line.total_text).font(theme::semibold(12.5))));
					});
				});
				continue;
			};
			let (cat, active) = {
				let d = doc.doc.lock();
				// Highlighted only while a takeoff tool is actually recording into it.
				(d.data.category(&cid).cloned(), canvas.active_category.as_deref() == Some(cid.as_str()) && canvas.tool.category_kind().is_some())
			};
			let Some(cat) = cat else { continue };
			if let Some(c) = line.cost {
				total_cost += c;
				any_cost = true;
			}
			let frame = egui::Frame::new()
				.fill(if active { if p.dark { Color32::from_rgb(0x24, 0x1C, 0x1A) } else { Color32::from_rgb(0xFF, 0xF4, 0xEF) } } else { p.card })
				.stroke(egui::Stroke::new(1.0, if active { p.accent } else { p.border }))
				.corner_radius(4)
				.inner_margin(egui::Margin::symmetric(10, 8));
			// The card's own click sense is registered *before* its children, so the
			// eye / ⋯ buttons and the rename field on top of it still get their clicks.
			let mut name_clicked = false;
			let resp = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| frame.show(ui, |ui| {
				ui.horizontal(|ui| {
					let sw = widgets::swatch(ui, hex(&cat.color), 14.0);
					sw.on_hover_text("Color");
					ui.label(RichText::new(widgets::kind_icon(cat.kind())).font(theme::icon_font(13.0)).color(p.text2));
					if let Some((rid, buf)) = &mut self.renaming {
						if *rid == cid {
							let r = ui.add(egui::TextEdit::singleline(buf).desired_width(120.0));
							r.request_focus();
							if r.lost_focus() {
								let (id, name) = self.renaming.take().unwrap();
								if !name.trim().is_empty() {
									doc.doc.lock().edit("Rename item", |d| {
										if let Some(c) = d.categories.iter_mut().find(|c| c.id == id) {
											c.name = name.trim().to_string();
										}
									});
								}
							}
						} else {
							ui.label(RichText::new(&cat.name).font(theme::semibold(12.5)));
						}
					} else {
						let l = ui.add(egui::Label::new(RichText::new(&cat.name).font(theme::semibold(12.5))).truncate().sense(egui::Sense::click()));
						if l.double_clicked() {
							self.renaming = Some((cid.clone(), cat.name.clone()));
						} else if l.clicked() {
							name_clicked = true;
						}
					}
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						ui.menu_button(RichText::new(i::ELLIPSIS).font(theme::icon_font(14.0)), |ui| {
							ui.set_min_width(220.0);
							widgets::section_label(ui, p, "Color");
							if let Some(c) = color_row(ui, &cat.color) {
								doc.doc.lock().edit("Item color", |d| {
									if let Some(x) = d.categories.iter_mut().find(|x| x.id == cid) {
										x.color = c;
									}
								});
							}
							ui.add_space(6.0);
							widgets::section_label(ui, p, "Style");
							let mut c2 = cat.clone();
							let mut changed = false;
							if cat.kind() == CategoryKind::Length {
								ui.horizontal(|ui| {
									for (s, l) in [(None, "Solid"), (Some(LineStyle::Dashed), "Dashed"), (Some(LineStyle::Dotted), "Dotted")] {
										changed |= ui.selectable_value(&mut c2.line_style, s, l).changed();
									}
								});
							}
							if cat.kind() == CategoryKind::Area {
								ui.horizontal_wrapped(|ui| {
									for (s, l) in [(None, "Tint"), (Some(Hatch::Diagonal), "Diagonal"), (Some(Hatch::Cross), "Cross"), (Some(Hatch::Grid), "Grid")] {
										changed |= ui.selectable_value(&mut c2.hatch, s, l).changed();
									}
								});
							}
							let mut op = c2.opacity.unwrap_or(1.0) as f32;
							if ui.add(egui::Slider::new(&mut op, 0.15..=1.0).text("Opacity")).changed() {
								c2.opacity = Some(op as f64);
								changed = true;
							}
							ui.add_space(4.0);
							widgets::section_label(ui, p, "Cost");
							let mut uc = c2.unit_cost.unwrap_or(0.0);
							let unit = match cat.kind() { CategoryKind::Count => "ea".to_string(), _ => line.unit.clone() };
							ui.horizontal(|ui| {
								ui.label("$");
								if ui.add(egui::DragValue::new(&mut uc).speed(0.1).range(0.0..=1e9).max_decimals(2)).changed() {
									c2.unit_cost = (uc > 0.0).then_some(uc);
									changed = true;
								}
								ui.label(format!("per {}", if unit.is_empty() { "unit".into() } else { unit }));
							});
							ui.add_space(4.0);
							widgets::section_label(ui, p, "Describe it for the AI");
							let mut desc = c2.description.clone().unwrap_or_default();
							if ui.add(egui::TextEdit::multiline(&mut desc).desired_rows(2).hint_text("e.g. duplex receptacle, circle with two lines")).changed() {
								c2.description = (!desc.trim().is_empty()).then_some(desc);
								changed = true;
							}
							if changed {
								doc.doc.lock().edit("Item style", |d| {
									if let Some(x) = d.categories.iter_mut().find(|x| x.id == cid) {
										*x = c2.clone();
									}
								});
							}
							ui.separator();
							if ui.button(format!("{}  Rename", i::PENCIL)).clicked() {
								self.renaming = Some((cid.clone(), cat.name.clone()));
								ui.close();
							}
							if ui.button(format!("{}  Save to Tool Chest", i::BOOKMARK)).clicked() {
								let mut c = cat.clone();
								c.legend_crop = None;
								c.legend_region = None;
								settings.tool_chest.retain(|x| x.name != c.name);
								settings.tool_chest.push(c);
								settings.save();
								ui.close();
							}
							if ui.button(format!("{}  Select all on page", i::MOUSE_POINTER_2)).clicked() {
								let d = doc.doc.lock();
								let ids: Vec<String> = d.data.marks.iter().filter(|m| m.page == doc.page && m.category_id.as_deref() == Some(&cid)).map(|m| m.id.clone())
									.chain(d.data.measurements.iter().filter(|m| m.page == doc.page && m.category_id.as_deref() == Some(&cid)).map(|m| m.id.clone()))
									.collect();
								drop(d);
								doc.selection = ids.into_iter().collect();
								ui.close();
							}
							if ui.button(RichText::new(format!("{}  Delete item and its quantities", i::TRASH_2)).color(p.danger)).clicked() {
								doc.doc.lock().edit("Delete item", |d| {
									d.categories.retain(|x| x.id != cid);
									d.marks.retain(|m| m.category_id.as_deref() != Some(&cid));
									d.measurements.retain(|m| m.category_id.as_deref() != Some(&cid));
								});
								if canvas.active_category.as_deref() == Some(&cid) {
									canvas.active_category = None;
								}
								ui.close();
							}
						});
						let eye = if cat.hidden { i::EYE_OFF } else { i::EYE };
						if widgets::icon_button(ui, p, eye, if cat.hidden { "Show" } else { "Hide" }, false).clicked() {
							doc.doc.lock().edit("Visibility", |d| {
								if let Some(x) = d.categories.iter_mut().find(|x| x.id == cid) {
									x.hidden = !x.hidden;
								}
							});
						}
					});
				});
				ui.horizontal(|ui| {
					ui.add_space(22.0);
					ui.label(RichText::new(group_thousands(&line.total_text)).font(theme::bold(15.0)).color(p.text));
					if line.kind != CategoryKind::Count {
						ui.label(RichText::new(format!("· {} shapes", line.items)).size(11.5).color(p.text2));
					}
					if let Some(v) = line.volume {
						ui.label(RichText::new(format!("· {}", format_quantity(v, &format!("{}³", line.unit.trim_end_matches('²')), &Default::default()))).size(11.5).color(p.text2));
					}
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						if let Some(c) = line.cost {
							ui.label(RichText::new(format!("${}", group_thousands(&format!("{c:.2}")))).font(theme::semibold(12.5)).color(p.text2));
						}
					});
				});
				if line.pending > 0 || line.warning.is_some() {
					ui.horizontal(|ui| {
						ui.add_space(22.0);
						if line.pending > 0 {
							ui.label(RichText::new(format!("{} {} to review", i::SPARKLES, line.pending)).size(11.5).color(p.accent));
							if ui.small_button("Confirm all").clicked() {
								doc.doc.lock().edit("Confirm AI marks", |d| {
									for m in d.marks.iter_mut().filter(|m| m.category_id.as_deref() == Some(&cid)) {
										m.status = MarkStatus::Confirmed;
									}
								});
							}
						}
						if let Some(w) = &line.warning {
							ui.label(RichText::new(format!("{} {w}", i::CIRCLE_ALERT)).size(11.5).color(p.warning));
						}
					});
				}
			}));
			let r = resp.response;
			if r.clicked() || name_clicked {
				if active {
					// Second click: stop taking off this item.
					canvas.active_category = None;
					canvas.set_tool(Tool::Select);
				} else {
					canvas.active_category = Some(cid.clone());
					canvas.set_tool(match cat.kind() { CategoryKind::Count => Tool::Count, CategoryKind::Length => if matches!(canvas.tool, Tool::Perimeter | Tool::Curve | Tool::Trace) { canvas.tool } else { Tool::Length }, CategoryKind::Area => Tool::Area });
				}
			}
			r.on_hover_text(if active { "Click again to stop (or press Esc)" } else { "Click to take off this item" });
			ui.add_space(4.0);
		}
		if any_cost {
			ui.add_space(6.0);
			widgets::card(ui, p, |ui| {
				ui.horizontal(|ui| {
					ui.label(RichText::new("Estimated cost").font(theme::semibold(13.0)));
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						ui.label(RichText::new(format!("${}", group_thousands(&format!("{total_cost:.2}")))).font(theme::bold(15.0)).color(p.accent));
					});
				});
			});
		}
	}

	// ── Properties tab ──────────────────────────────────────────────────

	pub fn properties(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, canvas: &mut Canvas, p: &Palette) {
		ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
			let n = doc.selection.len();
			if n == 0 {
				if let Tool::Annot(kind) = canvas.tool {
					widgets::section_label(ui, p, &format!("{} defaults", kind.label()));
					style_editor(ui, p, &mut canvas.annot_style, kind, &mut canvas.highlight_color, &mut canvas.stamp_text);
				} else {
					widgets::empty_state(ui, p, i::MOUSE_POINTER_2, "Nothing selected", "Select a markup to see and edit its details.");
				}
				return;
			}
			if n > 1 {
				ui.label(RichText::new(format!("{n} items selected")).font(theme::semibold(14.0)));
				ui.add_space(6.0);
				if ui.button(format!("{}  Confirm AI marks", i::CHECK)).clicked() {
					let ids = doc.selection.clone();
					doc.doc.lock().edit("Confirm", |d| {
						for m in d.marks.iter_mut().filter(|m| ids.contains(&m.id)) {
							m.status = MarkStatus::Confirmed;
						}
					});
				}
				if ui.button(RichText::new(format!("{}  Delete", i::TRASH_2)).color(p.danger)).clicked() {
					canvas.delete_selection(doc);
				}
				return;
			}
			let id = doc.selection.iter().next().unwrap().clone();
			let item = doc.doc.lock().data.find(&id);
			match item {
				Some(ItemRef::Mark(_)) => self.mark_props(ui, doc, &id, p),
				Some(ItemRef::Measurement(_)) => self.measurement_props(ui, doc, &id, p, canvas),
				Some(ItemRef::Annotation(_)) => self.annotation_props(ui, doc, &id, p, canvas),
				None => {
					if !crate::legends::props(ui, doc, &id, p) && !crate::spaces::props(ui, doc, &id, p) {
						doc.selection.clear();
					}
				}
			}
		});
	}

	fn common_meta(&self, ui: &mut egui::Ui, p: &Palette, meta: &ItemMeta, source: &str) {
		egui::Grid::new("meta").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
			ui.label(RichText::new("Author").color(p.text2));
			ui.label(meta.author.clone().unwrap_or_else(|| source.to_string()));
			ui.end_row();
			if let Some(c) = meta.created_at {
				ui.label(RichText::new("Created").color(p.text2));
				ui.label(takeoff_core::time::iso_from_ms(c).replace('T', " ").trim_end_matches('Z').to_string());
				ui.end_row();
			}
		});
	}

	fn category_picker(&self, ui: &mut egui::Ui, doc: &OpenDoc, kind: CategoryKind, current: &Option<String>) -> Option<Option<String>> {
		let cats: Vec<(String, String, String)> = doc.doc.lock().data.categories.iter().filter(|c| c.kind() == kind).map(|c| (c.id.clone(), c.name.clone(), c.color.clone())).collect();
		let name = current.as_ref().and_then(|c| cats.iter().find(|x| &x.0 == c)).map(|x| x.1.clone()).unwrap_or_else(|| "Ungrouped".into());
		let mut out = None;
		egui::ComboBox::from_id_salt("cat").selected_text(name).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
			if ui.selectable_label(current.is_none(), "Ungrouped").clicked() {
				out = Some(None);
			}
			for (cid, n, _) in &cats {
				if ui.selectable_label(current.as_deref() == Some(cid), n).clicked() {
					out = Some(Some(cid.clone()));
				}
			}
		});
		out
	}

	fn mark_props(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, id: &str, p: &Palette) {
		let Some(m) = doc.doc.lock().data.marks.iter().find(|m| m.id == id).cloned() else { return };
		header(ui, p, i::HASH, "Count mark", if m.source == Source::Ai { "Found by AI" } else { "Placed by you" });
		widgets::section_label(ui, p, "Item");
		if let Some(c) = self.category_picker(ui, doc, CategoryKind::Count, &m.category_id) {
			doc.doc.lock().edit("Change item", |d| {
				if let Some(x) = d.marks.iter_mut().find(|x| x.id == id) {
					x.category_id = c;
				}
			});
		}
		ui.add_space(6.0);
		ui.horizontal(|ui| {
			let pending = m.status == MarkStatus::Pending;
			ui.label(RichText::new(if pending { "Pending review" } else { "Confirmed" }).color(if pending { p.accent } else { p.success }));
			if pending && widgets::primary_button(ui, p, "Confirm").clicked() {
				doc.doc.lock().edit("Confirm", |d| {
					if let Some(x) = d.marks.iter_mut().find(|x| x.id == id) {
						x.status = MarkStatus::Confirmed;
					}
				});
			}
			if !pending && ui.small_button("Mark pending").clicked() {
				doc.doc.lock().edit("Status", |d| {
					if let Some(x) = d.marks.iter_mut().find(|x| x.id == id) {
						x.status = MarkStatus::Pending;
					}
				});
			}
		});
		note_editor(ui, p, doc, id, m.note.clone());
		fields_editor(ui, p, doc, id, m.fields.clone().unwrap_or_default());
		ui.add_space(8.0);
		self.common_meta(ui, p, &m.meta, if m.source == Source::Ai { "AI" } else { "" });
	}

	fn measurement_props(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, id: &str, p: &Palette, canvas: &mut Canvas) {
		let (m, q, display) = {
			let d = doc.doc.lock();
			let Some(m) = d.data.measurements.iter().find(|m| m.id == id).cloned() else { return };
			let q = d.quantity(&m);
			(m, q, d.data.display.clone())
		};
		let (icon, title) = match m.kind {
			MeasurementType::Length if m.closed => (i::WAYPOINTS, "Perimeter"),
			MeasurementType::Length => (i::RULER, "Length"),
			MeasurementType::Area => (i::PENTAGON, "Area"),
			MeasurementType::Angle => (i::TRIANGLE_RIGHT, "Angle"),
		};
		header(ui, p, icon, title, if m.source == Some(Source::Ai) { "Measured by AI" } else { "Measured by you" });
		widgets::card(ui, p, |ui| {
			match &q {
				Some(q) => {
					ui.label(RichText::new(group_thousands(&format_quantity(q.value, &q.unit, &display))).font(theme::bold(20.0)));
					if let Some(per) = q.perimeter {
						ui.label(RichText::new(format!("Perimeter {}", format_quantity(per, &q.base_unit, &display))).color(p.text2));
					}
					if let Some(v) = q.volume {
						ui.label(RichText::new(format!("Volume {}", format_quantity(v, &format!("{}³", q.base_unit), &display))).color(p.text2));
						if q.base_unit == "ft" {
							ui.label(RichText::new(format!("= {:.2} yd³", v / 27.0)).color(p.text2));
						}
					}
				}
				None => {
					ui.label(RichText::new("No scale on this page").color(p.warning));
					if ui.button("Calibrate…").clicked() {
						canvas.set_tool(Tool::Calibrate);
					}
				}
			}
			ui.label(RichText::new(format!("{} vertices{}", m.points.len(), if m.cutouts.is_empty() { String::new() } else { format!(" · {} cutout(s)", m.cutouts.len()) })).size(11.5).color(p.muted));
		});
		if m.kind != MeasurementType::Angle {
			ui.add_space(6.0);
			widgets::section_label(ui, p, "Item");
			let kind = if m.kind == MeasurementType::Area { CategoryKind::Area } else { CategoryKind::Length };
			if let Some(c) = self.category_picker(ui, doc, kind, &m.category_id) {
				doc.doc.lock().edit("Change item", |d| {
					if let Some(x) = d.measurements.iter_mut().find(|x| x.id == id) {
						x.category_id = c;
					}
				});
			}
		}
		if m.kind == MeasurementType::Area {
			ui.add_space(6.0);
			widgets::section_label(ui, p, "Depth (for volume)");
			let mut depth = m.depth.unwrap_or(0.0);
			let unit = q.as_ref().map(|q| q.base_unit.clone()).unwrap_or_default();
			ui.horizontal(|ui| {
				if ui.add(egui::DragValue::new(&mut depth).speed(0.05).range(0.0..=1e6).max_decimals(3)).changed() {
					doc.doc.lock().edit("Depth", |d| {
						if let Some(x) = d.measurements.iter_mut().find(|x| x.id == id) {
							x.depth = (depth > 0.0).then_some(depth);
						}
					});
				}
				ui.label(unit);
				if ui.small_button(format!("{} Add cutout", i::SCISSORS)).clicked() {
					canvas.set_tool(Tool::Cutout);
				}
			});
		}
		if m.kind == MeasurementType::Length {
			let mut closed = m.closed;
			if ui.checkbox(&mut closed, "Closed (perimeter)").changed() {
				doc.doc.lock().edit("Perimeter", |d| {
					if let Some(x) = d.measurements.iter_mut().find(|x| x.id == id) {
						x.closed = closed;
					}
				});
			}
		}
		ui.add_space(6.0);
		widgets::section_label(ui, p, "Label");
		let mut label = m.label.clone().unwrap_or_default();
		if ui.add(egui::TextEdit::singleline(&mut label).desired_width(f32::INFINITY).hint_text("e.g. Corridor 2F")).changed() {
			doc.doc.lock().edit("Label", |d| {
				if let Some(x) = d.measurements.iter_mut().find(|x| x.id == id) {
					x.label = (!label.trim().is_empty()).then_some(label.clone());
				}
			});
		}
		note_editor(ui, p, doc, id, m.note.clone());
		fields_editor(ui, p, doc, id, m.fields.clone().unwrap_or_default());
		ui.add_space(8.0);
		self.common_meta(ui, p, &m.meta, if m.source == Some(Source::Ai) { "AI" } else { "" });
	}

	fn annotation_props(&mut self, ui: &mut egui::Ui, doc: &mut OpenDoc, id: &str, p: &Palette, canvas: &mut Canvas) {
		let Some(a) = doc.doc.lock().data.annotations.iter().find(|a| a.id == id).cloned() else { return };
		header(ui, p, Tool::Annot(a.kind).icon(), a.kind.label(), if a.source == Some(Source::Ai) { "Added by AI" } else { "Markup" });
		if matches!(a.kind, AnnotationKind::Text | AnnotationKind::Callout | AnnotationKind::Stamp) {
			widgets::section_label(ui, p, "Text");
			let mut text = a.text.clone().unwrap_or_default();
			if ui.add(egui::TextEdit::multiline(&mut text).desired_rows(3).desired_width(f32::INFINITY)).changed() {
				doc.doc.lock().edit("Edit text", |d| {
					if let Some(x) = d.annotations.iter_mut().find(|x| x.id == id) {
						x.text = Some(text.clone());
					}
				});
			}
			if ui.small_button(format!("{} Edit on drawing", i::PENCIL)).clicked() {
				canvas.start_text_edit(doc, id);
			}
		}
		let mut style = a.style.clone();
		let (mut hl, mut st) = (a.style.stroke.clone(), a.text.clone().unwrap_or_default());
		if style_editor(ui, p, &mut style, a.kind, &mut hl, &mut st) {
			if a.kind == AnnotationKind::Highlight {
				style.stroke = hl;
			}
			doc.doc.lock().edit("Style", |d| {
				if let Some(x) = d.annotations.iter_mut().find(|x| x.id == id) {
					x.style = style.clone();
					if a.kind == AnnotationKind::Stamp {
						x.text = Some(st.clone());
					}
				}
			});
		}
		ui.add_space(6.0);
		widgets::section_label(ui, p, "Subject");
		let mut subject = a.subject.clone().unwrap_or_else(|| a.kind.label().to_string());
		if ui.add(egui::TextEdit::singleline(&mut subject).desired_width(f32::INFINITY)).changed() {
			doc.doc.lock().edit("Subject", |d| {
				if let Some(x) = d.annotations.iter_mut().find(|x| x.id == id) {
					x.subject = Some(subject.clone());
				}
			});
		}
		widgets::section_label(ui, p, "Status");
		let current = a.meta.review.clone().unwrap_or_else(|| "None".into());
		egui::ComboBox::from_id_salt("review").selected_text(&current).show_ui(ui, |ui| {
			for s in ["None", "Accepted", "Rejected", "Completed", "Cancelled", "Marked"] {
				if ui.selectable_label(current == s, s).clicked() {
					let id = id.to_string();
					doc.doc.lock().edit("Status", |d| {
						if let Some(x) = d.annotations.iter_mut().find(|x| x.id == id) {
							x.meta.review = (s != "None").then(|| s.to_string());
						}
					});
				}
			}
		});
		ui.add_space(8.0);
		self.common_meta(ui, p, &a.meta, "");
	}
}

fn header(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, sub: &str) {
	ui.horizontal(|ui| {
		ui.label(RichText::new(icon).font(theme::icon_font(18.0)).color(p.accent));
		ui.vertical(|ui| {
			ui.label(RichText::new(title).font(theme::semibold(14.5)));
			ui.label(RichText::new(sub).size(11.5).color(p.text2));
		});
	});
	ui.add_space(6.0);
}

fn note_editor(ui: &mut egui::Ui, p: &Palette, doc: &mut OpenDoc, id: &str, note: Option<String>) {
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Note");
	let mut text = note.unwrap_or_default();
	if ui.add(egui::TextEdit::multiline(&mut text).desired_rows(2).desired_width(f32::INFINITY).hint_text("Size, spec, condition…")).changed() {
		let id = id.to_string();
		doc.doc.lock().edit("Note", |d| {
			let v = (!text.trim().is_empty()).then(|| text.clone());
			if let Some(x) = d.marks.iter_mut().find(|x| x.id == id) {
				x.note = v;
			} else if let Some(x) = d.measurements.iter_mut().find(|x| x.id == id) {
				x.note = v;
			} else if let Some(x) = d.annotations.iter_mut().find(|x| x.id == id) {
				x.note = v;
			}
		});
	}
}

fn fields_editor(ui: &mut egui::Ui, p: &Palette, doc: &mut OpenDoc, id: &str, values: std::collections::BTreeMap<String, String>) {
	let defs = doc.doc.lock().data.custom_fields.clone();
	if defs.is_empty() {
		return;
	}
	ui.add_space(6.0);
	widgets::section_label(ui, p, "Custom columns");
	egui::Grid::new("fields").num_columns(2).show(ui, |ui| {
		for f in &defs {
			ui.label(RichText::new(&f.name).color(p.text2));
			let mut v = values.get(&f.id).cloned().unwrap_or_default();
			if ui.text_edit_singleline(&mut v).changed() {
				let (id, fid) = (id.to_string(), f.id.clone());
				doc.doc.lock().edit("Field", |d| {
					let set = |fields: &mut Option<std::collections::BTreeMap<String, String>>| {
						let map = fields.get_or_insert_with(Default::default);
						if v.is_empty() {
							map.remove(&fid);
						} else {
							map.insert(fid.clone(), v.clone());
						}
					};
					if let Some(x) = d.marks.iter_mut().find(|x| x.id == id) {
						set(&mut x.fields);
					} else if let Some(x) = d.measurements.iter_mut().find(|x| x.id == id) {
						set(&mut x.fields);
					}
				});
			}
			ui.end_row();
		}
	});
}

/// Edits an annotation style; returns true when something changed.
pub fn style_editor(ui: &mut egui::Ui, p: &Palette, style: &mut AnnotationStyle, kind: AnnotationKind, highlight: &mut String, stamp: &mut String) -> bool {
	let mut changed = false;
	widgets::section_label(ui, p, if kind == AnnotationKind::Highlight { "Highlight color" } else { "Line color" });
	let current = if kind == AnnotationKind::Highlight { highlight.clone() } else { style.stroke.clone() };
	if let Some(c) = color_row(ui, &current) {
		if kind == AnnotationKind::Highlight {
			*highlight = c;
		} else {
			style.stroke = c;
		}
		changed = true;
	}
	if !matches!(kind, AnnotationKind::Highlight) {
		ui.add_space(4.0);
		let mut w = style.width as f32;
		if ui.add(egui::Slider::new(&mut w, 0.0..=8.0).step_by(0.5).text("Width (pt)")).changed() {
			style.width = w as f64;
			changed = true;
		}
		let mut o = style.opacity as f32;
		if ui.add(egui::Slider::new(&mut o, 0.1..=1.0).text("Opacity")).changed() {
			style.opacity = o as f64;
			changed = true;
		}
		ui.horizontal(|ui| {
			for (s, l) in [(None, "Solid"), (Some(LineStyle::Dashed), "Dashed"), (Some(LineStyle::Dotted), "Dotted")] {
				changed |= ui.selectable_value(&mut style.line_style, s, l).changed();
			}
		});
	}
	if matches!(kind, AnnotationKind::Rectangle | AnnotationKind::Ellipse | AnnotationKind::Polygon | AnnotationKind::Cloud | AnnotationKind::Text | AnnotationKind::Callout) {
		ui.add_space(4.0);
		let mut has_fill = style.fill.is_some();
		if ui.checkbox(&mut has_fill, "Fill").changed() {
			style.fill = has_fill.then(|| "#FFF4CC".to_string());
			changed = true;
		}
		if let Some(f) = style.fill.clone() {
			if let Some(c) = color_row(ui, &f) {
				style.fill = Some(c);
				changed = true;
			}
		}
	}
	if matches!(kind, AnnotationKind::Text | AnnotationKind::Callout | AnnotationKind::Stamp) {
		let mut fs = style.font_size as f32;
		if ui.add(egui::Slider::new(&mut fs, 5.0..=72.0).text("Font size (pt)")).changed() {
			style.font_size = fs as f64;
			changed = true;
		}
	}
	if kind == AnnotationKind::Stamp {
		widgets::section_label(ui, p, "Stamp");
		ui.horizontal_wrapped(|ui| {
			for s in ["APPROVED", "REVIEWED", "REVISE", "REJECTED", "VERIFY IN FIELD", "NOT IN CONTRACT", "FOR INFORMATION"] {
				if ui.selectable_label(stamp == s, s).clicked() {
					*stamp = s.to_string();
					changed = true;
				}
			}
		});
		changed |= ui.text_edit_singleline(stamp).changed();
	}
	changed
}

pub fn describe_scale(s: &PageScale, page_w_pt: f64) -> String {
	// Express as paper inches → real units, and match a preset if it is one.
	let ref_frac = ((s.p2.x - s.p1.x).powi(2) + (s.p2.y - s.p1.y).powi(2)).sqrt();
	if ref_frac <= 0.0 {
		return "Invalid scale".into();
	}
	let paper_in = ref_frac * page_w_pt / 72.0;
	let real_per_in = s.distance / paper_in;
	for preset in ScalePreset::all() {
		if preset.unit == s.unit && ((preset.real / preset.paper_in) - real_per_in).abs() / real_per_in < 0.002 {
			return preset.label.to_string();
		}
	}
	if s.unit == "ft" {
		format!("1\" = {:.2}' (calibrated)", real_per_in)
	} else {
		format!("1 in paper = {:.3} {} (calibrated)", real_per_in, s.unit)
	}
}
