//! Rasterizing Palace draw commands onto a [`Canvas`].
//!
//! A room's paint is a list of [`DrawCmd`]s split into two layers. Commands
//! flagged `DF_LAYER_FRONT` are stored in the **front** list and commands
//! without it in the **back** list; the compositor rasterizes back at its
//! reference layer (after the room dim, before loose props) and front at its own
//! (after the above-avatars overlays, before name tags). Within one list,
//! commands paint in arrival order, so the later command covers the earlier one.
//!
//! ## Undo and detonate
//!
//! `DC_Delete` pops the **most recent** command from whichever layer it was
//! added to; `DC_Detonate` clears every command in both layers.
//! [`DrawList`] keeps a layer history alongside the two lists so a delete knows
//! which list to pop. This mirrors `OpenPalace PalaceClient.as:1608`
//! `handleDrawCommand` (`drawLayerHistory`) and `QPRoom` (`mDraws.removeLast`
//! / `mDraws.clear`).
//!
//! ## Geometry
//!
//! * A `PATH`/`SHAPE` operand's points are **relative to the point before
//!   them**: the first point is absolute and every later point is a delta.
//!   `OpenPalace PalaceController.as:787` states it ("all points are relative
//!   to the one before them") and `PaintLayer.as:123-127` sums them.
//! * `DF_IS_ELLIPSE` selects an ellipse; its two points are read the way
//!   `PaintLayer.as:84-89` reads them (centre then radii, components swapped).
//! * `DF_USE_FILL` fills the shape with the fill colour, falling back to the
//!   pen colour when the optional PC5 tail is absent.
//! * `DC_Text`'s operand layout is undocumented (the 1999 reference says "the
//!   remaining drawing operations need to be documented"). It is deliberately
//!   **not** rasterized here; the raw bytes stay on the [`DrawCmd`].

use palace_room::{draw_cmd, draw_flags, DrawCmd, DrawPayload};

use crate::canvas::Canvas;

/// Which of the two paint layers a command belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawLayer {
    /// `DF_LAYER_FRONT` clear — painted after the dim, under loose props.
    Back,
    /// `DF_LAYER_FRONT` set — painted above avatars, under name tags.
    Front,
}

/// The layer a command belongs to, from its `DF_LAYER_FRONT` flag.
#[must_use]
pub fn layer_of(cmd: &DrawCmd) -> DrawLayer {
    if cmd.is_front_layer() {
        DrawLayer::Front
    } else {
        DrawLayer::Back
    }
}

/// A room's draw commands, split by layer, with the undo history.
///
/// The two lists are the front and back command lists the reference client
/// keeps. [`DrawList::apply`] is the single entry point for a received record:
/// it appends a draw command, pops the last command on `DC_Delete`, or clears
/// both lists on `DC_Detonate`.
#[derive(Debug, Clone, Default)]
pub struct DrawList {
    back: Vec<DrawCmd>,
    front: Vec<DrawCmd>,
    history: Vec<DrawLayer>,
}

impl DrawList {
    /// An empty draw list.
    #[must_use]
    pub fn new() -> Self {
        DrawList::default()
    }

    /// Build a list by applying `commands` in order.
    #[must_use]
    pub fn from_commands(commands: impl IntoIterator<Item = DrawCmd>) -> Self {
        let mut list = DrawList::new();
        for cmd in commands {
            list.apply(cmd);
        }
        list
    }

    /// Apply one received draw record.
    ///
    /// A `DC_Delete` removes the most recent draw command of the layer it was
    /// added to; a `DC_Detonate` clears everything; anything else is appended to
    /// the back or front list according to its layer flag.
    pub fn apply(&mut self, cmd: DrawCmd) {
        match cmd.command {
            draw_cmd::DELETE => {
                self.undo();
            }
            draw_cmd::DETONATE => self.detonate(),
            _ => {
                let layer = layer_of(&cmd);
                match layer {
                    DrawLayer::Back => self.back.push(cmd),
                    DrawLayer::Front => self.front.push(cmd),
                }
                self.history.push(layer);
            }
        }
    }

    /// Undo the most recent command, if any, returning it.
    pub fn undo(&mut self) -> Option<DrawCmd> {
        match self.history.pop()? {
            DrawLayer::Back => self.back.pop(),
            DrawLayer::Front => self.front.pop(),
        }
    }

    /// Delete every draw command.
    pub fn detonate(&mut self) {
        self.back.clear();
        self.front.clear();
        self.history.clear();
    }

    /// The back-layer commands, oldest first.
    #[must_use]
    pub fn back(&self) -> &[DrawCmd] {
        &self.back
    }

    /// The front-layer commands, oldest first.
    #[must_use]
    pub fn front(&self) -> &[DrawCmd] {
        &self.front
    }

    /// Number of draw commands across both layers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.back.len() + self.front.len()
    }

    /// True when both layers are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.back.is_empty() && self.front.is_empty()
    }
}

/// Rasterize the back layer onto `canvas`.
pub fn rasterize_back(canvas: &mut Canvas, list: &DrawList) {
    rasterize_layer(canvas, list.back());
}

/// Rasterize the front layer onto `canvas`.
pub fn rasterize_front(canvas: &mut Canvas, list: &DrawList) {
    rasterize_layer(canvas, list.front());
}

/// Rasterize one layer's commands in order.
pub fn rasterize_layer(canvas: &mut Canvas, commands: &[DrawCmd]) {
    for cmd in commands {
        rasterize_command(canvas, cmd);
    }
}

/// Rasterize one command, if its operand decoded.
fn rasterize_command(canvas: &mut Canvas, cmd: &DrawCmd) {
    let Some(payload) = cmd.payload.as_ref() else {
        return;
    };
    if payload.points.is_empty() {
        return;
    }
    let pen = pen_width(cmd, payload);
    let line = line_colour(payload);
    let fill = fill_colour(payload);

    if is_ellipse(cmd) {
        let fill_colour = cmd.is_filled().then_some(fill);
        rasterize_ellipse(canvas, payload, pen, line, fill_colour);
        return;
    }

    let points = absolute_points(payload);
    if cmd.is_filled() {
        fill_polygon(canvas, &points, fill);
    }
    stroke_polyline(canvas, &points, pen, line, cmd.command == draw_cmd::SHAPE);
}

/// True when a command draws an ellipse (`DC_Ellipse` or `DF_IS_ELLIPSE`).
#[must_use]
pub fn is_ellipse(cmd: &DrawCmd) -> bool {
    cmd.command == draw_cmd::ELLIPSE || cmd.flags & draw_flags::IS_ELLIPSE != 0
}

/// The pen width in logical pixels.
///
/// `PalaceDrawRecord.readData` zeroes `penSize` when the PC5 tail is absent and
/// the command is an ellipse or a fill ("PalaceChat 3 style"). That behaviour is
/// reproduced here; a zero pen is still a hairline, so the result floors at 1.
fn pen_width(cmd: &DrawCmd, payload: &DrawPayload) -> i32 {
    let pc3_style = payload.line_rgba.is_none() && (cmd.is_filled() || is_ellipse(cmd));
    let declared = if pc3_style {
        0
    } else {
        payload.pen_size.max(0)
    };
    declared.max(1) as i32
}

/// The line colour as `[r, g, b, a]`.
///
/// The optional PC5 line tail carries its own alpha; without it the pen colour
/// is opaque.
fn line_colour(payload: &DrawPayload) -> [u8; 4] {
    match payload.line_rgba {
        Some([a, r, g, b]) => [r, g, b, a],
        None => [
            payload.pen_rgb[0],
            payload.pen_rgb[1],
            payload.pen_rgb[2],
            255,
        ],
    }
}

/// The fill colour as `[r, g, b, a]`, falling back to the line colour the way
/// the reference does when no fill tail is present.
fn fill_colour(payload: &DrawPayload) -> [u8; 4] {
    match payload.fill_rgba {
        Some([a, r, g, b]) => [r, g, b, a],
        None => line_colour(payload),
    }
}

/// Absolute `(x, y)` vertices from the wire points, summing the deltas.
///
/// `Point` is `(v, h)`, i.e. `(y, x)`. The first point starts the running sum at
/// the origin, so it is its own absolute position; every later point is added to
/// the running sum.
#[must_use]
pub fn absolute_points(payload: &DrawPayload) -> Vec<(i32, i32)> {
    let mut x = 0i32;
    let mut y = 0i32;
    let mut out = Vec::with_capacity(payload.points.len());
    for p in &payload.points {
        x = x.saturating_add(i32::from(p.h));
        y = y.saturating_add(i32::from(p.v));
        out.push((x, y));
    }
    out
}

/// Ellipse `(cx, cy, rx, ry)` from a two-point operand.
///
/// Ported from `OpenPalace PaintLayer.as:84-89`: the centre is `points[0]` and
/// the radii are half of `points[1]`, with the components **swapped** — the
/// reference comments "points x and y are reversed on ellipses". An ellipse's
/// two points are read directly, not delta-summed. The 1999 protocol reference
/// leaves the ellipse operand undocumented, so this is the reference client's
/// behaviour rather than a recovered wire fact; `None` means the operand cannot
/// describe an ellipse.
#[must_use]
pub fn ellipse_geometry(payload: &DrawPayload) -> Option<(i32, i32, i32, i32)> {
    let p0 = payload.points.first()?;
    let p1 = payload.points.get(1)?;
    Some((
        i32::from(p0.v),
        i32::from(p0.h),
        (i32::from(p1.v) / 2).abs(),
        (i32::from(p1.h) / 2).abs(),
    ))
}

/// Stroke a polyline, optionally closing it back to the first point.
fn stroke_polyline(
    canvas: &mut Canvas,
    points: &[(i32, i32)],
    width: i32,
    rgba: [u8; 4],
    close: bool,
) {
    if points.is_empty() {
        return;
    }
    for pair in points.windows(2) {
        stroke_segment(canvas, pair[0], pair[1], width, rgba);
    }
    if let [single] = points {
        stamp(canvas, *single, width, rgba);
    }
    if close && points.len() > 2 {
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            stroke_segment(canvas, *last, *first, width, rgba);
        }
    }
}

/// Bresenham from `a` to `b`, stamping the brush at every step.
fn stroke_segment(canvas: &mut Canvas, a: (i32, i32), b: (i32, i32), width: i32, rgba: [u8; 4]) {
    let (x0, y0) = a;
    let (x1, y1) = b;
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        stamp(canvas, (x, y), width, rgba);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

/// Stamp a `width × width` square brush centred on a logical pixel.
fn stamp(canvas: &mut Canvas, at: (i32, i32), width: i32, rgba: [u8; 4]) {
    let half = width / 2;
    let lo = -half;
    let hi = lo + width;
    for dy in lo..hi {
        for dx in lo..hi {
            canvas.paint_pixel(at.0 + dx, at.1 + dy, rgba);
        }
    }
}

/// Fill a closed polygon with an even-odd scanline fill.
fn fill_polygon(canvas: &mut Canvas, points: &[(i32, i32)], rgba: [u8; 4]) {
    if points.len() < 3 {
        return;
    }
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for p in points {
        min_y = min_y.min(p.1);
        max_y = max_y.max(p.1);
    }
    let mut crossings: Vec<i32> = Vec::new();
    for y in min_y..=max_y {
        crossings.clear();
        for i in 0..points.len() {
            let (x0, y0) = points[i];
            let (x1, y1) = points[(i + 1) % points.len()];
            let spans = (y0 <= y && y1 > y) || (y1 <= y && y0 > y);
            if spans {
                let denominator = f64::from(y1 - y0);
                if denominator != 0.0 {
                    let t = f64::from(y - y0) / denominator;
                    crossings.push((f64::from(x0) + t * f64::from(x1 - x0)).round() as i32);
                }
            }
        }
        crossings.sort_unstable();
        for pair in crossings.chunks_exact(2) {
            for x in pair[0]..=pair[1] {
                canvas.paint_pixel(x, y, rgba);
            }
        }
    }
}

/// Draw an ellipse: an optional fill, then the outline at `pen` width.
fn rasterize_ellipse(
    canvas: &mut Canvas,
    payload: &DrawPayload,
    pen: i32,
    line: [u8; 4],
    fill: Option<[u8; 4]>,
) {
    let Some((cx, cy, rx, ry)) = ellipse_geometry(payload) else {
        return;
    };
    if rx <= 0 || ry <= 0 {
        return;
    }
    if let Some(fill) = fill {
        fill_ellipse(canvas, cx, cy, rx, ry, fill);
    }
    if pen > 0 {
        stroke_ellipse(canvas, cx, cy, rx, ry, pen, line);
    }
}

/// Fill the interior of an ellipse using the implicit equation.
fn fill_ellipse(canvas: &mut Canvas, cx: i32, cy: i32, rx: i32, ry: i32, rgba: [u8; 4]) {
    let rx2 = i64::from(rx) * i64::from(rx);
    let ry2 = i64::from(ry) * i64::from(ry);
    let limit = rx2 * ry2;
    for y in (cy - ry)..=(cy + ry) {
        let dy = i64::from(y - cy);
        for x in (cx - rx)..=(cx + rx) {
            let dx = i64::from(x - cx);
            if dx * dx * ry2 + dy * dy * rx2 <= limit {
                canvas.paint_pixel(x, y, rgba);
            }
        }
    }
}

/// Stroke an ellipse outline, parametrically sampled then brushed.
fn stroke_ellipse(
    canvas: &mut Canvas,
    cx: i32,
    cy: i32,
    rx: i32,
    ry: i32,
    pen: i32,
    rgba: [u8; 4],
) {
    let steps = (2 * rx.max(ry)).clamp(16, 1024);
    let mut points: Vec<(i32, i32)> = Vec::with_capacity(steps as usize + 1);
    for step in 0..=steps {
        let angle = std::f64::consts::TAU * f64::from(step) / f64::from(steps);
        let x = f64::from(cx) + f64::from(rx) * angle.cos();
        let y = f64::from(cy) + f64::from(ry) * angle.sin();
        points.push((x.round() as i32, y.round() as i32));
    }
    stroke_polyline(canvas, &points, pen, rgba, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{render, RenderOptions};
    use crate::scene::Scene;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// Convert an `[r, g, b, a]` colour to the operand tail's `[a, r, g, b]`.
    fn wire(rgba: [u8; 4]) -> [u8; 4] {
        [rgba[3], rgba[0], rgba[1], rgba[2]]
    }

    /// Build a draw record from its raw fields and parse it with the
    /// `palace-room` decoder, so the rasterizer's tests run on the real wire
    /// representation rather than a hand-built struct.
    fn record(
        command: u8,
        flags: u8,
        pen: i16,
        points: &[(i16, i16)],
        tail: Option<([u8; 4], [u8; 4])>,
    ) -> DrawCmd {
        let mut operand: Vec<u8> = Vec::new();
        operand.extend_from_slice(&pen.to_le_bytes());
        operand.extend_from_slice(&(points.len() as i16 - 1).to_le_bytes());
        operand.extend_from_slice(&[0x10, 0x10, 0x20, 0x20, 0x30, 0x30]);
        for (y, x) in points {
            operand.extend_from_slice(&y.to_le_bytes());
            operand.extend_from_slice(&x.to_le_bytes());
        }
        if let Some((line, fill)) = tail {
            operand.extend_from_slice(&wire(line));
            operand.extend_from_slice(&wire(fill));
        }

        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&0i16.to_le_bytes());
        bytes.extend_from_slice(&0i16.to_le_bytes());
        bytes.extend_from_slice(&(u16::from(command) | (u16::from(flags) << 8)).to_le_bytes());
        bytes.extend_from_slice(&(operand.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&10i16.to_le_bytes());
        bytes.extend_from_slice(&operand);

        let (cmd, warnings) =
            palace_room::decode_draw_record(&bytes, palace_wire::ByteOrder::Little);
        assert!(
            warnings.is_empty(),
            "test record must parse cleanly: {warnings:?}"
        );
        cmd
    }

    /// A filled shape with a solid tail colour, on the given layer.
    fn filled_shape(flags: u8, rgba: [u8; 4]) -> DrawCmd {
        // Triangle (2,2) -> (12,2) -> (2,12).
        record(
            draw_cmd::SHAPE,
            flags | draw_flags::USE_FILL,
            1,
            &[(2, 2), (0, 10), (10, -10)],
            Some((rgba, rgba)),
        )
    }

    fn px(canvas: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let at = ((y as usize) * (canvas.width() as usize) + x as usize) * 4;
        let s = &canvas.as_rgba()[at..at + 4];
        [s[0], s[1], s[2], s[3]]
    }

    fn render_with(commands: &[DrawCmd]) -> Canvas {
        let mut scene = Scene::new(20, 16);
        for cmd in commands {
            scene.draw.apply(cmd.clone());
        }
        render(&scene, RenderOptions::at_dpr(1.0))
    }

    #[test]
    fn a_path_changes_the_expected_canvas_pixels() {
        // Absolute (5,5) then a delta (+10 x) -> the segment y=5, x in 5..=15.
        let path = record(draw_cmd::PATH, 0, 1, &[(5, 5), (0, 10)], Some((RED, RED)));
        let canvas = render_with(&[path]);
        assert_eq!(px(&canvas, 10, 5), RED, "the stroke lies on y=5");
        assert_eq!(px(&canvas, 5, 5), RED, "including its first point");
        assert_eq!(px(&canvas, 15, 5), RED, "including its last point");
        assert_ne!(px(&canvas, 10, 8), RED, "and nowhere else");
    }

    #[test]
    fn a_filled_shape_changes_the_expected_canvas_pixels() {
        let shape = filled_shape(0, BLUE);
        let canvas = render_with(&[shape]);
        assert_eq!(px(&canvas, 4, 4), BLUE, "inside the triangle is filled");
        assert_eq!(
            px(&canvas, 2, 11),
            BLUE,
            "close to the right edge is inside"
        );
        assert_ne!(px(&canvas, 11, 11), BLUE, "outside the hypotenuse is not");
    }

    #[test]
    fn an_ellipse_changes_the_expected_canvas_pixels() {
        // OpenPalace mapping: centre (8,8); rx = |p1.v|/2 = 5, ry = |p1.h|/2 = 3.
        let ellipse = record(
            draw_cmd::ELLIPSE,
            draw_flags::USE_FILL,
            1,
            &[(8, 8), (10, 6)],
            Some((GREEN, GREEN)),
        );
        let canvas = render_with(&[ellipse]);
        assert_eq!(px(&canvas, 8, 8), GREEN, "the centre is inside the ellipse");
        assert_eq!(px(&canvas, 13, 8), GREEN, "and so is the rx extreme");
        assert_ne!(
            px(&canvas, 8, 15),
            GREEN,
            "8 px below the centre is outside"
        );
        assert_ne!(
            px(&canvas, 16, 8),
            GREEN,
            "8 px right of the centre is outside"
        );
    }

    #[test]
    fn front_commands_paint_over_back_commands() {
        let back = filled_shape(0, RED);
        let front = filled_shape(draw_flags::LAYER_FRONT, GREEN);
        let canvas = render_with(&[back.clone(), front.clone()]);
        assert_eq!(
            px(&canvas, 4, 4),
            GREEN,
            "the front layer is rasterized after the back layer"
        );

        // The same two commands in list order must not decide: the layer does.
        let canvas = render_with(&[front, back]);
        assert_eq!(px(&canvas, 4, 4), GREEN, "front still wins");
    }

    #[test]
    fn undo_removes_the_last_command_of_the_correct_layer() {
        let mut list = DrawList::new();
        list.apply(filled_shape(0, RED));
        list.apply(filled_shape(draw_flags::LAYER_FRONT, GREEN));
        assert_eq!(list.back().len(), 1);
        assert_eq!(list.front().len(), 1);

        let undone = list.undo().expect("a command is undone");
        assert!(
            undone.is_front_layer(),
            "the last command was on the front layer"
        );
        assert!(list.front().is_empty(), "the front layer is popped");
        assert_eq!(list.back().len(), 1, "the back layer is untouched");

        let undone = list.undo().expect("the back command is undone next");
        assert!(!undone.is_front_layer());
        assert!(list.is_empty());
        assert!(list.undo().is_none(), "an empty list undoes nothing");
    }

    #[test]
    fn apply_routes_delete_and_detonate_as_the_reference_does() {
        let mut list = DrawList::new();
        list.apply(filled_shape(0, RED));
        list.apply(filled_shape(draw_flags::LAYER_FRONT, GREEN));
        list.apply(record(draw_cmd::DELETE, 0, 0, &[], None));
        assert!(list.front().is_empty(), "delete pops the front command");
        assert_eq!(list.len(), 1);

        list.apply(filled_shape(0, RED));
        list.apply(filled_shape(draw_flags::LAYER_FRONT, GREEN));
        assert!(!list.back().is_empty(), "the back layer holds a command");
        assert!(!list.front().is_empty(), "and so does the front layer");
        list.apply(record(draw_cmd::DETONATE, 0, 0, &[], None));
        assert!(list.back().is_empty(), "detonate clears the back layer");
        assert!(
            list.front().is_empty(),
            "detonate clears the front layer too"
        );
        assert!(list.undo().is_none(), "and the history with it");
    }

    #[test]
    fn a_point_operand_that_cannot_describe_an_ellipse_is_ignored() {
        let one_point = record(draw_cmd::ELLIPSE, 0, 1, &[(5, 5)], Some((RED, RED)));
        let canvas = render_with(&[one_point]);
        assert_eq!(
            px(&canvas, 5, 5),
            [0, 0, 0, 255],
            "an ellipse with no second point paints nothing"
        );
    }

    #[test]
    fn a_text_command_is_not_rasterized() {
        // DC_Text's operand layout is undocumented: the bytes are carried but
        // deliberately not drawn, so the frame stays the empty backdrop.
        let text = record(draw_cmd::TEXT, 0, 1, &[(5, 5), (0, 10)], None);
        let canvas = render_with(&[text]);
        assert_eq!(px(&canvas, 5, 5), [0, 0, 0, 255]);
        assert_eq!(px(&canvas, 10, 5), [0, 0, 0, 255]);
    }

    #[test]
    fn absolute_points_sum_each_delta_onto_the_one_before() {
        let cmd = record(
            draw_cmd::PATH,
            0,
            1,
            &[(5, 5), (0, 10), (3, -10)],
            Some((RED, RED)),
        );
        let payload = cmd.payload.expect("operand decodes");
        assert_eq!(
            absolute_points(&payload),
            vec![(5, 5), (15, 5), (5, 8)],
            "each point is relative to its predecessor"
        );
    }
}
