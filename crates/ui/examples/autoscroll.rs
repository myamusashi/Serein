//! Offline debug check: cargo run --locked -p ui --example autoscroll
fn rows(shape: &egui::Shape, clip: egui::Rect, visible: &mut Vec<u64>) {
	match shape {
		egui::Shape::Text(text) if clip.contains(text.pos) => {
			if let Some(id) = text
				.galley
				.job
				.text
				.strip_prefix("Autoscroll row ")
				.and_then(|text| text.parse().ok())
			{
				visible.push(id);
			}
		}
		egui::Shape::Vec(shapes) => {
			for shape in shapes {
				rows(shape, clip, visible);
			}
		}
		_ => {}
	}
}
fn main() {
	let mut state = test_support::empty_channel_demo_state(false);
	let channel = state.selected.unwrap();
	for id in 1..=500 {
		let mut message = test_support::message(id, channel);
		message.content = format!("Autoscroll row {id}");
		message.embeds.clear();
		message.attachments.clear();
		message.reactions = Some(vec![]);
		state.timeline.insert(message, false, false).unwrap();
	}
	state
		.channels
		.iter_mut()
		.find(|entry| entry.id == channel)
		.unwrap()
		.last_message = Some(model::Id(500));
	let ctx = egui::Context::default();
	ui::design::apply(&ctx);
	let mut view = ui::MessagingUi::default();
	let mut number = 0;
	let repaint_delay = std::cell::Cell::new(std::time::Duration::ZERO);
	let mut frame = |events| {
		number += 1;
		let output = ctx.run_ui(
			egui::RawInput {
				time: Some(f64::from(number) / 60.0),
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1120.0, 760.0),
				)),
				events,
				..Default::default()
			},
			|ui| {
				let _ = view.show(ui, &mut state);
			},
		);
		repaint_delay.set(output.viewport_output[&egui::ViewportId::ROOT].repaint_delay);
		let mut visible = Vec::new();
		for shape in &output.shapes {
			rows(&shape.shape, shape.clip_rect, &mut visible);
		}
		output.drop_without_applying_deltas();
		*visible.iter().max().expect("chat must remain visible")
	};
	for _ in 0..12 {
		frame(vec![]);
	}
	let origin = egui::pos2(600.0, 350.0);
	let button = |pressed| egui::Event::PointerButton {
		pos: origin,
		button: egui::PointerButton::Middle,
		pressed,
		modifiers: egui::Modifiers::NONE,
	};
	frame(vec![egui::Event::PointerMoved(origin)]);
	frame(vec![button(true)]);
	frame(vec![button(false)]);
	for (y, upward) in [(250.0, true), (450.0, false)] {
		let start = frame(vec![egui::Event::PointerMoved(egui::pos2(600.0, y))]);
		let mut previous = start;
		for _ in 0..60 {
			let current = frame(vec![]);
			assert!(
				current.abs_diff(previous) <= 5,
				"chat jumped: {previous} -> {current}"
			);
			assert!(
				if upward {
					current <= previous
				} else {
					current >= previous
				},
				"chat reversed: {previous} -> {current}"
			);
			previous = current;
		}
		assert!(
			if upward {
				previous < start
			} else {
				previous > start
			},
			"autoscroll must keep moving"
		);
	}
	for y in [100.0, 600.0] {
		frame(vec![egui::Event::PointerMoved(egui::pos2(600.0, y))]);
		for _ in 0..300 {
			frame(vec![]);
		}
		assert!(
			repaint_delay.get() >= std::time::Duration::from_millis(16),
			"autoscroll must stop scheduling animation frames at the boundary (pointer y={y})"
		);
	}
	println!(
		"PASS: synthetic chat scrolls continuously in both directions without reversing or jumping; no animation loop at either boundary."
	);
}
