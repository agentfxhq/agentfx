use super::ElementCx;
use crate::color::{Color, ToColorColor as _};
use anyrender::PaintScene;
use blitz_dom::local_name;
use kurbo::{BezPath, Cap, Circle, Join, Rect, Stroke};
use peniko::Fill;
use style::dom::TElement as _;
use style::values::computed::ColorOrAuto;

// reel patch: checkbox / radio drawn the way Chrome's `NativeThemeBase::PaintCheckbox` /
// `PaintRadio` (ui/native_theme/native_theme_base.cc, light mode, state normal) draw them,
// with the CSS `accent-color` (stylo patched to parse it for the servo engine). Colours are
// Chrome's `kColorWebNativeControl*` light-mode values, verified by a 4x headless probe
// (`results/m4d/probe-checkbox`).

/// `kColorWebNativeControlAccent` (light): the `accent-color: auto` accent.
const CHROME_ACCENT: Color = Color::from_rgb8(0, 117, 255);
/// `kColorWebNativeControlAccentDisabled` (light).
const CHROME_ACCENT_DISABLED: Color = Color::from_rgb8(197, 197, 197);
/// `kColorWebNativeControlBorder` (light).
const CHROME_BORDER: Color = Color::from_rgb8(118, 118, 118);
/// `kColorWebNativeControlBorderDisabled` (light).
const CHROME_BORDER_DISABLED: Color = Color::from_rgb8(197, 197, 197);
/// `kColorWebNativeControlCheckboxBackground` (light); the check mark and the dot gap use it.
const CHROME_BACKGROUND: Color = Color::WHITE;
/// `kColorWebNativeControlBackgroundDisabled` (light).
const CHROME_BACKGROUND_DISABLED: Color = Color::from_rgb8(239, 239, 239);

impl ElementCx<'_, '_> {
    pub(super) fn draw_input(&self, scene: &mut impl PaintScene) {
        if self.node.local_name() != "input" {
            return;
        }
        let Some(checked) = self.element.checkbox_input_checked() else {
            return;
        };

        let type_attr = self.node.attr(local_name!("type"));
        let is_checkbox = match type_attr {
            Some("checkbox") => true,
            Some("radio") => false,
            _ => return,
        };
        let disabled = self.node.attr(local_name!("disabled")).is_some();

        // `accent-color`; `auto` is Chrome's default accent. Disabled controls ignore it.
        let accent_color = if disabled {
            CHROME_ACCENT_DISABLED
        } else {
            let current_color = self.style.clone_color();
            match &self.style.get_inherited_ui().accent_color {
                ColorOrAuto::Auto => CHROME_ACCENT,
                ColorOrAuto::Color(c) => c.resolve_to_absolute(&current_color).as_srgb_color(),
            }
        };
        let border_color = match (checked, disabled) {
            (true, _) => accent_color,
            (false, false) => CHROME_BORDER,
            (false, true) => CHROME_BORDER_DISABLED,
        };
        let background = if disabled { CHROME_BACKGROUND_DISABLED } else { CHROME_BACKGROUND };

        let tf = self.transform;
        let s = self.scale;
        let bb = self.frame.border_box;
        let side = bb.width().min(bb.height());
        let sq = Rect::from_center_size(bb.center(), (side, side));
        let border_width = 1.0 * s;

        // "If the square is too small then paint only a square."
        if side <= 2.0 * s {
            scene.fill(Fill::NonZero, tf, border_color, None, &sq);
            return;
        }
        let radius = if is_checkbox { 2.0 * s } else { side * 0.5 };

        // Background, shrunk by 0.2 border widths against AA seams with the border.
        let bg = sq.inset(-0.2 * border_width).to_rounded_rect(radius);
        scene.fill(Fill::NonZero, tf, background, None, &bg);

        // Border (a checked checkbox gets the accent fill instead).
        if !is_checkbox || !checked {
            let st = Stroke { width: border_width, join: Join::Miter, miter_limit: 4.0, start_cap: Cap::Butt, end_cap: Cap::Butt, dash_pattern: Default::default(), dash_offset: 0.0 };
            scene.stroke(&st, tf, border_color, None, &sq.inset(-border_width / 2.0).to_rounded_rect(radius));
        }

        if is_checkbox {
            if checked {
                scene.fill(Fill::NonZero, tf, accent_color, None, &sq.to_rounded_rect(radius));
                // The check mark: a two-segment polyline, butt caps, miter joins, 0.16 of the side wide.
                let (w, h) = (sq.width(), sq.height());
                let mut check = BezPath::new();
                check.move_to((sq.x0 + w * 0.2, sq.y0 + h * 0.5));
                check.line_to((sq.x0 + w * 0.4, sq.y0 + h * 0.7));
                check.line_to((sq.x1 - w * 0.2, sq.y0 + h * 0.2));
                let st = Stroke { width: h * 0.16, join: Join::Miter, miter_limit: 4.0, start_cap: Cap::Butt, end_cap: Cap::Butt, dash_pattern: Default::default(), dash_offset: 0.0 };
                scene.stroke(&st, tf, background, None, &check);
            }
        } else if checked {
            // The dot: the square inset by 0.2 of the side, rounded at 0.5 side, i.e. a circle of radius 0.3 side.
            scene.fill(Fill::NonZero, tf, accent_color, None, &Circle::new(sq.center(), side * 0.3));
        }
    }
}
