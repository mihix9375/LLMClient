use crate::ui::theme::{self, UiPalette};
use iced::widget::canvas::{self, path::Arc, Canvas, Frame, Geometry, LineCap, LineJoin, Path, Stroke};
use iced::widget::{button, container};
use iced::{Color, Element, Length, Point, Radians, Renderer, Theme};
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    Menu,
    Sun,
    Moon,
    Help,
    Refresh,
    Settings,
    Close,
    Command,
    Search,
    Plus,
    Paperclip,
    ArrowDown,
    Send,
    Download,
}

pub struct VectorIcon {
    pub kind: IconKind,
    pub color: Color,
}

impl<M> canvas::Program<M> for VectorIcon {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height).max(8.0);
        let c = frame.center();
        let stroke = Stroke::default()
            .with_color(self.color)
            .with_width((s * 0.115).clamp(1.3, 2.0))
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round);

        let pt = |dx: f32, dy: f32| Point::new(c.x + dx * s, c.y + dy * s);

        match self.kind {
            IconKind::Menu => {
                for dy in [-0.25, 0.0, 0.25] {
                    let p = Path::line(pt(-0.31, dy), pt(0.31, dy));
                    frame.stroke(&p, stroke);
                }
            }
            IconKind::Sun => {
                frame.stroke(&Path::circle(c, 0.17 * s), stroke);
                for k in 0..8 {
                    let a = (k as f32) * (PI * 0.25);
                    let p = Path::line(
                        Point::new(c.x + a.cos() * 0.28 * s, c.y + a.sin() * 0.28 * s),
                        Point::new(c.x + a.cos() * 0.37 * s, c.y + a.sin() * 0.37 * s),
                    );
                    frame.stroke(&p, stroke);
                }
            }
            IconKind::Moon => {
                let p = Path::new(|b| {
                    b.move_to(pt(0.06, -0.32));
                    b.bezier_curve_to(pt(-0.36, -0.26), pt(-0.36, 0.28), pt(0.10, 0.32));
                    b.bezier_curve_to(pt(0.22, 0.33), pt(0.30, 0.24), pt(0.33, 0.14));
                    b.bezier_curve_to(pt(0.08, 0.18), pt(-0.10, -0.02), pt(0.06, -0.32));
                    b.close();
                });
                frame.stroke(&p, stroke);
            }
            IconKind::Help => {
                let p = Path::new(|b| {
                    b.move_to(pt(-0.16, -0.17));
                    b.bezier_curve_to(pt(-0.16, -0.34), pt(0.18, -0.34), pt(0.18, -0.14));
                    b.bezier_curve_to(pt(0.18, -0.02), pt(0.0, 0.01), pt(0.0, 0.12));
                });
                frame.stroke(&p, stroke);
                frame.fill(&Path::circle(pt(0.0, 0.29), (s * 0.07).max(1.2)), self.color);
            }
            IconKind::Refresh => {
                let r = 0.27;
                let end_a = 5.55f32;
                let tip_x = end_a.cos() * r;
                let tip_y = end_a.sin() * r;
                let arc = Path::new(|b| {
                    b.arc(Arc {
                        center: c,
                        radius: r * s,
                        start_angle: Radians(0.55),
                        end_angle: Radians(end_a),
                    });
                });
                frame.stroke(&arc, stroke);
                let head = Path::new(|b| {
                    b.move_to(pt(0.22, -0.34));
                    b.line_to(pt(tip_x, tip_y));
                    b.line_to(pt(0.04, -0.19));
                });
                frame.stroke(&head, stroke);
            }
            IconKind::Settings => {
                frame.stroke(&Path::circle(c, 0.13 * s), stroke);
                frame.stroke(&Path::circle(c, 0.26 * s), stroke);
                for k in 0..6 {
                    let a = (k as f32) * (PI / 3.0);
                    let ray = Path::line(
                        Point::new(c.x + a.cos() * 0.26 * s, c.y + a.sin() * 0.26 * s),
                        Point::new(c.x + a.cos() * 0.36 * s, c.y + a.sin() * 0.36 * s),
                    );
                    frame.stroke(&ray, stroke);
                }
            }
            IconKind::Close => {
                frame.stroke(&Path::line(pt(-0.24, -0.24), pt(0.24, 0.24)), stroke);
                frame.stroke(&Path::line(pt(0.24, -0.24), pt(-0.24, 0.24)), stroke);
            }
            IconKind::Command => {
                let p = Path::new(|b| {
                    b.move_to(pt(-0.26, -0.22));
                    b.line_to(pt(-0.02, 0.0));
                    b.line_to(pt(-0.26, 0.22));
                    b.move_to(pt(0.04, 0.22));
                    b.line_to(pt(0.30, 0.22));
                });
                frame.stroke(&p, stroke);
            }
            IconKind::Search => {
                frame.stroke(&Path::circle(pt(-0.06, -0.06), 0.22 * s), stroke);
                frame.stroke(&Path::line(pt(0.10, 0.10), pt(0.30, 0.30)), stroke);
            }
            IconKind::Plus => {
                frame.stroke(&Path::line(pt(-0.27, 0.0), pt(0.27, 0.0)), stroke);
                frame.stroke(&Path::line(pt(0.0, -0.27), pt(0.0, 0.27)), stroke);
            }
            IconKind::Paperclip => {
                let p = Path::new(|b| {
                    b.move_to(pt(0.18, -0.05));
                    b.line_to(pt(-0.08, 0.21));
                    b.bezier_curve_to(pt(-0.20, 0.33), pt(-0.34, 0.19), pt(-0.22, 0.07));
                    b.line_to(pt(0.09, -0.24));
                    b.bezier_curve_to(pt(0.19, -0.34), pt(0.31, -0.22), pt(0.21, -0.12));
                    b.line_to(pt(-0.04, 0.13));
                });
                frame.stroke(&p, stroke);
            }
            IconKind::ArrowDown => {
                frame.stroke(&Path::line(pt(0.0, -0.28), pt(0.0, 0.24)), stroke);
                let head = Path::new(|b| {
                    b.move_to(pt(-0.20, 0.05));
                    b.line_to(pt(0.0, 0.26));
                    b.line_to(pt(0.20, 0.05));
                });
                frame.stroke(&head, stroke);
            }
            IconKind::Send => {
                frame.stroke(&Path::line(pt(-0.28, 0.0), pt(0.26, 0.0)), stroke);
                let head = Path::new(|b| {
                    b.move_to(pt(0.05, -0.21));
                    b.line_to(pt(0.27, 0.0));
                    b.line_to(pt(0.05, 0.21));
                });
                frame.stroke(&head, stroke);
            }
            IconKind::Download => {
                frame.stroke(&Path::line(pt(0.0, -0.28), pt(0.0, 0.14)), stroke);
                let head = Path::new(|b| {
                    b.move_to(pt(-0.18, -0.02));
                    b.line_to(pt(0.0, 0.16));
                    b.line_to(pt(0.18, -0.02));
                });
                frame.stroke(&head, stroke);
                let tray = Path::new(|b| {
                    b.move_to(pt(-0.28, 0.14));
                    b.line_to(pt(-0.28, 0.28));
                    b.line_to(pt(0.28, 0.28));
                    b.line_to(pt(0.28, 0.14));
                });
                frame.stroke(&tray, stroke);
            }
        }
        vec![frame.into_geometry()]
    }
}

pub fn icon<'a, M: 'a>(kind: IconKind, size: f32, color: Color) -> Element<'a, M> {
    Canvas::new(VectorIcon { kind, color })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

pub fn icon_button<'a, M: Clone + 'a>(
    kind: IconKind,
    box_w: f32,
    box_h: f32,
    icon_sz: f32,
    pal: UiPalette,
    msg: M,
) -> Element<'a, M> {
    button(container(icon(kind, icon_sz, pal.ink_secondary)).center(Length::Fill))
        .padding(0)
        .width(Length::Fixed(box_w))
        .height(Length::Fixed(box_h))
        .style(theme::btn_icon(pal))
        .on_press(msg)
        .into()
}
